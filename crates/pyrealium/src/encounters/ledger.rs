//! The timed effects on players and creatures, as the client lists them.
//!
//! The skill records name, per effect, the `Effect` state (`Effect`, package 0x14D: name, icon,
//! description) and a duration table. The runtimes (`mends`, `regen`, `ratings`, `burns`) do the
//! work of an effect; this ledger is the list of what is on whom and until when, which the
//! server tells the player's client with `WM_Effect` 0x17 (added) and 0x16 (removed) and shows
//! in a creature's examination. Pyrealium's own policy where the retail server's is unknown: a recast
//! of the same skill's effect replaces its record (a new effect id, the strip restarts its
//! timer), effects of different skills stack as separate records, and death, leaving the world
//! or a creature's despawn take every effect off.
use super::{Encounters, Event};
use crate::{scene::Scene, store::Store};

/// One effect on one owner.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub eid: u32,
    pub owner: u64,
    pub skill: u32,
    pub did: u32,
    pub cast: u64,
    pub until: u64,
    pub power: f32,
    pub pulsed: bool,
}

/// An effect as the event gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Applied {
    pub eid: u32,
    pub skill: u32,
    /// The `Effect` state.
    pub did: u32,
    /// `Encounters` time it was cast at, and how long it lasts.
    pub cast_ms: u64,
    pub duration_ms: u64,
    pub power: f32,
    pub pulsed: bool,
}

/// The ledger's skill id for the quest tables' effects (`quest_emotes` ApplyEffect): no skill
/// record is 0, so these never collide with a skill's own records.
pub const QUEST_EFFECT_SKILL: u32 = 0;

impl Encounters {
    /// Makes the ledger's quest effects of player `owner` those of its quest book (`active`:
    /// effect, until, both in the book's Unix seconds; `unix_now` the same clock, `now` the
    /// ledger's): missing ones join with their time left, gone ones leave. `announce` sends the
    /// changes to the client (`WM_Effect` 0x17 / 0x16); a login lists the ledger itself.
    pub fn sync_quest_effects(&mut self, owner: u64, active: &[(u32, u64)], unix_now: u64, now: u64, announce: bool) {
        let mut gone = Vec::new();
        self.ledger.retain(|e| {
            let keep = e.owner != owner || e.skill != QUEST_EFFECT_SKILL || active.iter().any(|(did, _)| *did == e.did);
            if !keep { gone.push(e.eid); }
            keep
        });
        for eid in gone {
            if announce { self.pending.push(Event::EffectRemoved { owner, eid, expired: false }); }
        }
        for (did, until) in active {
            let left_ms = until.saturating_sub(unix_now).saturating_mul(1000);
            let present = self.ledger.iter().any(|e| e.owner == owner && e.skill == QUEST_EFFECT_SKILL && e.did == *did);
            if present || left_ms == 0 { continue; }
            if announce {
                self.note_effect(owner, QUEST_EFFECT_SKILL, *did, left_ms, now, 0.0, false);
            } else {
                self.next_eid = self.next_eid.wrapping_add(1).max(1);
                self.ledger.push(Entry { eid: 0x1000 + self.next_eid, owner, skill: QUEST_EFFECT_SKILL, did: *did,
                    cast: now, until: now.saturating_add(left_ms), power: 0.0, pulsed: false });
            }
        }
    }

    /// Puts effect `did` of `skill` on `owner` for `duration_ms`; the same skill's earlier record
    /// of that effect leaves first. The events go out with the next tick.
    pub fn note_effect(&mut self, owner: u64, skill: u32, did: u32, duration_ms: u64, now: u64, power: f32, pulsed: bool) {
        if owner == 0 || did == 0 || duration_ms == 0 {
            return;
        }
        let mut left = Vec::new();
        self.ledger.retain(|e| {
            let same = e.owner == owner && e.skill == skill && e.did == did;
            if same {
                left.push(e.eid);
            }
            !same
        });
        for eid in left {
            self.pending.push(Event::EffectRemoved {
                owner,
                eid,
                expired: false,
            });
        }
        self.next_eid = self.next_eid.wrapping_add(1).max(1);
        let entry = Entry {
            eid: 0x1000 + self.next_eid,
            owner,
            skill,
            did,
            cast: now,
            until: now.saturating_add(duration_ms),
            power,
            pulsed,
        };
        self.pending.push(Event::EffectAdded {
            owner,
            effect: Applied {
                eid: entry.eid,
                skill,
                did,
                cast_ms: now,
                duration_ms,
                power,
                pulsed,
            },
        });
        self.ledger.push(entry);
    }

    /// What is on `owner` at `now`, for its login: each effect as the client lists it.
    pub fn applied_to(&self, owner: u64, now: u64) -> Vec<Applied> {
        self.ledger
            .iter()
            .filter(|e| e.owner == owner && e.until > now)
            .map(|e| Applied {
                eid: e.eid,
                skill: e.skill,
                did: e.did,
                cast_ms: e.cast,
                duration_ms: e.until - e.cast,
                power: e.power,
                pulsed: e.pulsed,
            })
            .collect()
    }

    /// What is on `owner` at `now`: the skill, the effect and how long is left.
    pub fn effects_on(&self, owner: u64, now: u64) -> Vec<(u32, u32, u64)> {
        self.ledger
            .iter()
            .filter(|e| e.owner == owner && e.until > now)
            .map(|e| (e.skill, e.did, e.until - now))
            .collect()
    }

    /// Effects whose time is up leave, and so do those of an owner that is dead or gone.
    pub(super) fn prune_effects(
        &mut self,
        now: u64,
        _scene: &Scene,
        store: &Store,
        events: &mut Vec<Event>,
    ) {
        let actors = &self.actors;
        self.ledger.retain(|e| {
            let owner_ok = match store.characters.get(&e.owner) {
                // A player who logged off keeps the effects, which run out in the world's time.
                // A quest's effect (a curse until its reward) outlives a death: the quest book
                // holds it (Pyrealium policy; no source says a death lifted one).
                Some(c) => !c.deleted && (c.progression.alive() || e.skill == QUEST_EFFECT_SKILL),
                None => actors
                    .get(&e.owner)
                    .is_some_and(|a| a.health > 0 && !a.evading),
            };
            let expired = e.until <= now;
            if expired || !owner_ok {
                events.push(Event::EffectRemoved {
                    owner: e.owner,
                    eid: e.eid,
                    expired: expired && owner_ok,
                });
            }
            !expired && owner_ok
        });
    }
}
