//! One encounter step: attacks, creature AI and movement, regeneration and the spawners.
use super::*;

impl Encounters {
    pub fn tick(
        &mut self,
        now: u64,
        scene: &Scene,
        store: &mut Store,
        active: impl Iterator<Item = u64>,
        mut geometry: Option<&mut crate::geometry::Geometry>,
    ) -> io::Result<Vec<Event>> {
        if now < self.next_tick {
            return Ok(std::mem::take(&mut self.pending));
        }
        // Use elapsed time rather than assuming every scheduled 100ms tick ran
        // on time. Bound catch-up after a stall so no actor leaps across a zone.
        let elapsed=if self.next_tick==0 {0.1}else{(now.saturating_sub(self.next_tick.saturating_sub(100)).min(250) as f32)/1000.};
        self.next_tick = now.saturating_add(100);
        self.ratings.retain(|(_,_,_,until)|*until>now);
        self.regen.retain(|(who,_),(_,until)|*until>now && store.characters.get(who).is_some_and(|c|c.progression.alive()));
        self.ratings.retain(|(who,_,_,_)|store.characters.get(who).map_or_else(||self.actors.get(who).is_some_and(|a|a.health>0 && !a.evading),|c|c.progression.alive()));
        self.burns.retain(|b| b.next<=b.until && now<=b.until && self.actors.get(&b.target).is_some_and(|a|a.health>0 && !a.evading) && store.characters.get(&b.source).is_some_and(|c|!c.deleted && c.progression.alive()) && scene.player_placement(b.source).is_some());
        let mut events = std::mem::take(&mut self.pending);
        self.prune_effects(now, scene, store, &mut events);
        let active=active.collect::<Vec<_>>();
        let mut due = self
            .attacks
            .iter()
            .filter(|(_, attack)| attack.next <= now)
            .map(|(&id, &attack)| (id, attack))
            .collect::<Vec<_>>();
        for burn in &mut self.burns {
            if burn.next<=now {
                burn.next=now.saturating_add(burn.period);
                due.push((burn.source,Attack{target:burn.target,next:now,repeat:false,category:0,stage:2,started:now,skill:false,periodic:true,
                    strike:Some(Strike{ratings:None,skill:burn.skill,bonus:burn.per_tick,vigor_damage:burn.vigor_per_tick,weapon_damage:false,cost:0,range:0.,behavior:0,fx:0,burn:None,effects:[(0,0);2]})}));
            }
        }
        for (player, attack) in due {
            if !attack.periodic && !self.attacks.contains_key(&player) {
                continue;
            }
            let available = self
                .actors
                .get(&attack.target)
                .is_some_and(|actor| actor.health > 0 && !actor.evading);
            let alive = store
                .characters
                .get(&player)
                .is_some_and(|c| !c.deleted && c.progression.alive());
            if !available
                || !alive
                || (!attack.periodic && !scene.combat_distance(player, attack.target).is_some_and(|distance|
                    distance <= attack.strike.map_or(if attack.category == 0 { self.range } else { 18.0 }, |s|s.range.max(self.range))))
            {
                if alive {
                    events.push(Event::Note {
                        player,
                        text: if available {
                            "Attack stopped: move closer to your target."
                        } else {
                            "Attack stopped: your target is no longer available."
                        }.into(),
                    });
                }
                if !attack.periodic { self.stopped(player, &mut events); }
                continue;
            }
            if !attack.periodic { if let Some(world)=geometry.as_deref_mut() {
                let clear=scene.player_placement(player).zip(scene.content_placement(attack.target))
                    .is_some_and(|(a,b)|world.clear(a,b,1.));
                if !clear {events.push(Event::Note{player,text:"Attack stopped: your target is obstructed.".into()});
                    self.stopped(player,&mut events);continue;}
            }
            }
            if !attack.periodic && attack.strike.is_some_and(|s|s.cost>0) && store.characters[&player].progression.vigor==0 {
                events.push(Event::Note{player,text:"Attack stopped: insufficient vigor.".into()});self.stopped(player,&mut events);continue;
            }
            if !attack.periodic && attack.category != 0 && attack.stage == 0 {
                let cost = attack.strike.map_or(if attack.category == 2 { self.cost.max(6) } else { self.cost }, |s|s.cost.min(store.characters[&player].progression.vigor));
                if cost > 0 && store.characters[&player].progression.apply(&Command::SpendVigor(cost)).is_err() {
                    events.push(Event::Note { player, text: "Attack stopped: insufficient vigor.".into() });
                    self.stopped(player, &mut events);
                    continue;
                }
                if let Some(strike)=attack.strike { events.push(Event::SkillSwing{player,behavior:strike.behavior,skill:strike.skill,target:attack.target}); } else { events.push(Event::Swing(player)); }
                let queued = self.attacks.get_mut(&player).unwrap();
                queued.started = now;
                queued.stage = 1;
                queued.next = now.saturating_add(crate::projectiles::RELEASE_MS);
                continue;
            }
            if attack.strike.is_none() && attack.category != 0 && attack.stage == 1 {
                let race = store.characters[&player].creation.as_ref().map_or(1, |r| r.race);
                if let Some(profile) = crate::projectiles::profile(race, attack.category, attack.skill) {
                    let distance = scene.combat_distance(player, attack.target).unwrap();
                    let flight_ms = crate::projectiles::flight_ms(distance);
                    events.push(Event::Projectile { player, target: attack.target, profile, flight_ms });
                    let queued = self.attacks.get_mut(&player).unwrap();
                    queued.stage = 2;
                    queued.next = now.saturating_add(flight_ms);
                    continue;
                }
            }
            // A skill whose missile the client flies (`crate::skill_missiles`) lands when the
            // missile reaches the target at the projectiles' authored speed (Pyrealium policy, as for
            // the basic attacks' projectiles above).
            if attack.stage == 1 && attack.strike.is_some_and(|s| crate::skill_missiles::missile_hook(s.skill).is_some()) {
                let distance = scene.combat_distance(player, attack.target).unwrap();
                let queued = self.attacks.get_mut(&player).unwrap();
                queued.stage = 2;
                queued.next = now.saturating_add(crate::projectiles::flight_ms(distance));
                continue;
            }
            let actor = &self.actors[&attack.target];
            // Authored starter balance; the client sends only a learned skill/target.
            let weapon=crate::professions::weapon_damage(&store.characters[&player].inventory).unwrap_or(self.damage);
            let base = if attack.category == 2 { weapon.saturating_add(4) } else { weapon };
            let damage = attack.strike.map_or(base, |s|s.bonus.saturating_add(if s.weapon_damage {base} else {0}));
            let armor=if attack.periodic {0} else {crate::combat::adjusted(actor.rule.armor,self.ratings_for(attack.target).armor)};
            let cost = attack.strike.map_or(if attack.category == 2 { self.cost.max(6) } else { self.cost }, |s|s.cost.min(store.characters[&player].progression.vigor));
            let school=match attack.category {1=>crate::combat::School::Missile,2=>crate::combat::School::Magic,_=>crate::combat::School::Melee};
            let caster=&store.characters[&player];
            let rating=crate::combat::adjusted(crate::combat::player_attack(caster.progression.level().number,&caster.skills,school),self.ratings_for(player).attack(school));
            let defense=crate::combat::adjusted(actor.rule.level.unwrap_or(1)*8,self.ratings_for(attack.target).creature_defense());
            let hit=attack.periodic || self.dice.roll(crate::combat::hit_chance(rating,defense));
            let dealt = if !hit || damage == 0 {0} else {damage
                .saturating_sub(armor)
                .max(1)
                .min(actor.health)};
            let drained = if hit { attack.strike.map_or(0, |s| s.vigor_damage).min(actor.vigor.unwrap_or(0)) } else { 0 };
            let killed = dealt == actor.health;
            let mut commands = if cost > 0 { vec![Command::SpendVigor(cost)] } else { Vec::new() };
            let shared_kill = killed && actor.rule.xp > 0 && self.shared_xp.contains(&player);
            if killed && actor.rule.xp > 0 && !shared_kill {
                commands.push(Command::AwardXp(actor.rule.xp));
            }
            if killed && actor.rule.gold > 0 {
                commands.push(Command::AwardGold(actor.rule.gold));
            }
            let character = &store.characters[&player];
            let candidate = commands
                .iter()
                .try_fold(character.progression.clone(), |state, command| {
                    state.apply(command)
                });
            if candidate.is_err() {
                events.push(Event::Note {
                    player,
                    text: "Attack stopped: insufficient vigor or reward capacity.".into(),
                });
                if !attack.periodic { self.stopped(player, &mut events); }
                continue;
            }
            let account = character.account.clone();
            let previous = character.progression.clone();
            let previous_quests = character.quests.clone();
            // A kill's quest emotes give their items as this kill's loot (`quest_emotes::kill`).
            let kill_outcome=crate::quest_emotes::kill(&character.quests,&character.inventory,&character.progression,actor.entity,0,crate::recovered_quests::now());
            let quest_drop=kill_outcome.as_ref().and_then(|o|o.given.first().copied());
            // A Kill set that pauses for a delay carries on from the server's tick.
            if killed { if let Some(outcome)=&kill_outcome { crate::quest_emotes::pending::defer_outcome(player,outcome); } }
            let loot = if killed && (quest_drop.is_some() || (self.training_loot && matches!(actor.entity,crate::shelter::DIGGER|crate::shelter::SENTINEL))) {
                character
                    .inventory
                    .free_slot()
                    .filter(|_| character.inventory.dense_bags())
                    .and_then(|slot| {
                        store
                            .next_item_id()
                            .ok()
                            .map(|id| quest_drop.and_then(|d|crate::quest_emotes::item(d,id,slot)).or_else(||matches!(actor.entity,crate::shelter::DIGGER|crate::shelter::SENTINEL).then(||crate::apparel::drop(&character.inventory,character.creation.as_ref().map_or(1,|r|r.race),id,slot)).flatten()).unwrap_or_else(||crate::item_services::potion(id, slot)))
                    })
                    .and_then(|item| {
                        scene
                            .admit_inventory_grant(player, item.id)
                            .ok()
                            .map(|instance| (item, instance))
                    })
            } else {
                None
            };
            if let Some((item, instance)) = &loot {
                store.encounter_transaction(
                    &account,
                    player,
                    actor.entity,
                    &[crate::inventory::Command::Grant(item.clone())],
                    &commands,
                )?;
                events.push(Event::Item {
                    player,
                    item: item.clone(),
                    instance: *instance,
                });
            } else if killed {
                store.encounter_reward(&account, player, actor.entity, &commands)?;
            } else if !commands.is_empty() {
                store.progression_batch(&account, player, &commands)?;
            }
            if killed && quest_drop.is_some() && loot.is_none() {
                events.push(Event::Note{player,text:"Quest item not collected: free a backpack slot and defeat another Drudge.".into()});
            }
            // Test-only (PYREALIUM_TEST_CORPSE_LOOT=1): a training kill that was paid straight into the pack
            // also leaves its body with rolled loot, so the corpse window can be tested in the
            // training area.
            let corpse_too=std::env::var("PYREALIUM_TEST_CORPSE_LOOT").as_deref()==Ok("1");
            let corpse_loot=killed && self.training_loot && (corpse_too || (quest_drop.is_none() && loot.is_none()));
            if corpse_loot {events.push(Event::Loot{actor:attack.target,player,entity:actor.entity,ai:actor.ai});}
            if killed {events.push(Event::Kill{actor:attack.target,player,entity:actor.entity});}
            if shared_kill {events.push(Event::KillXp{player,xp:actor.rule.xp});}
            if store.characters[&player].quests != previous_quests {
                events.push(Event::Quest {
                    player,
                    previous: previous_quests,
                    book: store.characters[&player].quests.clone(),
                });
            }
            if !attack.periodic {
            let until = if attack.category == 0 { now.saturating_add(self.interval) }
                else { attack.started.saturating_add(self.interval).max(now.saturating_add(100)) };
            self.cooldowns.insert(player, until);
            self.attacks.get_mut(&player).unwrap().next = until;
            self.attacks.get_mut(&player).unwrap().stage = 0;
            self.attacks.get_mut(&player).unwrap().skill = false;
                self.attacks.get_mut(&player).unwrap().strike=None;
            }
            if let Some(strike)=attack.strike {
                if !attack.periodic {
                    if strike.fx!=0 {events.push(Event::Fx{target:attack.target,fx:strike.fx});}
                    if hit { if let Some((ratings,ms))=strike.ratings {self.add_ratings(attack.target,strike.skill,ratings,ms,now)?;} }
                    if hit { for (did,ms) in strike.effects.into_iter().filter(|(did,_)|*did!=0) {self.note_effect(attack.target,strike.skill,did,ms,now,0.0,false);} }
                    if let Some(burn)=strike.burn.filter(|b|hit && (b.health>0 || b.vigor>0) && b.duration_ms>0) {
                        self.burns.retain(|b|!(b.source==player && b.target==attack.target && b.skill==strike.skill));
                        self.burns.push(Burn{source:player,target:attack.target,skill:strike.skill,per_tick:burn.health,vigor_per_tick:burn.vigor,
                            period:burn.period_ms.max(250),next:now.saturating_add(burn.period_ms.max(250)),until:now.saturating_add(burn.duration_ms)});
                    }
                }
            }
            let actor = self.actors.get_mut(&attack.target).unwrap();
            actor.health -= dealt;
            if drained > 0 {
                let vigor = actor.vigor.as_mut().unwrap();
                *vigor -= drained;
                events.push(Event::Vigor { actor: attack.target, vigor: *vigor });
            }
            if !attack.periodic && attack.category == 0 { if let Some(strike)=attack.strike {events.push(Event::SkillSwing{player,behavior:strike.behavior,skill:strike.skill,target:attack.target});} else {events.push(Event::Swing(player));} }
            if let Some(dealt) = i32::try_from(dealt).ok().filter(|value| *value > 0) {
                events.push(Event::Hit {
                    victim: attack.target,
                    attacker: player,
                    health_change: -dealt,
                });
            }
            events.push(Event::Health {
                actor: attack.target,
                health: actor.health,
            });
            events.push(Event::Owner {
                player,
                previous,
                state: store.characters[&player].progression.clone(),
            });
            events.push(Event::Note {
                player,
                text: if !hit {"Your attack missed.".into()} else if drained > 0 {format!("You hit for {dealt} damage and drain {drained} vigor.")} else if dealt == 0 {"Your skill takes effect.".into()} else {format!("You hit for {dealt} damage.")},
            });
            if killed {
                actor.target = None;
                actor.route.clear();
                events.push(Event::Note {
                    player,
                    text: if shared_kill {
                        format!("Creature defeated. Earned {} gold.", actor.rule.gold)
                    } else {
                        format!(
                            "Creature defeated. Earned {} XP and {} gold.",
                            actor.rule.xp, actor.rule.gold
                        )
                    },
                });
                // Death mode/behavior is shared by the native creature tables;
                // the receiving client selects that creature's own DAT graph.
                events.push(Event::IdolVisual(
                    attack.target,
                    crate::combat_visual::IdolAction::Death,
                ));
                let grace=crate::combat_visual::death_presentation_ms(actor.entity);
                if !self.spawners.died(attack.target,now,if corpse_loot{60000}else{grace}) {
                    return Err(invalid("defeated creature has no occupied spawner"));
                }
                let stopped = self
                    .attacks
                    .iter()
                    .filter(|(_, queued)| queued.target == attack.target)
                    .map(|(&player, _)| player)
                    .collect::<Vec<_>>();
                for player in stopped {
                    self.stopped(player, &mut events);
                }
            } else if actor.target.is_none() {
                actor.target = Some(player);
                if actor.patrol.is_some() {actor.engaged_at=scene.content_placement(attack.target);}
                actor.next_attack = now.saturating_add(actor.rule.interval);
                events.push(Event::IdolVisual(
                    attack.target,
                    crate::combat_visual::IdolAction::Combat,
                ));
            }
            if !attack.repeat && !attack.periodic {
                self.attacks.remove(&player);
            }
        }
        let mut spell_ratings=BTreeMap::<u64,crate::combat::Ratings>::new();
        for (who,_,rating,_) in &self.ratings {spell_ratings.entry(*who).or_default().add(*rating);}
        let mut casts = Vec::new();
        for (&id, actor) in &mut self.actors {
            // A real removal may also come from a script. Refill the owned slot;
            // leaving the client's PVS never removes content from the server.
            if actor.health>0 && scene.content_placement(id).is_none() {
                actor.health=0;actor.target=None;actor.route.clear();
                self.spawners.removed(id,now);
            }
            if actor.health == 0 {
                actor.velocity=0.;
                if let Some(player)=self.escorts.remove(&id) {events.push(Event::EscortLost{player,entity:actor.entity});}
                continue;
            }
            // An escorted creature follows its player at ESCORT_DISTANCE through the creature
            // motion's peace speed and the walkable ground; one left far behind (a portal, a
            // recall) comes along beside the player. It still fights what attacks it.
            if let Some(&player)=self.escorts.get(&id).filter(|_|actor.target.is_none()) {
                if let (Some(from),Some(to))=(scene.content_placement(id),scene.player_placement(player)) {
                    if from.distance(to).is_none_or(|d|d>ESCORT_WARP) {
                        // Once the player has stood ESCORT_REJOIN_MS at the far end (its client has
                        // entered the new place and can be shown the creature arriving).
                        let since=*self.escort_far.entry(id).or_insert(now);
                        if now.saturating_sub(since)>=ESCORT_REJOIN_MS {
                            self.escort_far.remove(&id);
                            if let Some(beside)=to.translated([ESCORT_DISTANCE,0.,0.]) {events.push(Event::Warp{actor:id,placement:beside});}
                        }
                    } else {
                        self.escort_far.remove(&id);
                        let budget=actor.movement.peace.distance(&mut actor.velocity,elapsed);
                        match advance(from,to,ESCORT_DISTANCE,budget,&mut actor.route,|from,to|
                            geometry.as_deref_mut().and_then(|world|world.walk_step(from,to,0.35))) {
                            Some(next)=>events.push(Event::Move{actor:id,placement:rested(next,actor.movement.rest_height)}),
                            None=>actor.velocity=0.,
                        }
                    }
                    // A follower does not walk back to its spawn along the way it came.
                    actor.route.clear();
                }
                continue;
            }
            if actor.target.is_none() && !actor.evading && now>=actor.next_idle {
                // Native detection radius, with a staggered 1s scan. Do not
                // load collision or roam distant, unobserved regions.
                actor.next_idle=now.saturating_add(1000);
                let nearby=active.iter().filter_map(|&player| {
                    let alive=store.characters.get(&player).is_some_and(|c|!c.deleted && c.progression.alive());
                    alive.then(||scene.combat_distance(player,id).map(|d|(player,d))).flatten()
                }).filter(|(_,distance)|*distance<=actor.ai.perception.max(actor.ai.wander_range+20.)
                    .max(if actor.patrol.is_some() {80.} else {0.}))
                  .collect::<Vec<_>>();
                // A patrolling creature leashes around where it currently is.
                let center=if actor.patrol.is_some() {scene.content_placement(id)} else {Some(actor.home)};
                if actor.ai.aggressive() {
                    let mut candidates=nearby.iter().filter(|(_,d)|*d<=actor.ai.perception)
                        .filter(|(p,_)|in_range(center,scene.player_placement(*p),actor.rule.leash))
                        .copied().collect::<Vec<_>>();
                    candidates.sort_by(|a,b|a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                    // A nearer player behind a wall must not mask a visible
                    // player who is also within the native perception radius.
                    let target=candidates.into_iter().find(|(player,_)|
                        scene.player_placement(*player).zip(scene.content_placement(id))
                            .is_some_and(|(a,b)|geometry.as_deref_mut().is_some_and(|world|world.clear(a,b,1.))));
                    if let Some((player,_))=target {
                        actor.target=Some(player);actor.wander_goal=None;
                        if actor.patrol.is_some() {actor.engaged_at=center;}
                        actor.next_attack=now.saturating_add(actor.rule.interval);
                        events.push(Event::IdolVisual(id,crate::combat_visual::IdolAction::Combat));
                    }
                }
                if let Some(patrol)=actor.patrol.as_ref().filter(|_|actor.target.is_none() && actor.wander_goal.is_none()
                    && actor.route.is_empty() && actor.pursuit && !nearby.is_empty() && now>=actor.patrol_wait) {
                    // Waypoints in order, then back to the spawn point; repeat.
                    let leg=actor.patrol_next%(patrol.waypoints.len()+1);
                    let [dx,dy]=patrol.waypoints.get(leg).copied().unwrap_or([0.,0.]);
                    actor.wander_goal=actor.home.translated([dx,dy,0.]);
                } else if actor.patrol.is_none() && actor.target.is_none() && actor.wander_goal.is_none() && actor.route.is_empty()
                    && actor.pursuit && actor.ai.wanders() && !nearby.is_empty()
                    && crate::creature_ai::random(&mut actor.random)<actor.ai.probability.unwrap_or(0.2) {
                    // Native range/probability; uniform disk destinations and
                    // 1s opportunities are explicitly Pyrealium scheduling policy.
                    let angle=crate::creature_ai::random(&mut actor.random)*std::f32::consts::TAU;
                    let radius=crate::creature_ai::random(&mut actor.random).sqrt()*actor.ai.wander_range.min(actor.rule.leash);
                    actor.wander_goal=actor.home.translated([angle.cos()*radius,angle.sin()*radius,0.]);
                }
            }
            let Some(mut target) = actor.target else {
                if let Some(point)=scene.content_placement(id) {
                    if let Some(goal)=actor.wander_goal {
                        if actor.route.len()<2048 {
                            let budget=actor.movement.peace.distance(&mut actor.velocity,elapsed);
                            let mut patrol_route=Vec::new();
                            let route=if actor.patrol.is_some(){&mut patrol_route}else{&mut actor.route};
                            if let Some(next)=advance(point,goal,0.2,budget,route,|from,to|
                                geometry.as_deref_mut().and_then(|world|world.walk_step(from,to,0.35))) {
                                events.push(Event::Move{actor:id,placement:rested(next,actor.movement.rest_height)});continue;
                            }
                        }
                        actor.wander_goal=None;
                        actor.velocity=0.;
                        if let Some(patrol)=&actor.patrol {
                            // Arrived, or blocked: either way continue with the next leg.
                            actor.patrol_next=(actor.patrol_next+1)%(patrol.waypoints.len()+1);
                            actor.patrol_wait=now.saturating_add(patrol.pause_ms);
                        }
                    }
                    // A blocked return waits for geometry; never teleport or
                    // walk through an obstacle to restore an authored spawn.
                    let budget=actor.movement.peace.distance(&mut actor.velocity,elapsed);
                    if let Some(next)=retreat(point,budget,&mut actor.route,|from,to|
                        geometry.as_deref_mut().and_then(|world|world.walk_step(from,to,0.35))) {
                        events.push(Event::Move{actor:id,placement:rested(next,actor.movement.rest_height)});
                    } else {actor.velocity=0.;}
                    if actor.route.is_empty(){actor.evading=false;actor.velocity=0.;actor.engaged_at=None;}
                }
                continue;
            };
            // Engagement and melee reach are separate. In particular, a ranged
            // hit across visible cells or a slope must not heal the victim on
            // the same tick merely because our bounded pursuit cannot reach it.
            let eligible = |player| {
                store
                    .characters
                    .get(&player)
                    .is_some_and(|c| !c.deleted && c.progression.alive())
                    && actor.route.len()<4096
                    && in_range(Some(actor.engaged_at.unwrap_or(actor.home)),scene.player_placement(player),actor.rule.leash)
                    && scene.combat_distance(player, id).is_some_and(|distance| distance <= actor.rule.leash)
            };
            if !eligible(target) {
                if let Some((&other, _)) = self
                    .attacks
                    .iter()
                    .find(|(player, attack)| attack.target == id && eligible(**player))
                {
                    target = other;
                    actor.target = Some(other);
                    actor.next_attack = now.saturating_add(actor.rule.interval);
                } else {
                    actor.target = None;
                    actor.evading=true;actor.wander_goal=None;actor.engaged_at=None;
                    events.push(Event::IdolVisual(
                        id,
                        crate::combat_visual::IdolAction::Peace,
                    ));
                    if actor.vigor != actor.rule.vigor {
                        actor.vigor = actor.rule.vigor;
                        if let Some(vigor) = actor.vigor { events.push(Event::Vigor { actor: id, vigor }); }
                    }
                    if actor.health != actor.rule.health {
                        actor.health = actor.rule.health;
                        events.push(Event::Health {
                            actor: id,
                            health: actor.health,
                        });
                    }
                    continue;
                }
            }
            // Apply homesickness before movement. Shared displacement rebases
            // adjacent blocks; native collision resolves the destination cell.
            if actor.pursuit {
                if let (Some(from),Some(player))=(scene.content_placement(id),scene.player_placement(target)) {
                    let budget=actor.movement.combat.distance(&mut actor.velocity,elapsed);
                    let step=advance(from,player,(actor.rule.range-0.5).max(0.5),budget,&mut actor.route,|from,to|
                        geometry.as_deref_mut().and_then(|world|world.walk_step(from,to,0.35)));
                    if let Some(next)=step {
                        events.push(Event::Move{actor:id,placement:rested(next,actor.movement.rest_height)});
                    } else {
                        actor.velocity=0.;
                        if let Some(turned)=facing(from,player).filter(|p|p.heading!=from.heading) {
                        // Targets can circle inside melee range. Turn in place
                        // without adding a fake walking step to the home route.
                        events.push(Event::Move{actor:id,placement:turned});
                        }
                    }
                }
            }
            if !scene.combat_distance(target, id).is_some_and(|distance| distance <= actor.rule.range)
                || now < actor.next_attack {
                continue;
            }
            if let Some(world)=geometry.as_deref_mut() {
                if !scene.player_placement(target).zip(scene.content_placement(id)).is_some_and(|(a,b)|world.clear(a,b,1.)){continue;}
            }
            // A creature with a harmful ability uses it in place of a swing (see `creature_skills`).
            if now >= actor.next_ability {
                if let Some(ability) = self.abilities.get(&actor.entity).and_then(|list| list.first()) {
                    actor.next_ability = now.saturating_add(creature_skills::ABILITY_COOLDOWN_MS);
                    actor.next_attack = now.saturating_add(actor.rule.interval);
                    events.push(Event::IdolVisual(id, crate::combat_visual::IdolAction::Swing));
                    casts.push((id, target, ability.clone()));
                    continue;
                }
            }
            let account = store.characters[&target].account.clone();
            let previous = store.characters[&target].progression.clone();
            // ArmorRating from Pyreal_DB_Insert_PlayerLevelData.sql. At least 1
            // damage remains so a hit is still a hit.
            let armor = previous.level().armor.saturating_add(crate::apparel::armor(&store.characters[&target].inventory,store.characters[&target].creation.as_ref().map_or(1,|r|r.race)));
            let offense=spell_ratings.get(&id).copied().unwrap_or_default();
            let protection=spell_ratings.get(&target).copied().unwrap_or_default();
            let armor=crate::combat::adjusted(armor,protection.armor);
            let attack=crate::combat::adjusted(actor.rule.level.unwrap_or(1)*8,offense.creature_attack());
            let defense=crate::combat::adjusted(crate::combat::player_defense(previous.level().number,&store.characters[&target].skills,crate::combat::School::Melee),protection.defense(crate::combat::School::Melee));
            if !self.dice.roll(crate::combat::hit_chance(attack,defense)) {
                actor.next_attack=now.saturating_add(actor.rule.interval);
                events.push(Event::IdolVisual(id,crate::combat_visual::IdolAction::Swing));
                events.push(Event::Note{player:target,text:"The creature's attack misses you.".into()});continue;
            }
            let dealt = actor.rule.damage.saturating_sub(armor).max(1);
            store.progression_command(&account, target, Command::Damage(dealt))?;
            actor.next_attack = now.saturating_add(actor.rule.interval);
            events.push(Event::IdolVisual(
                id,
                crate::combat_visual::IdolAction::Swing,
            ));
            if let Ok(damage) = i32::try_from(dealt) {
                events.push(Event::Hit {
                    victim: target,
                    attacker: id,
                    health_change: -damage,
                });
            }
            let state = store.characters[&target].progression.clone();
            events.push(Event::Note {
                player: target,
                text: format!("Creature hits you for {dealt} damage."),
            });
            if !state.alive() {
                self.attacks.remove(&target);
                events.push(Event::Stop(target));
                events.push(Event::Note {
                    player: target,
                    text: "You have fallen. Use Resurrect to recover.".into(),
                });
            }
            events.push(Event::Owner {
                player: target,
                previous,
                state,
            });
        }
        for (caster, target, ability) in casts {
            self.cast_ability(caster, target, &ability, now, &mut events)?;
        }
        self.tick_drains(now, store, &mut events)?;
        self.tick_mends(now,scene,store,&mut events)?;
        if now >= self.next_regen {
            self.next_regen = now.saturating_add(self.regen_interval);
            let busy: BTreeSet<_> = self
                .attacks
                .keys()
                .copied()
                .chain(self.actors.values().filter_map(|actor| actor.target))
                .collect();
            for player in active {
                if scene.player_placement(player).is_none() || busy.contains(&player) {
                    continue;
                }
                let Some(character) = store
                    .characters
                    .get(&player)
                    .filter(|c| !c.deleted && c.progression.alive())
                else {
                    continue;
                };
                let state = &character.progression;
                let level = state.level();
                let mut commands = Vec::new();
                let (h,v)=self.regen.iter().filter(|((who,_),_)|*who==player).fold((1f32,1f32),|(h,v),(_, (r,_))|((h*r.health).clamp(0.,100.),(v*r.vigor).clamp(0.,100.)));
                let health = (self.regen_health as f32*h).round() as u32;
                let vigor = (self.regen_vigor as f32*v).round() as u32;
                if health > 0 && state.health < level.health {
                    commands.push(Command::Heal(health));
                }
                if vigor > 0 && state.vigor < level.vigor {
                    commands.push(Command::RecoverVigor(vigor));
                }
                if commands.is_empty() {
                    continue;
                }
                let account = character.account.clone();
                let previous = character.progression.clone();
                store.progression_batch(&account, player, &commands)?;
                events.push(Event::Owner {
                    player,
                    previous,
                    state: store.characters[&player].progression.clone(),
                });
            }
        }
        for action in self.spawners.poll(now) {
            match action {
                crate::spawners::Action::Remove(id)=>events.push(Event::Despawn(id)),
                crate::spawners::Action::Spawn(id)=>events.push(Event::Respawn{actor:id,health:self.actors[&id].rule.health,vigor:self.actors[&id].rule.vigor}),
            }
        }
        Ok(events)
    }
}
