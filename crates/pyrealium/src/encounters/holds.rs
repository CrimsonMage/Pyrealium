//! Timed holds (`quest_emotes` StartHold): the player must keep a quest's spawn-slot creature
//! alive and stay within HOLD_LEASH of it until the hold's time runs out. Kept, its HoldDone
//! sets run; lost (the creature died, or the player died or went past the leash), its HoldLost
//! sets. The radius is Pyrealium policy: no source gives one.
use super::*;

const HOLD_LEASH: f32 = 40.0;

#[derive(Clone, Debug)]
pub(super) struct Hold { player: u64, actor: u64, until: u64, live: bool }

impl Encounters {
    /// Starts a timed hold on a summoned placement (the quest's Generate brings it up); false
    /// when it is not one.
    pub fn start_hold(&mut self, player: u64, actor: u64, seconds: u32, now: u64) -> bool {
        let Some(live) = self.actors.get(&actor).map(|a| a.health > 0) else { return false };
        // A creature already up is watched at once; one the Generate brings up, once it is.
        self.holds.retain(|h| h.actor != actor);
        self.holds.push(Hold { player, actor, until: now.saturating_add(u64::from(seconds) * 1000), live });
        true
    }

    /// Each tick: a hold whose creature has come up is watched; one that ends is reported.
    pub(super) fn hold_tick(&mut self, now: u64, scene: &Scene, store: &Store, events: &mut Vec<Event>) {
        let actors = &self.actors;
        self.holds.retain_mut(|hold| {
            let Some(actor) = actors.get(&hold.actor) else { return false };
            if !hold.live && actor.health > 0 { hold.live = true; }
            let lost = if hold.live {
                let alive = store.characters.get(&hold.player).is_some_and(|c| !c.deleted && c.progression.alive());
                let near = in_range(scene.content_placement(hold.actor), scene.player_placement(hold.player), HOLD_LEASH);
                actor.health == 0 || !alive || !near
            } else {
                // Its creature never came up in the hold's time.
                now >= hold.until
            };
            if lost || now >= hold.until {
                events.push(Event::Hold { player: hold.player, actor: hold.actor, entity: actor.entity, done: !lost });
                return false;
            }
            true
        });
    }
}
