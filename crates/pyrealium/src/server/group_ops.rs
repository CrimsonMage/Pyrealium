//! Fellowship membership, kill-XP sharing and group publishing.
use super::*;

pub(super) fn group_online(
    active: &BTreeMap<u64, SocketAddr>,
    store: &Store,
    scene: &crate::scene::Scene,
) -> BTreeMap<u64, crate::groups::Member> {
    active
        .keys()
        .filter_map(|id| {
            let saved = store.characters.get(id).filter(|saved| !saved.deleted)?;
            let point = scene.player_placement(*id)?;
            let level = saved.progression.level();
            Some((
                *id,
                crate::groups::Member {
                    id: *id,
                    name: saved.name.clone(),
                    level: u32::from(level.number),
                    health: saved.progression.health,
                    max_health: level.health,
                    vigor: saved.progression.vigor,
                    max_vigor: level.vigor,
                    cell: point.cell,
                },
            ))
        })
        .collect()
}

/// A kill's experience split among the earner's fellowship (`fellowship_xp`) and paid, each
/// recipient's progression published as `encounters::Event::Owner` publishes the earner's.
/// Fellows count by the level rule whether alive or not (the panel's parts); only living ones
/// in the world receive.
pub(super) fn share_kill_xp(
    player: u64,
    xp: u64,
    groups: &crate::groups::Runtime,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    active: &BTreeMap<u64, SocketAddr>,
    initialize_ui: bool,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let fellows = groups.sharing(player).map(|sharing| {
        let fellows: Vec<crate::fellowship_xp::Fellow> = sharing
            .members
            .iter()
            .filter(|id| active.contains_key(id))
            .filter_map(|id| {
                let saved = store.characters.get(id).filter(|c| !c.deleted)?;
                Some(crate::fellowship_xp::Fellow {
                    id: *id,
                    level: u32::from(saved.progression.level().number),
                    placement: scene.player_placement(*id),
                })
            })
            .collect();
        (sharing.leader, sharing.shared, fellows)
    });
    let paid = match fellows {
        Some((leader, shared, fellows)) => crate::fellowship_xp::split(player, xp, leader, shared, &fellows),
        None => vec![(player, xp)],
    };
    let mut updates = Vec::new();
    for (id, amount) in paid {
        let Some(saved) = store.characters.get(&id).filter(|c| !c.deleted && c.progression.alive()) else {
            continue;
        };
        let (account, previous) = (saved.account.clone(), saved.progression.clone());
        if store
            .progression_command(&account, id, crate::progression::Command::AwardXp(amount))
            .is_err()
        {
            continue;
        }
        let saved = &store.characters[&id];
        let state = saved.progression.clone();
        updates.extend(scene.player_stats(id, &state));
        if initialize_ui
            && saved.skills.available_credits(&previous) != saved.skills.available_credits(&state)
        {
            updates.push(crate::scene::Delivery {
                recipient: id,
                data: crate::ui::skills_state(&saved.skills, &state, &saved.skill_used)
                    .map_err(other)?
                    .into(),
            });
        }
        for wire in state.private_changes(&previous) {
            updates.push(crate::scene::Delivery { recipient: id, data: wire.into() });
        }
    }
    Ok(updates)
}

pub(super) fn group_admit(
    deliveries: &[crate::groups::Delivery],
    active: &BTreeMap<u64, SocketAddr>,
    peers: &BTreeMap<SocketAddr, Peer>,
) -> io::Result<BTreeMap<SocketAddr, Vec<Vec<u8>>>> {
    let mut packets: BTreeMap<SocketAddr, Vec<Vec<u8>>> = BTreeMap::new();
    for delivery in deliveries {
        // A disconnect plan also clears the departed player's UI. There is no
        // transport to send that clear to, and reconnect starts with empty UI.
        if let Some(addr) = active.get(&delivery.recipient) {
            packets
                .entry(*addr)
                .or_default()
                .push(delivery.data.clone());
        }
    }
    for (addr, events) in &packets {
        peers
            .get(addr)
            .ok_or_else(|| invalid("Group recipient disconnected."))?
            .validate_scene_batch(events)?;
    }
    Ok(packets)
}

pub(super) fn group_publish(
    packets: BTreeMap<SocketAddr, Vec<Vec<u8>>>,
    peers: &mut BTreeMap<SocketAddr, Peer>,
) -> io::Result<()> {
    for (addr, events) in packets {
        peers
            .get_mut(&addr)
            .ok_or_else(|| invalid("Group recipient disconnected."))?
            .queue_ordered_batch(events)?;
    }
    Ok(())
}
