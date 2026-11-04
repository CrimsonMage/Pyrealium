//! Instances: parallel copies of a place (an arena, a vault) a quest hands out one per player,
//! and the spawn slots in them a quest summons and sends away (ACE's instanced generators).
//!
//! An `instance_group` has lanes (`instance_lane`, each with its entry point); a `spawn_slot`
//! is one held world creature per lane (its placement's iid; it spawns only when summoned and
//! never regenerates). A player's lane is durable in the quest book (`quests::Command::Lane`);
//! which lanes are taken is kept here, rebuilt from every character's book at start and moved by
//! each committed outcome (`committed`). The creatures a committed outcome summons or sends away
//! are queued here for the server's tick, which has the world (`take_world`).
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::movement::Placement;

/// An instance group: its id (the quest book's key) and each lane's entry point.
#[derive(Clone, Debug, PartialEq)]
pub struct Group { pub id: u32, pub lanes: BTreeMap<u8, Placement> }
/// A spawn slot: its group and its creature (world iid) in each lane.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot { pub group: String, pub iids: BTreeMap<u8, u64>, pub arrivals: BTreeMap<u8, Placement> }

/// A creature a committed outcome summons or sends away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldOp { Generate(u64), Dismiss(u64) }

/// A group's id: FNV-1a of its name (stable while the name is).
pub fn group_id(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

static TAKEN: Mutex<BTreeSet<(u32, u8)>> = Mutex::new(BTreeSet::new());
static WORLD: Mutex<Vec<WorldOp>> = Mutex::new(Vec::new());

/// The lanes every character holds (at start, from their books).
pub fn rebuild<'a>(books: impl Iterator<Item = &'a crate::quests::Book>) {
    let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    taken.clear();
    for book in books { taken.extend(book.lanes()); }
}
/// The first lane of `group` nobody holds and `also` does not take.
pub fn free_lane(group: &Group, also: &[(u32, u8)]) -> Option<u8> {
    let taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    group.lanes.keys().copied().find(|lane| !taken.contains(&(group.id, *lane)) && !also.contains(&(group.id, *lane)))
}
/// An outcome was committed for `character`: its lanes are taken or freed, its creatures queued,
/// and a set that paused waits (`pending`).
pub fn committed(character: u64, outcome: &super::Outcome) {
    {
        let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
        for claim in &outcome.claims { taken.insert(*claim); }
        for release in &outcome.releases { taken.remove(release); }
    }
    if !outcome.world.is_empty() { WORLD.lock().unwrap_or_else(|e| e.into_inner()).extend(&outcome.world); }
    super::pending::defer_outcome(character, outcome);
}
/// The creatures to summon or send away, in order.
pub fn take_world() -> Vec<WorldOp> { std::mem::take(&mut *WORLD.lock().unwrap_or_else(|e| e.into_inner())) }
