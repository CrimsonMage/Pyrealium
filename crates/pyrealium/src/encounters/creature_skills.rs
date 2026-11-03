//! Harmful skills creatures use on the players they fight.
//!
//! A creature entity record lists skills (`NPC_StartingSkill1..20` with levels); the ones whose
//! skill record puts a timed health or vigor loss, or lowered ratings, on a player are its
//! abilities (`abilities::creature_abilities`), valued at the creature's level in the skill.
//! The records give what an ability does, not when a creature casts it (the retail creature
//! spell AI is not recovered), so Pyrealium's own policy: a creature in melee reach of its target
//! uses its first ability in place of a swing, then again every `ABILITY_COOLDOWN_MS`.
use super::{Encounters, Event};
use crate::{
    abilities::CreatureAbility,
    progression::Command,
    store::Store,
};
use std::io;

/// Pyrealium policy: how long a creature waits before it uses its ability again.
pub(super) const ABILITY_COOLDOWN_MS: u64 = 45_000;

/// A health or vigor loss over time on a player.
#[derive(Clone)]
pub(super) struct Drain {
    pub source: u64,
    pub skill: u32,
    pub target: u64,
    pub health: u32,
    pub vigor: u32,
    pub period: u64,
    pub next: u64,
    pub until: u64,
}

impl Encounters {
    /// A creature uses `ability` on `target`: its effects join the player's list, its ratings
    /// change, its drain starts.
    pub(super) fn cast_ability(
        &mut self,
        caster: u64,
        target: u64,
        ability: &CreatureAbility,
        now: u64,
        events: &mut Vec<Event>,
    ) -> io::Result<()> {
        for &(did, duration_ms, pulsed) in &ability.effects {
            self.note_effect(
                target,
                ability.skill,
                did,
                duration_ms,
                now,
                f32::from(ability.level),
                pulsed,
            );
        }
        if let Some((ratings, ms)) = ability.ratings {
            self.add_ratings(target, ability.skill, ratings, ms, now)?;
        }
        if let Some(drain) = ability.drain {
            let period = drain.period_ms.max(250);
            self.drains
                .retain(|d| !(d.target == target && d.skill == ability.skill));
            self.drains.push(Drain {
                source: caster,
                skill: ability.skill,
                target,
                health: drain.health,
                vigor: drain.vigor,
                period,
                next: now.saturating_add(period),
                until: now.saturating_add(drain.duration_ms),
            });
        }
        events.push(Event::Note {
            player: target,
            text: "The creature afflicts you.".into(),
        });
        Ok(())
    }

    /// The drains that are due take their health or vigor from their players.
    pub(super) fn tick_drains(
        &mut self,
        now: u64,
        store: &mut Store,
        events: &mut Vec<Event>,
    ) -> io::Result<()> {
        let mut fallen = Vec::new();
        for drain in &mut self.drains {
            if drain.next > now || drain.next > drain.until {
                continue;
            }
            drain.next = now.saturating_add(drain.period);
            let Some(character) = store
                .characters
                .get(&drain.target)
                .filter(|c| !c.deleted && c.progression.alive())
            else {
                drain.until = 0;
                continue;
            };
            let account = character.account.clone();
            let previous = character.progression.clone();
            let mut commands = Vec::new();
            if drain.health > 0 {
                commands.push(Command::Damage(drain.health));
            }
            if drain.vigor > 0 && previous.vigor > 0 {
                commands.push(Command::SpendVigor(drain.vigor.min(previous.vigor)));
            }
            if commands.is_empty() {
                continue;
            }
            store.progression_batch(&account, drain.target, &commands)?;
            let state = store.characters[&drain.target].progression.clone();
            let lost = previous.health.saturating_sub(state.health);
            if let Some(change) = i32::try_from(lost).ok().filter(|l| *l > 0) {
                events.push(Event::Hit {
                    victim: drain.target,
                    attacker: drain.source,
                    health_change: -change,
                });
            }
            if !state.alive() {
                fallen.push(drain.target);
                events.push(Event::Note {
                    player: drain.target,
                    text: "You have fallen. Use Resurrect to recover.".into(),
                });
            }
            events.push(Event::Owner {
                player: drain.target,
                previous,
                state,
            });
        }
        for player in fallen {
            self.attacks.remove(&player);
            events.push(Event::Stop(player));
        }
        self.drains.retain(|d| d.next <= d.until);
        Ok(())
    }
}
