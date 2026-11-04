//! Forced moves: portals, map travel, recall and resurrection.
use super::*;

pub(super) struct ForcedMove {
    pub(super) motion: Motion,
    pub(super) update: crate::movement::ClientPosition,
    pub(super) wire: Vec<u8>,
}
pub(super) fn prepare_forced_move(
    peer: &Peer,
    character: u64,
    destination: Placement,
    scene: &mut crate::scene::Scene,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
) -> io::Result<ForcedMove> {
    if scene.player_placement(character).is_none()
        || !bootstrap
            .as_mut()
            .is_some_and(|profile| profile.contains_cell(destination.cell))
    {
        return Err(invalid("Saved destination is not available in this world."));
    }
    scene.prepare_cell(destination.cell)?;
    let (motion, update) = peer
        .motion
        .as_ref()
        .ok_or_else(|| invalid("Missing player motion state."))?
        .forced(destination)
        .map_err(other)?;
    let wire = crate::movement::server_position(
        character,
        motion.instance,
        motion.position_stamp,
        &update,
    )
    .map_err(other)?;
    Ok(ForcedMove {
        motion,
        update,
        wire,
    })
}
pub(super) fn use_portal(
    peer: &mut Peer,
    character: u64,
    target: u64,
    route: crate::portals::Route,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    encounters: &mut Option<crate::encounters::Encounters>,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
    now_ms: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if peer.combat || !saved.progression.alive() {
        return Err(invalid("Use a portal while alive and out of combat."));
    }
    if saved.progression.level().number < route.level {
        return Err(io::Error::other(format!(
            "This portal requires level {}.",
            route.level
        )));
    }
    if peer
        .last_world_use_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < 5000)
    {
        return Err(invalid("Wait a few seconds before using a portal."));
    }
    let player = scene
        .player_placement(character)
        .ok_or_else(|| invalid("Player is not ready."))?;
    let portal = scene
        .examination_content(character, target)
        .ok_or_else(|| invalid("Portal is not visible."))?;
    let portal_entity = portal.entity;
    let distance: f64 = player
        .coordinates
        .into_iter()
        .zip(portal.placement.coordinates)
        .map(|(a, b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    if player.cell >> 16 != portal.placement.cell >> 16 || distance > 16.0 {
        return Err(invalid("Move within four meters of the portal."));
    }
    let prepared = prepare_forced_move(peer, character, route.destination, scene, bootstrap)?;
    let mut events = vec![
        crate::ui::portal_used(character, target, route.destination, distance.sqrt() as f32)
            .map_err(other)?,
        prepared.wire,
        script::private_call(script::COMBAT_STATE, &script::integer(0)).map_err(other)?,
    ];
    if saved.quests.journey_phase() == 2 && crate::quests::is_training_portal(target) {
        events.extend(saved.quests.apply(&crate::quests::Command::Travelled).map_err(other)?.updates_since(&saved.quests).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    store.relocate_training_portal(&peer.session.account, character, route.destination, target)?;
    // The quests' Travel sets (`quest_emotes`).
    let travelled = object_quests(store, character, crate::quest_emotes::Trigger::Travel, portal_entity, target);
    if let Some(runtime) = encounters {
        runtime.stop(character);
    }
    let updates = scene
        .update(character, prepared.motion.position_stamp, &prepared.update)
        .map_err(other)?;
    peer.motion = Some(prepared.motion);
    peer.target = None;
    peer.last_world_use_ms = Some(now_ms);
    peer.queue_ordered_batch(events)?;
    if !travelled.is_empty() { peer.queue_ordered_batch(travelled)?; }
    Ok(updates)
}
pub(super) fn map_travel_character(
    peer: &mut Peer, character: u64, destination: Placement, store: &mut Store,
    scene: &mut crate::scene::Scene, encounters: &mut Option<crate::encounters::Encounters>,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>, now_ms: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid("Leave combat before map travel; the character must be alive."));
    }
    if peer.last_world_use_ms.is_some_and(|last| now_ms.saturating_sub(last) < 5000) {
        return Err(invalid("Wait a few seconds before traveling again."));
    }
    let prepared = prepare_forced_move(peer, character, destination, scene, bootstrap)?;
    let events = vec![prepared.wire, script::chat("Map travel accepted.", 1).map_err(other)?];
    peer.validate_scene_batch(&events)?;
    store.relocate(&peer.session.account, character, destination)?;
    if let Some(runtime) = encounters { runtime.stop(character); }
    let updates = scene.update(character, prepared.motion.position_stamp, &prepared.update).map_err(other)?;
    peer.motion = Some(prepared.motion);
    peer.target = None;
    peer.last_world_use_ms = Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
pub(super) fn recall_character(
    peer: &mut Peer,
    character: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    encounters: &mut Option<crate::encounters::Encounters>,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
    now_ms: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if peer.combat || !saved.progression.alive() {
        return Err(invalid("Recall while alive and out of combat."));
    }
    if peer
        .last_world_use_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < 5000)
    {
        return Err(invalid("Wait a few seconds before recalling."));
    }
    let destination = saved
        .binding
        .ok_or_else(|| invalid("Bind to a lifestone before recalling."))?;
    let prepared = prepare_forced_move(peer, character, destination, scene, bootstrap)?;
    let mut events = vec![prepared.wire];
    if saved.skills.get(crate::skills::LIFESTONE_RECALL).is_some() {
        events
            .push(crate::ui::skill_update(crate::skills::LIFESTONE_RECALL, &saved.skills, saved.skill_used.get(&crate::skills::LIFESTONE_RECALL).copied()).map_err(other)?);
    }
    events.push(script::private_call(script::COMBAT_STATE, &script::integer(0)).map_err(other)?);
    peer.validate_scene_batch(&events)?;
    store.return_to_binding(&peer.session.account, character, false)?;
    if let Some(runtime) = encounters {
        runtime.stop(character);
    }
    let updates = scene
        .update(character, prepared.motion.position_stamp, &prepared.update)
        .map_err(other)?;
    peer.motion = Some(prepared.motion);
    peer.target = None;
    peer.last_world_use_ms = Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
pub(super) fn resurrect_character(
    peer: &mut Peer,
    character: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    encounters: &mut Option<crate::encounters::Encounters>,
    lifestones: bool,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = store
        .characters
        .get(&character)
        .filter(|c| !c.deleted && c.account == peer.session.account)
        .ok_or_else(|| invalid("Character unavailable."))?;
    if saved.progression.alive() {
        return Err(invalid("You are already alive."));
    }
    let next = saved
        .progression
        .apply(&crate::progression::Command::Resurrect)
        .map_err(other)?;
    let forced = if lifestones {
        saved
            .binding
            .map(|destination| prepare_forced_move(peer, character, destination, scene, bootstrap))
            .transpose()?
    } else {
        None
    };
    let mut owner_events = next.private_updates();
    if let Some(prepared) = &forced {
        owner_events.push(prepared.wire.clone());
    }
    owner_events.push(crate::ui::lock_player(false).map_err(other)?);
    owner_events
        .push(script::private_call(script::COMBAT_STATE, &script::integer(0)).map_err(other)?);
    if let Some(request_id) = peer.pending_skill {
        owner_events.push(script::skill_finished(request_id).map_err(other)?);
    }
    peer.validate_scene_batch(&owner_events)?;
    if forced.is_some() {
        store.return_to_binding(&peer.session.account, character, true)?;
    } else {
        store.progression_command(
            &peer.session.account,
            character,
            crate::progression::Command::Resurrect,
        )?;
    }
    if let Some(runtime) = encounters {
        runtime.stop(character);
    }
    peer.combat = false;
    peer.target = None;
    peer.pending_skill = None;
    let state = &store.characters[&character].progression;
    let mut updates = scene.player_stats(character, state);
    updates.extend(player_body_alive(peer, scene, character, store.characters[&character].instance, true));
    if let Some(prepared) = forced {
        updates.extend(
            scene
                .update(character, prepared.motion.position_stamp, &prepared.update)
                .map_err(other)?,
        );
        peer.motion = Some(prepared.motion);
    }
    peer.queue_ordered_batch(owner_events)?;
    Ok(updates)
}
