//! Native projectile presentation with bounded, authored development timing.
//! Entity/visual pairs come from the Legions skill scripts and entity records.
//! Release timing and speed below are server rules, not recovered retail physics.
use crate::{
    movement::Placement,
    scene::{Delivery, Scene},
    world::{self, WorldError},
};

pub const RELEASE_MS: u64 = 350;
pub const SPEED: f32 = 22.0;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub entity: u32,
    pub visual: u32,
}
pub fn profile(race: u32, category: usize, skill: bool) -> Option<Profile> {
    let (entity, visual) = match (race, category, skill) {
        // 7000179f/700017c5, 7000187f/7000188d, 70001a48/70001a55.
        (1, 1, _) => (0x470000be, 0x1f000066),
        (2, 1, _) => (0x47000307, 0x1f000392),
        (4, 1, _) => (0x47000308, 0x1f000393),
        // Auto-magic 7000173c, 700017fe, 700019d8. Starter magic skills
        // 60/111/2a0 have direct cast/impact scripts, without a projectile.
        (1, 2, false) => (0x47000393, 0x1f0003ce),
        (2, 2, false) => (0x47000394, 0x1f0003cf),
        (4, 2, false) => (0x47000395, 0x1f0003d0),
        _ => return None,
    };
    Some(Profile { entity, visual })
}
pub fn flight_ms(distance: f32) -> u64 {
    ((distance / SPEED * 1000.0).ceil() as u64).clamp(100, 1000)
}
fn position(bytes: &mut Vec<u8>, cell: u32, origin: [f32; 3]) {
    bytes.extend(cell.to_le_bytes());
    for v in origin {
        bytes.extend(v.to_le_bytes());
    }
    for v in [1f32, 0., 0., 0.] {
        bytes.extend(v.to_le_bytes());
    }
}
fn launch(
    id: u64,
    target: u64,
    profile: Profile,
    mut from: Placement,
    to: Placement,
    ms: u64,
) -> Result<[Vec<u8>; 2], WorldError> {
    if !from.valid() || !to.valid() || target == 0 || !(100..=1000).contains(&ms) {
        return Err(WorldError::InvalidValue);
    }
    from.coordinates[2] += 1.2;
    let mut aim = to.coordinates;
    aim[2] += 1.0;
    // Outdoor blocks have distinct 160m origins; indoor cells share block space.
    if (1..=64).contains(&(from.cell & 0xffff)) && (1..=64).contains(&(to.cell & 0xffff)) {
        aim[0] += ((to.cell >> 24) as i32 - (from.cell >> 24) as i32) as f32 * 160.;
        aim[1] += (((to.cell >> 16) & 255) as i32 - ((from.cell >> 16) & 255) as i32) as f32 * 160.;
    }
    let create = world::create_stationary_object(&world::StationaryObject {
        character_id: id,
        parent_visual_did: profile.visual,
        entity_did: profile.entity,
        cell_id: from.cell,
        position: from.coordinates,
        orientation: [1., 0., 0., 0.],
        instance_stamp: 1,
        visual_ordering_stamp: 1,
        timestamps: [0; 4],
    })?;
    // ECM_Physics::InitializeMissile: stamped sender, Position, velocity,
    // TargetPosition (IID, relative Position, target velocity, initialized).
    let mut init = 0x100b5u32.to_le_bytes().to_vec();
    init.extend(id.to_le_bytes());
    init.extend(1u16.to_le_bytes());
    init.extend(2u16.to_le_bytes());
    position(&mut init, from.cell, from.coordinates);
    for i in 0..3 {
        init.extend(((aim[i] - from.coordinates[i]) / (ms as f32 / 1000.)).to_le_bytes());
    }
    init.extend(target.to_le_bytes());
    position(&mut init, 0, [0., 0., 1.]);
    for _ in 0..3 {
        init.extend(0f32.to_le_bytes());
    }
    init.extend(1u32.to_le_bytes());
    Ok([create, init])
}
struct Live {
    id: u64,
    until: u64,
    recipients: Vec<u64>,
}
#[derive(Default)]
pub struct Projectiles {
    sequence: u64,
    live: Vec<Live>,
}
impl Projectiles {
    pub fn fire(
        &mut self,
        scene: &Scene,
        player: u64,
        target: u64,
        profile: Profile,
        ms: u64,
        now: u64,
    ) -> Result<Vec<Delivery>, WorldError> {
        if self.live.len() >= 128 {
            return Err(WorldError::LimitExceeded);
        }
        let from = scene
            .player_placement(player)
            .ok_or(WorldError::InvalidValue)?;
        let to = scene
            .content_placement(target)
            .ok_or(WorldError::InvalidValue)?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .filter(|v| *v < (1 << 48))
            .ok_or(WorldError::LimitExceeded)?;
        let id = 0x8170000000000000 | self.sequence;
        let [create, init] = launch(id, target, profile, from, to, ms)?;
        let mut messages = scene.broadcast(player, create)?;
        let recipients = messages.iter().map(|d| d.recipient).collect();
        messages.extend(scene.broadcast(player, init)?);
        self.live.push(Live {
            id,
            until: now.saturating_add(ms),
            recipients,
        });
        Ok(messages)
    }
    pub fn retire(&mut self, now: u64) -> Vec<Delivery> {
        let mut result = Vec::new();
        self.live.retain(|p| {
            if now < p.until {
                return true;
            }
            let data: std::sync::Arc<[u8]> = world::destroy_inventory_item(p.id, 1).into();
            // Use original recipients even if the attacker changed area/disconnected.
            result.extend(p.recipients.iter().map(|&recipient| Delivery {
                recipient,
                data: data.clone(),
            }));
            false
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_projectile_envelope_and_racial_profiles() {
        for race in [1, 2, 4] {
            for category in [1, 2] {
                let profile = profile(race, category, false).unwrap();
                let [create, init] = launch(
                    0x8170000000000001,
                    2,
                    profile,
                    Placement::HUMAN_START,
                    Placement::HUMAN_START,
                    500,
                )
                .unwrap();
                assert_eq!(create[16..20], profile.visual.to_le_bytes());
                assert_eq!(init.len(), 116);
                assert_eq!(init[0..4], 0x100b5u32.to_le_bytes());
                assert_eq!(init[60..68], 2u64.to_le_bytes());
            }
            assert!(profile(race, 2, true).is_none());
        }
        assert!(profile(8, 1, false).is_none());
        assert_eq!(flight_ms(0.), 100);
        assert!(flight_ms(18.) < 1000);
    }
}
