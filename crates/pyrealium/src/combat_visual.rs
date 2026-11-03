//! Narrow training melee presentation from native 7148 physics serializers.
//! DoMode: 00495420; stamped IID: 004946d0; BehaviorParams: 00679220.
//! The human 44000001 combo table resolves one-hand mode + Melee_Auto1.
//! Damage, range and timing remain the authored encounter owner's authority.
use crate::world::WorldError;

pub const DO_MODE: u32 = 0x10082;
pub const DO_BEHAVIOR: u32 = 0x1007f;
/// PhysicsStory on the victim. StreamPack 006ae550; hook pack 006ad9f0.
pub const DO_STORY: u32 = 0x100a8;
pub const PEACE: u32 = 0x40000001;
/// The `death` behaviour and the `dead` mode (`gmbehaviors.emp`, `gmmodes.emp`).
pub const DEATH_BEHAVIOR: u32 = 0x4100001f;
pub const DEAD_MODE: u32 = 0x41000005;
pub const ONE_HAND: u32 = 0x4100000a;
pub const MARTIAL_ARTS: u32 = 0x4100000f;
// Legions EnumMapper 23000026 / 23000027.
pub const MISSILE: u32 = 0x41000010;
pub const MAGIC: u32 = 0x41000011;
pub const BOULDER: u32 = 0x41000015;
pub const MAGIC_AUTO: u32 = 0x410000b0;
pub const MISSILE_AUTO: u32 = 0x410000e2;
pub const MELEE_AUTO: u32 = 0x41000089;
pub const MELEE_NORMAL: u32 = 0x41000077;
/// Captured result paired with a nonzero health hook. Not a reconstructed crit table.
pub const HEALTH_RESULT: u32 = 0x40000008;

/// Presentation grace, not combat authority or a loot lifetime. Native Legions
/// death graphs: Idol 03000072 = 2.375s; Drudge 0300002a variants <= 4.375s.
/// Uncatalogued profiles receive room for the 10s humanoid death/hold clips.
pub fn death_presentation_ms(entity: u32) -> u64 {
    match entity {
        crate::quests::TARGET_ENTITY => 3500,
        crate::shelter::DIGGER | crate::shelter::SENTINEL => 5500,
        _ => 12000,
    }
}

#[derive(Clone, Copy)]
pub enum IdolAction {
    Combat,
    Swing,
    Peace,
    Death,
}

/// Exact Idol Spawn profile: BehaviorTable 44000018 and retained fight capture.
/// Return the final shared visual stamp, snapshot mode and fresh wire messages.
pub fn idol(
    owner: u64,
    instance: u16,
    current_stamp: u16,
    action: IdolAction,
) -> Result<(u16, u32, Vec<Vec<u8>>), WorldError> {
    let final_stamp = current_stamp
        .checked_add(if matches!(action, IdolAction::Swing) {
            2
        } else {
            1
        })
        .ok_or(WorldError::LimitExceeded)?;
    let mode = match action {
        IdolAction::Combat | IdolAction::Swing => 0x41000012,
        IdolAction::Peace => PEACE,
        IdolAction::Death => 0x41000005,
    };
    let mut events = Vec::new();
    if matches!(action, IdolAction::Death) {
        let mut wire = prefix(DO_BEHAVIOR, owner, instance, final_stamp)?;
        wire.extend(3u32.to_le_bytes()); // behavior and explicit mode, no captured context.
        wire.extend(0x4100001fu32.to_le_bytes());
        wire.extend(mode.to_le_bytes());
        events.push(wire);
    } else {
        let mut wire = prefix(DO_MODE, owner, instance, current_stamp + 1)?;
        wire.extend(mode.to_le_bytes());
        events.push(wire);
        if matches!(action, IdolAction::Swing) {
            events.push(swing(owner, instance, final_stamp)?);
        }
    }
    Ok((final_stamp, mode, events))
}

fn prefix(opcode: u32, owner: u64, instance: u16, stamp: u16) -> Result<Vec<u8>, WorldError> {
    if [0, u64::MAX].contains(&owner) || instance == 0 || stamp == 0 {
        return Err(WorldError::InvalidValue);
    }
    let mut wire = Vec::with_capacity(24);
    wire.extend(opcode.to_le_bytes());
    wire.extend(owner.to_le_bytes());
    wire.extend(instance.to_le_bytes());
    wire.extend(stamp.to_le_bytes());
    Ok(wire)
}
pub fn appearance(owner:u64, instance:u16, stamp:u16, visual:&[u8]) -> Result<Vec<u8>, WorldError> {
    if crate::appearance::visual_len(visual) != Some(visual.len()) {return Err(WorldError::InvalidValue);}
    let mut wire=prefix(0x1009f,owner,instance,stamp)?;
    wire.extend(visual); Ok(wire)
}
/// Legions shared Cast_Attack (23000051 / DBFXTable 1e000056) plus racial
/// impacts in 1e000075. The older named *_cast enums have no DAT FX binding.
/// This uses the native shared attack cue; elemental spell-specific sequencing
/// still requires the original skill effect scripts.
pub fn magic_effects(race:u32) -> Option<(u32,u32)> {
    match race {1=>Some((0x810001cd,0x41000248)),2=>Some((0x810001cd,0x410002e5)),4=>Some((0x810001cd,0x41000391)),_=>None}
}
pub fn effect(owner:u64,instance:u16,stamp:u16,fx:u32) -> Result<Vec<u8>,WorldError> {
    if !matches!(fx>>24,0x40|0x41|0x81) {return Err(WorldError::InvalidValue);}
    let mut wire=prefix(0x100a0,owner,instance,stamp)?;wire.extend(fx.to_le_bytes());wire.extend(1f32.to_le_bytes());Ok(wire)
}

pub fn mode(owner: u64, instance: u16, stamp: u16, fighting: bool) -> Result<Vec<u8>, WorldError> {
    mode_id(owner,instance,stamp,if fighting { ONE_HAND } else { PEACE })
}
/// A player's own death: the `death` behaviour into the `dead` mode with the actions locked
/// (`BehaviorParams` bits 1, 2 and 0x800), so the client's `PlayerMovementModule` stops the
/// keys (`PlayerMovementModule::LockActions`) and the body lies down. Pyrealium policy: no capture
/// of the retail server's message is known.
pub fn player_death(owner: u64, instance: u16, stamp: u16) -> Result<Vec<u8>, WorldError> {
    let mut wire = prefix(DO_BEHAVIOR, owner, instance, stamp)?;
    wire.extend(0x803u32.to_le_bytes());
    wire.extend(DEATH_BEHAVIOR.to_le_bytes());
    wire.extend(DEAD_MODE.to_le_bytes());
    wire.extend(1u32.to_le_bytes());
    Ok(wire)
}
/// A player's respawn: `SetMode` of the peace mode at once and `StopBehavior` of the death
/// behaviour, which releases its action lock.
pub fn player_alive(owner: u64, instance: u16, stamp: u16) -> Result<Vec<u8>, WorldError> {
    let mut wire = prefix(0x100a9, owner, instance, stamp)?;
    wire.extend(PEACE.to_le_bytes());
    Ok(wire)
}
pub fn stop_death(owner: u64, instance: u16, stamp: u16) -> Result<Vec<u8>, WorldError> {
    let mut wire = prefix(0x10080, owner, instance, stamp)?;
    wire.extend(DEATH_BEHAVIOR.to_le_bytes());
    Ok(wire)
}
pub fn mode_id(owner: u64, instance: u16, stamp: u16, mode: u32) -> Result<Vec<u8>, WorldError> {
    if ![PEACE,ONE_HAND,MARTIAL_ARTS,MISSILE,MAGIC,BOULDER].contains(&mode) {return Err(WorldError::InvalidValue);}
    let mut wire = prefix(DO_MODE, owner, instance, stamp)?;
    wire.extend(mode.to_le_bytes());
    Ok(wire)
}

/// Player basic-attack presentation. Captured sword attacks use an attacker
/// PhysicsStory, whose native StoryManager creates a fresh behavior context.
/// Damage remains in the existing authoritative encounter/victim update path.
pub fn basic_attack(owner: u64, instance: u16, stamp: u16) -> Result<Vec<u8>, WorldError> {
    weapon_attack(owner, instance, stamp, ONE_HAND)
}
pub fn weapon_attack(owner: u64, instance: u16, stamp: u16, mode: u32) -> Result<Vec<u8>, WorldError> {
    attack_behavior(owner,instance,stamp,match mode {MAGIC=>MAGIC_AUTO,MISSILE|BOULDER=>MISSILE_AUTO,_=>MELEE_AUTO})
}
pub fn attack_behavior(owner:u64,instance:u16,stamp:u16,behavior:u32)->Result<Vec<u8>,WorldError> {
    let mut wire = prefix(DO_STORY, owner, instance, stamp)?;
    wire.extend(0x26u32.to_le_bytes()); // Attacker, behavior, basic-attack flag.
    wire.extend(owner.to_le_bytes());
    wire.extend(0x601u32.to_le_bytes()); // Behavior, propagate, send-event.
    wire.extend(behavior.to_le_bytes());
    wire.extend(1u32.to_le_bytes()); // Propagate to the held weapon.
    wire.extend(0u32.to_le_bytes()); // No interpreted completion callback.
    wire.extend(0u32.to_le_bytes()); // No duplicated damage hooks.
    Ok(wire)
}

/// A skill's story on its caster: the attacker, its cast behaviour and the skill
/// (`PhysicsStory` bits `0x2 | 0x4 | 0x10`; not a basic attack), with one fire-missile hook
/// (`StoryHookData` type 3) at `target` naming the skill's missile hook `hook`
/// (`crate::skill_missiles`). The client's script names the missile
/// (`GetMissileInformationFromWSL(skill, hook, ..)`) and `StoryManager::FireMissile` makes and
/// fires it at the target. The hook runs once the cast behaviour has passed `hook` animation
/// hooks (`StoryManager::DoCurrentStoryHooks`: attack hook 0 at once), or when the behaviour is
/// done (`FinishStory`). Pyrealium policy: which animation hook the retail server named is not
/// known; attack hook `n` is taken to come at the cast's `n`-th animation hook.
pub fn skill_missile_story(owner:u64,instance:u16,stamp:u16,behavior:u32,skill:u32,target:u64,hook:u32)->Result<Vec<u8>,WorldError> {
    if [0, u64::MAX].contains(&target) {return Err(WorldError::InvalidValue);}
    let mut wire = prefix(DO_STORY, owner, instance, stamp)?;
    wire.extend(0x16u32.to_le_bytes()); // Attacker, behavior, skill.
    wire.extend(owner.to_le_bytes());
    wire.extend(0x601u32.to_le_bytes()); // Behavior, propagate, send-event.
    wire.extend(behavior.to_le_bytes());
    wire.extend(1u32.to_le_bytes()); // Propagate to the held weapon.
    wire.extend(0u32.to_le_bytes()); // No interpreted completion callback.
    wire.extend(skill.to_le_bytes());
    wire.extend(1u32.to_le_bytes()); // One hook.
    wire.extend((3u32 | 0x100 | 0x200 | 0x400).to_le_bytes()); // Fire missile: target, animation and attack hooks.
    wire.extend(target.to_le_bytes());
    wire.extend(hook.to_le_bytes()); // Animation hook.
    wire.extend(hook.to_le_bytes()); // Attack hook.
    Ok(wire)
}

pub fn swing(owner: u64, instance: u16, stamp: u16) -> Result<Vec<u8>, WorldError> {
    let mut wire = prefix(DO_BEHAVIOR, owner, instance, stamp)?;
    wire.extend(1u32.to_le_bytes()); // Only mBehaviorID; native defaults for all other fields.
    // Legions Idol map 03000072 has Melee_Normal1 (81000043), but no Auto1.
    wire.extend(MELEE_NORMAL.to_le_bytes());
    Ok(wire)
}

/// Victim-owned health story. Bitfield 2 carries the attacker and omits skill,
/// behavior and the basic-attack bit. The single hook is type 1 (SetTypeToDamage,
/// 006ad840) with target, result and signed health. StoryManager::DoStoryHook
/// 00514ff0 presents type 1; it does not replace the separate vital-quality update.
/// Negative health is damage. Positive health is recovery. Zero is not packed.
pub fn damage_story(
    victim: u64,
    instance: u16,
    stamp: u16,
    attacker: u64,
    health_change: i32,
) -> Result<Vec<u8>, WorldError> {
    if [0, u64::MAX].contains(&attacker) || health_change == 0 {
        return Err(WorldError::InvalidValue);
    }
    let mut wire = prefix(DO_STORY, victim, instance, stamp)?;
    wire.extend(2u32.to_le_bytes());
    wire.extend(attacker.to_le_bytes());
    wire.extend(1u32.to_le_bytes());
    wire.extend(0x981u32.to_le_bytes());
    wire.extend(victim.to_le_bytes());
    wire.extend(HEALTH_RESULT.to_le_bytes());
    wire.extend(health_change.to_le_bytes());
    Ok(wire)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unarmed_mode_and_idol_attack_have_native_animation_keys() {
        let wire=mode_id(1,1,1,MARTIAL_ARTS).unwrap();
        assert_eq!(&wire[16..20],&MARTIAL_ARTS.to_le_bytes());
        assert!(mode_id(1,1,1,0).is_err());
        let (_,mode,events)=idol(2,1,1,IdolAction::Swing).unwrap();
        assert_eq!(mode,0x41000012);assert_eq!(events.len(),2);
        assert_eq!(&events[1][20..24],&MELEE_NORMAL.to_le_bytes());
    }

    #[test]
    fn a_skill_missile_story_names_the_skill_and_its_fire_hook() {
        let wire = skill_missile_story(0x11, 2, 9, 0x410000b6, 0x59, 0x22, 1).unwrap();
        assert_eq!(wire.len(), 72);
        assert_eq!(&wire[16..20], &0x16u32.to_le_bytes());
        assert_eq!(&wire[28..32], &0x601u32.to_le_bytes());
        assert_eq!(&wire[32..36], &0x410000b6u32.to_le_bytes());
        assert_eq!(&wire[44..48], &0x59u32.to_le_bytes());
        assert_eq!(&wire[48..52], &1u32.to_le_bytes());
        assert_eq!(&wire[52..56], &0x703u32.to_le_bytes());
        assert_eq!(&wire[56..64], &0x22u64.to_le_bytes());
        assert_eq!(&wire[64..68], &1u32.to_le_bytes());
        assert_eq!(&wire[68..72], &1u32.to_le_bytes());
        assert!(skill_missile_story(0x11, 2, 9, 0x410000b6, 0x59, 0, 1).is_err());
    }

    #[test]
    fn damage_story_matches_the_captured_victim_hook() {
        let wire = damage_story(0x11, 2, 9, 0x22, -20).unwrap();
        assert_eq!(wire.len(), 52);
        assert_eq!(&wire[..4], &DO_STORY.to_le_bytes());
        assert_eq!(&wire[4..12], &0x11u64.to_le_bytes());
        assert_eq!(&wire[12..14], &2u16.to_le_bytes());
        assert_eq!(&wire[14..16], &9u16.to_le_bytes());
        assert_eq!(&wire[16..20], &2u32.to_le_bytes());
        assert_eq!(&wire[20..28], &0x22u64.to_le_bytes());
        assert_eq!(&wire[28..32], &1u32.to_le_bytes());
        assert_eq!(&wire[32..36], &0x981u32.to_le_bytes());
        assert_eq!(&wire[36..44], &0x11u64.to_le_bytes());
        assert_eq!(&wire[44..48], &HEALTH_RESULT.to_le_bytes());
        assert_eq!(&wire[48..52], &(-20i32).to_le_bytes());
        let heal = damage_story(0x11, 2, 10, 0x11, 25).unwrap();
        assert_eq!(&heal[48..52], &25i32.to_le_bytes());
        assert!(damage_story(0x11, 2, 10, 0x22, 0).is_err());
        assert!(damage_story(0, 2, 10, 0x22, -1).is_err());
        assert!(damage_story(0x11, 2, 10, 0, -1).is_err());
    }
}
