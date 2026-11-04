//! Timed restorations, rating boosts and regeneration effects on players.
use super::*;

impl Encounters {
    /// Restore a player's vitals: once on the next tick when `duration_ms` is 0,
    /// otherwise every period until it expires. Recasting refreshes the effect.
    pub fn mend(
        &mut self,
        source: u64,
        skill: u32,
        player: u64,
        over: Over,
        fx: u32,
        now: u64,
    ) -> io::Result<()> {
        if over.health == 0 && over.vigor == 0 {
            return Err(invalid("empty restoration"));
        }
        let period = over.period_ms.max(250);
        if over.duration_ms > 0 {
            self.mends.retain(|m| !(m.source == source && m.skill == skill && m.player == player && !m.instant));
        }
        if self.mends.len() >= MAX_TIMED {
            return Err(invalid("timed effect capacity"));
        }
        self.mends.push(Mend {
            instant: over.duration_ms == 0,
            source,
            skill,
            player,
            health: over.health,
            vigor: over.vigor,
            period,
            next: if over.duration_ms == 0 {now} else {now.saturating_add(period)},
            until: now.saturating_add(over.duration_ms),
            fx,
        });
        Ok(())
    }

    pub fn add_ratings(&mut self, entity: u64, skill: u32, ratings: crate::combat::Ratings, duration_ms: u64, now: u64) -> io::Result<()> {
        if ratings.is_empty() || entity == 0 {
            return Ok(());
        }
        self.ratings.retain(|(who, id, _, until)| !(*who == entity && *id == skill) && *until > now);
        if self.ratings.len() >= MAX_TIMED {
            return Err(invalid("timed effect capacity"));
        }
        self.ratings.push((entity, skill, ratings, now.saturating_add(duration_ms.max(1000))));
        Ok(())
    }
    pub(super) fn ratings_for(&self, entity: u64) -> crate::combat::Ratings {
        let mut total = crate::combat::Ratings::default();
        for (who, _, ratings, _) in &self.ratings {
            if *who == entity {
                total.add(*ratings);
            }
        }
        total
    }
    /// Multiply a player's regeneration until the buff expires.
    pub fn boost_regen(&mut self, player: u64, skill:u32, regen: Regen, now: u64) -> io::Result<()> {
        if !(regen.health.is_finite() && regen.vigor.is_finite())
            || regen.health < 0.0
            || regen.vigor < 0.0
            || regen.health > 100.0
            || regen.vigor > 100.0
        {
            return Err(invalid("invalid regeneration buff"));
        }
        if self.regen.len() >= MAX_TIMED && !self.regen.contains_key(&(player,skill)) {return Err(invalid("timed effect capacity"));}
        self.regen.insert((player,skill),(regen,now.saturating_add(regen.duration_ms)));
        Ok(())
    }
    pub(super) fn tick_mends(
        &mut self,
        now: u64,
        scene: &Scene,
        store: &mut Store,
        events: &mut Vec<Event>,
    ) -> io::Result<()> {
        for mend in &mut self.mends {
            if mend.next > now || mend.next > mend.until || !mend.instant && now > mend.until {
                continue;
            }
            mend.next = now.saturating_add(mend.period);
            let Some(character) = store
                .characters
                .get(&mend.player)
                .filter(|c| !c.deleted && c.progression.alive())
            else {
                mend.until = 0;
                continue;
            };
            if scene.player_placement(mend.player).is_none() {
                continue;
            }
            if mend.fx != 0 {
                events.push(Event::Fx {
                    target: mend.player,
                    fx: std::mem::take(&mut mend.fx),
                });
            }
            let level = character.progression.level();
            let mut commands = Vec::new();
            if mend.health > 0 && character.progression.health < level.health {
                commands.push(Command::Heal(mend.health));
            }
            if mend.vigor > 0 && character.progression.vigor < level.vigor {
                commands.push(Command::RecoverVigor(mend.vigor));
            }
            if commands.is_empty() {
                continue;
            }
            let account = character.account.clone();
            let previous = character.progression.clone();
            store.progression_batch(&account, mend.player, &commands)?;
            let state = store.characters[&mend.player].progression.clone();
            let healed = state.health.saturating_sub(previous.health);
            if healed > 0 {
                if let Ok(healed) = i32::try_from(healed) {
                    events.push(Event::Hit {
                        victim: mend.player,
                        attacker: mend.source,
                        health_change: healed,
                    });
                }
            }
            events.push(Event::Owner {
                player: mend.player,
                previous,
                state,
            });
        }
        self.mends.retain(|mend| mend.next <= mend.until && (mend.instant || now <= mend.until));
        Ok(())
    }
    /// Capacity is checked before any progression persistence or queue mutation.
    pub fn admit_effects(&self) -> io::Result<()> {
        let pending = self.attacks.values().filter(|a|a.strike.is_some()).count();
        if self.burns.len()+pending+2>MAX_TIMED || self.mends.len()+2>MAX_TIMED || self.ratings.len()+pending+2>MAX_TIMED || self.regen.len()+1>MAX_TIMED {
            return Err(invalid("Timed effect capacity; try again shortly."));
        } Ok(())
    }
}
