//! Quest emotes with no object in front of the player: a death, a logout, a run outliving its
//! time limit (`quest_emotes` personal triggers), and sets carrying on after an action's delay
//! (`quest_emotes::pending`).
use super::*;
use crate::quest_emotes::{Outcome, Resume, Trigger};

/// Commits an outcome that changes quests and talks only, whatever the player is doing; returns
/// the journal updates and lines for the player. A set that paused again is queued.
pub(super) fn commit_quest_outcome(store: &mut Store, character: u64, outcome: &Outcome) -> io::Result<Vec<Vec<u8>>> {
    let before = store.characters.get(&character).ok_or_else(|| invalid("Quest record owner unavailable"))?.quests.clone();
    store.quest_records(character, &outcome.quests)?;
    let after = store.characters[&character].quests.clone();
    let mut events = after.updates_since(&before).map_err(other)?;
    for (table, key) in &outcome.tells { events.push(crate::quest_emotes::tell(*table, *key).map_err(other)?); }
    events.extend(outcome_lines(outcome, None, &before, &after)?);
    for line in &outcome.lines { events.push(script::chat(line, 1).map_err(other)?); }
    crate::quest_emotes::instances::committed(character, outcome);
    Ok(events)
}

/// A personal trigger (PlayerDeath, QuestExpired, Logout) for a character: each quest's set,
/// committed in turn. Returns the events for the player (dropped if it is offline).
pub(super) fn personal_quests(store: &mut Store, character: u64, trigger: Trigger) -> io::Result<Vec<Vec<u8>>> {
    let Some(tables) = crate::quest_emotes::tables() else { return Ok(Vec::new()) };
    let Some(c) = store.characters.get(&character).filter(|c| !c.deleted) else { return Ok(Vec::new()) };
    let outcomes = tables.run_personal(trigger, &c.quests, &c.inventory, &c.progression, store.next_item_id()?,
        crate::clock::now(), &crate::quest_emotes::item)?;
    let mut events = Vec::new();
    for outcome in &outcomes { events.extend(commit_quest_outcome(store, character, outcome)?); }
    Ok(events)
}

/// A trigger on a world object with no use behind it (a lifestone bound, a portal travelled):
/// the object's set, committed as quest state and talk (`load` holds those sets to that).
pub(super) fn object_quests(store: &mut Store, character: u64, trigger: Trigger, entity: u32, iid: u64) -> Vec<Vec<u8>> {
    let mut run = || -> io::Result<Vec<Vec<u8>>> {
        let Some(tables) = crate::quest_emotes::tables() else { return Ok(Vec::new()) };
        let Some(c) = store.characters.get(&character).filter(|c| !c.deleted) else { return Ok(Vec::new()) };
        let Some(outcome) = tables.run(trigger, entity, iid, None, &c.quests, &c.inventory, &c.progression, store.next_item_id()?, 0.0,
            crate::clock::now(), &crate::quest_emotes::item)? else { return Ok(Vec::new()) };
        commit_quest_outcome(store, character, &outcome)
    };
    run().unwrap_or_else(|error| { eprintln!("Quest {trigger:?} sets for character {character:x}: {error}"); Vec::new() })
}

/// The same, where a failure must not stop the caller (a death, a departure): logged instead.
pub(super) fn personal_quests_logged(store: &mut Store, character: u64, trigger: Trigger) -> Vec<Vec<u8>> {
    personal_quests(store, character, trigger).unwrap_or_else(|error| {
        eprintln!("Quest {trigger:?} sets for character {character:x}: {error}");
        Vec::new()
    })
}

/// A character left the world: its Logout sets run and its waiting sets are dropped.
pub(super) fn departed(store: &mut Store, character: u64) {
    crate::quest_emotes::pending::forget(character);
    personal_quests_logged(store, character, Trigger::Logout);
}

/// Each pass: sets whose delay ran out carry on, and (every second) runs past their time limit
/// run their QuestExpired sets once.
pub(super) fn quest_tick(srv: &mut Server, peers: &mut BTreeMap<SocketAddr, Peer>) -> io::Result<()> {
    let Some(tables) = crate::quest_emotes::tables() else { return Ok(()) };
    let updates = world_ops(srv)?;
    deliver_scene(updates, &srv.active_characters, peers, &srv.primary, srv.now, &mut srv.failed_peers)?;
    for (character, resume) in crate::quest_emotes::pending::take_due() {
        if let Err(error) = resume_set(srv, peers, &tables, character, &resume) {
            eprintln!("Quest set {} resumed for character {character:x}: {error}", resume.frames.first().map_or("", |f| f.0.as_str()));
        }
    }
    if srv.now_ms < srv.next_quest_expiry { return Ok(()); }
    srv.next_quest_expiry = srv.now_ms + 1000;
    let timed: BTreeSet<u32> = tables.timed_quests().collect();
    if timed.is_empty() { return Ok(()); }
    let unix = crate::clock::now();
    let characters: Vec<u64> = srv.active_characters.keys().copied().collect();
    for character in characters {
        let Some(book) = srv.store.characters.get(&character).map(|c| c.quests.clone()) else { continue };
        let fresh: Vec<(u32, u64)> = timed.iter().filter(|q| tables.expired(&book, **q, unix))
            .map(|q| (*q, book.timer(*q).started)).filter(|(q, started)| !srv.quest_expiries.contains(&(character, *q, *started))).collect();
        if fresh.is_empty() { continue; }
        srv.quest_expiries.extend(fresh.iter().map(|(q, started)| (character, *q, *started)));
        let events = personal_quests_logged(&mut srv.store, character, Trigger::QuestExpired);
        if let Some(peer) = srv.active_characters.get(&character).and_then(|addr| peers.get_mut(addr)) {
            peer.queue_ordered_batch(events)?;
        }
    }
    Ok(())
}

/// Carries on one waiting set for an online player. A part that only changes quests and talks
/// commits at once; one that gives, takes, awards or moves waits (another second) until the
/// player is alive and out of combat.
fn resume_set(srv: &mut Server, peers: &mut BTreeMap<SocketAddr, Peer>, tables: &crate::quest_emotes::Tables, character: u64, resume: &Resume)
    -> io::Result<()> {
    let Some(addr) = srv.active_characters.get(&character).copied() else { return Ok(()) };
    let Some(peer) = peers.get_mut(&addr) else { return Ok(()) };
    let Some(saved) = srv.store.characters.get(&character) else { return Ok(()) };
    let outcome = tables.resume(resume, &saved.quests, &saved.inventory, &saved.progression, srv.store.next_item_id()?,
        (srv.now_ms % 1000) as f32 / 1000.0, crate::clock::now(), &crate::quest_emotes::item)?;
    let talks_only = outcome.items.is_empty() && outcome.changes.is_empty() && outcome.moveto.is_none()
        && outcome.escort.is_none() && !outcome.end_escort;
    if talks_only {
        let events = commit_quest_outcome(&mut srv.store, character, &outcome)?;
        return peer.queue_ordered_batch(events);
    }
    if peer.combat || !saved.progression.alive() {
        crate::quest_emotes::pending::defer(character, Resume { frames: resume.frames.clone(), after_ms: 1000 });
        return Ok(());
    }
    let updates = commit_emote(peer, character, &outcome, Vec::new(), None, &mut srv.store, &mut srv.scene, &srv.inventory_metadata,
        &mut srv.encounters, &mut srv.bootstrap, srv.now_ms)?;
    deliver_scene(updates, &srv.active_characters, peers, &srv.primary, srv.now, &mut srv.failed_peers)
}

/// The spawn slots committed outcomes summon or send away (`quest_emotes::instances`).
fn world_ops(srv: &mut Server) -> io::Result<Vec<crate::scene::Delivery>> {
    let mut updates = Vec::new();
    let ops = crate::quest_emotes::instances::take_world();
    let Some(runtime) = srv.encounters.as_mut() else { return Ok(updates) };
    for op in ops {
        match op {
            crate::quest_emotes::instances::WorldOp::Dismiss(id) => updates.extend(dismiss_slot(runtime, &mut srv.scene, id, srv.now_ms)?),
            crate::quest_emotes::instances::WorldOp::Generate(id) => {
                // A slot whose creature still stands is left; an earlier one's corpse goes first.
                if !runtime.dormant(id) && !runtime.alive(id) { updates.extend(dismiss_slot(runtime, &mut srv.scene, id, srv.now_ms)?); }
                runtime.summon(id, srv.now_ms);
            }
        }
    }
    Ok(updates)
}
/// A spawn slot's creature (alive or a corpse) sent away now.
fn dismiss_slot(runtime: &mut crate::encounters::Encounters, scene: &mut crate::scene::Scene, id: u64, now: u64) -> io::Result<Vec<crate::scene::Delivery>> {
    let mut updates = Vec::new();
    if let Some(crate::encounters::Event::Despawn(id)) = runtime.dismiss(id) {
        if scene.content_placement(id).is_some() { updates.extend(scene.despawn_content(id)?); }
        runtime.removed(id, now);
    }
    Ok(updates)
}
