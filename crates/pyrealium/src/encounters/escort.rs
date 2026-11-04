//! Escorted creatures (`quest_emotes` StartEscort): who follows whom, when it walks, and when
//! it gives up on a player who left it.
//!
//! An escort walks after its player only while the player is near: within ESCORT_LEASH_OUTDOOR
//! (15 m) outdoors or ESCORT_LEASH_INDOOR (10 m) indoors (by the player's cell); farther, it waits
//! where it is. A fight comes first: an escort with a target fights it and does not walk. The
//! player stays the escort's while within the radar's range of it (the player's script's
//! `gRadarControl::InqRadarRanges`: 75 m outdoors, 50 m indoors, as AC2_Rusted's `radar_7160`
//! reads them), fighting or not; out of that range for ESCORT_ABANDON_MS (3 minutes) the escort
//! gives up (`Event::EscortAbandoned`, the quest's EscortAbandoned sets, else its EscortLost
//! ones) and stops following. The distances and the time are Pyrealium policy (the user's), no
//! retail source gives them.
use std::collections::{BTreeMap, BTreeSet};

use crate::movement::Placement;

/// The escort's distance behind its player.
pub(super) const ESCORT_DISTANCE: f32 = 2.5;
const ESCORT_LEASH_OUTDOOR: f32 = 15.0;
const ESCORT_LEASH_INDOOR: f32 = 10.0;
const RADAR_OUTDOOR: f32 = 75.0;
const RADAR_INDOOR: f32 = 50.0;
pub(super) const ESCORT_ABANDON_MS: u64 = 180_000;

fn indoor(cell: u32) -> bool { cell & 0xffff >= 0x100 }

/// What an escort does this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Step {
    /// Walk after the player (standing at `to`).
    Follow(Placement),
    /// Stand where it is: the player is beyond the leash, or it fights.
    Wait,
    /// Out of the player's radar too long: it gives up.
    Abandoned,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Escorts {
    /// Actor -> the player it follows.
    pub(super) by_actor: BTreeMap<u64, u64>,
    /// When each escort's player was last seen leaving its radar range.
    away: BTreeMap<u64, u64>,
    /// Escorts that told their player they wait (said once until the player comes back).
    waiting: BTreeSet<u64>,
}

impl Escorts {
    pub(super) fn start(&mut self, actor: u64, player: u64) { self.by_actor.insert(actor, player); self.forget_state(actor); }
    pub(super) fn player(&self, actor: u64) -> Option<u64> { self.by_actor.get(&actor).copied() }
    pub(super) fn has(&self, actor: u64) -> bool { self.by_actor.contains_key(&actor) }
    /// The escort leaves its player (delivered, dead, or given up).
    pub(super) fn remove(&mut self, actor: u64) -> Option<u64> { self.forget_state(actor); self.by_actor.remove(&actor) }
    pub(super) fn end_for(&mut self, player: u64) {
        let ended: Vec<u64> = self.by_actor.iter().filter(|(_, p)| **p == player).map(|(a, _)| *a).collect();
        for actor in ended { self.remove(actor); }
    }
    pub(super) fn of(&self, player: u64) -> Vec<u64> {
        self.by_actor.iter().filter(|(_, p)| **p == player).map(|(a, _)| *a).collect()
    }
    fn forget_state(&mut self, actor: u64) { self.away.remove(&actor); self.waiting.remove(&actor); }

    /// The escort `actor` (at `from`) this tick, its player at `to` (None: not in the world),
    /// `fighting` while it has a target. Notes for the player go to `notes`.
    pub(super) fn step(&mut self, actor: u64, from: Option<Placement>, to: Option<Placement>, fighting: bool, now: u64,
        notes: &mut Vec<&'static str>) -> Step {
        let distance = from.zip(to).and_then(|(a, b)| a.distance(b));
        let inside = to.is_some_and(|p| indoor(p.cell));
        let radar = if inside { RADAR_INDOOR } else { RADAR_OUTDOOR };
        if distance.is_some_and(|d| d <= radar) {
            if self.away.remove(&actor).is_some() { notes.push("Your escort sees you again."); }
        } else {
            let since = *self.away.entry(actor).or_insert_with(|| { notes.push("Your escort can no longer see you. Return within three minutes, or it gives up."); now });
            if now.saturating_sub(since) >= ESCORT_ABANDON_MS { return Step::Abandoned; }
        }
        let leash = if inside { ESCORT_LEASH_INDOOR } else { ESCORT_LEASH_OUTDOOR };
        match (to, distance) {
            (Some(to), Some(d)) if !fighting && d <= leash => { self.waiting.remove(&actor); Step::Follow(to) }
            _ => {
                if !fighting && distance.is_some_and(|d| d <= radar) && self.waiting.insert(actor) {
                    notes.push("Your escort waits for you to come closer.");
                }
                Step::Wait
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(cell: u32, x: f32) -> Placement { Placement { cell, coordinates: [x, 10.0, 0.0], heading: 0 } }

    #[test]
    fn an_escort_walks_near_its_player_waits_beyond_the_leash_and_gives_up_out_of_radar() {
        let mut e = Escorts::default();
        e.start(7, 1);
        let mut notes = Vec::new();
        let outdoor = 0x7d64_0021;
        // Outdoors: within 15 m it follows; at 20 m it waits (said once), still in radar.
        assert_eq!(e.step(7, Some(at(outdoor, 10.0)), Some(at(outdoor, 24.0)), false, 0, &mut notes), Step::Follow(at(outdoor, 24.0)));
        assert_eq!(e.step(7, Some(at(outdoor, 10.0)), Some(at(outdoor, 30.0)), false, 1000, &mut notes), Step::Wait);
        assert_eq!(e.step(7, Some(at(outdoor, 10.0)), Some(at(outdoor, 30.0)), false, 2000, &mut notes), Step::Wait);
        assert_eq!(notes, ["Your escort waits for you to come closer."]);
        // Fighting, it stands even with the player beside it.
        assert_eq!(e.step(7, Some(at(outdoor, 10.0)), Some(at(outdoor, 12.0)), true, 3000, &mut notes), Step::Wait);
        // Indoors the leash is 10 m.
        let inside = 0x0299_0110;
        assert_eq!(e.step(7, Some(at(inside, 10.0)), Some(at(inside, 22.0)), false, 4000, &mut notes), Step::Wait);
        assert_eq!(e.step(7, Some(at(inside, 10.0)), Some(at(inside, 19.0)), false, 5000, &mut notes), Step::Follow(at(inside, 19.0)));
        // Out of radar (60 m indoors) for three minutes: it gives up; back in sight in time, it does not.
        notes.clear();
        assert_eq!(e.step(7, Some(at(inside, 10.0)), Some(at(inside, 70.0)), false, 10_000, &mut notes), Step::Wait);
        assert_eq!(notes[0], "Your escort can no longer see you. Return within three minutes, or it gives up.");
        assert_eq!(e.step(7, Some(at(inside, 10.0)), Some(at(inside, 40.0)), false, 100_000, &mut notes), Step::Wait);
        assert!(notes.contains(&"Your escort sees you again."), "{notes:?}");
        assert_eq!(e.step(7, Some(at(inside, 10.0)), Some(at(inside, 70.0)), false, 110_000, &mut notes), Step::Wait);
        assert_eq!(e.step(7, Some(at(inside, 10.0)), None, false, 110_000 + ESCORT_ABANDON_MS - 1, &mut notes), Step::Wait, "logged off counts as away");
        assert_eq!(e.step(7, Some(at(inside, 10.0)), None, false, 110_000 + ESCORT_ABANDON_MS, &mut notes), Step::Abandoned);
        // Outdoors the radar reaches 75 m: a player 70 m off is still seen.
        let mut e = Escorts::default();
        e.start(8, 1);
        let mut notes = Vec::new();
        assert_eq!(e.step(8, Some(at(outdoor, 10.0)), Some(at(outdoor, 80.0)), false, 0, &mut notes), Step::Wait);
        assert_eq!(e.step(8, Some(at(outdoor, 10.0)), Some(at(outdoor, 80.0)), false, ESCORT_ABANDON_MS, &mut notes), Step::Wait);
        assert!(notes.iter().all(|n| !n.starts_with("Your escort can no longer")));
        assert_eq!(e.of(1), [8]);
        e.end_for(1);
        assert!(e.of(1).is_empty());
    }
}
