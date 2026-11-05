//! Live world edits: creatures added, removed and re-ruled while the server runs.
use super::*;

impl Encounters {
    /// Live edit: whether a newly placed object is a creature under the current rules.
    pub fn live_is_creature(&self, object: &crate::content::Object) -> bool {
        self.bestiary.actor(object).is_some()
    }
    /// Live edit: the operator rules every later live change is built from.
    pub fn live_set_rules(&mut self, rules: std::sync::Arc<crate::world_rules::Rules>) {
        self.bestiary.world = rules;
    }
    /// Live edit: a new creature placement. Content holds it inactive; its point
    /// spawner shows it through the normal spawn path on the next poll.
    pub fn live_add(&mut self, object: &crate::content::Object, motion: Option<crate::creature_motion::Profile>, now: u64) -> io::Result<()> {
        let mut actor = self.bestiary.actor(object).ok_or_else(|| invalid("object is not a creature"))?;
        if self.native_motion && motion.is_none(){return Err(invalid("live creature is missing native movement data"));}
        if let Some(profile)=motion {actor.movement=profile;}
        actor.health=0; // Waiting for its first published spawner lifetime.
        if self.actors.len() >= 16384 { return Err(invalid("creature limit reached")); }
        self.spawners.add_point(object.id, self.bestiary.policy(object.id, &actor), now)?;
        self.actors.insert(object.id, actor);
        Ok(())
    }
    /// The stats a new creature's content object carries (as at startup).
    pub fn live_stats(&self, id: u64) -> Vec<(u32, u32)> {
        let Some(actor) = self.actors.get(&id) else { return Vec::new() };
        let rule = actor.rule;
        [(0x107, Some(rule.health)), (0x102, Some(rule.health)), (0x12e, rule.level), (0x108, rule.vigor), (0x103, rule.vigor)]
            .into_iter().filter_map(|(stat, value)| value.map(|v| (stat, v))).collect()
    }
    /// Live edit: the creature leaves the world. Attackers stop at once.
    pub fn live_remove(&mut self, id: u64) {
        if self.actors.remove(&id).is_none() { return; }
        self.spawners.remove_point(id);
        self.burns.retain(|b|b.target!=id && b.source!=id);
        self.ratings.retain(|(who,_,_,_)|*who!=id);
        self.ledger.retain(|e|e.owner!=id);
        self.pending.retain(|event|!matches!(event,Event::Stats{actor,..} if *actor==id));
        let attackers = self.attacks.iter().filter(|(_, a)| a.target == id).map(|(&p, _)| p).collect::<Vec<_>>();
        for player in attackers {
            self.attacks.remove(&player);
            self.pending.push(Event::Stop(player));
            self.pending.push(Event::Note { player, text: "Attack stopped: your target was removed by a world edit.".into() });
        }
    }
    /// Live edit: re-rule the creatures a rules change touches (`None`: all of them),
    /// then rebuild the operator spawn groups.
    pub fn live_rules(&mut self, affected: Option<&BTreeSet<crate::world_rules::Target>>) -> io::Result<()> {
        use crate::world_rules::Target;
        for (&id, actor) in &mut self.actors {
            if affected.is_some_and(|set| !set.contains(&Target::Actor(id)) && !set.contains(&Target::Entity(actor.entity))) {
                continue;
            }
            let Some((rule, ai, patrol)) = self.bestiary.ruled(actor.entity, id) else { continue };
            if rule != actor.rule {
                let full = actor.health == actor.rule.health;
                if actor.health > 0 {
                    actor.health = if full { rule.health } else { actor.health.min(rule.health) };
                }
                self.pending.push(Event::Stats { actor: id, max_health: rule.health, health: actor.health, level: rule.level });
                actor.rule = rule;
            }
            // Made peaceful: drop the fight. A creature that was already peaceful
            // keeps defending itself against whoever attacked it.
            if actor.target.is_some() && actor.ai.aggressive() && !ai.aggressive() {
                actor.target = None;
                actor.evading = true;
                actor.wander_goal = None;
                self.pending.push(Event::IdolVisual(id, crate::combat_visual::IdolAction::Peace));
            }
            actor.ai = ai;
            if patrol != actor.patrol {
                actor.patrol = patrol;
                actor.patrol_next = 0;
                actor.patrol_wait = 0;
                if actor.target.is_none() { actor.wander_goal = None; }
            }
            self.spawners.set_policy(id, self.bestiary.policy(id, actor));
        }
        self.spawners.replace_groups(OPERATOR_GROUP, self.bestiary.operator_groups(&self.actors))?;
        self.spawners.set_chances(self.bestiary.operator_chances());
        Ok(())
    }
}
