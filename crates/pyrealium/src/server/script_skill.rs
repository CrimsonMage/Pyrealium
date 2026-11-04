//! The use-skill script call (`script::Request::UseSkill`).
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn on_use_skill(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request_id: u32,
    skill: u32,
    target: u64,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if srv.store.characters[&character].skills.untraining_id()==skill {
        peer.queue_ordered_batch(vec![script::chat("This skill is being untrained.",1).map_err(other)?,script::skill_finished(request_id).map_err(other)?])?;
        return Ok(replies);
    }
    if peer.pending_skill.is_some() {
        peer.queue_ordered_batch(vec![
            script::chat("Finish or cancel your current attack before using another skill.", 1).map_err(other)?,
            script::skill_finished(request_id).map_err(other)?,
        ])?;
        return Ok(replies);
    }
    if srv.abilities
        .as_ref()
        .is_some_and(|runtime| skill!=crate::skills::LIFESTONE_RECALL && runtime.supports(skill)
            && !srv.store.characters[&character].creation.as_ref().and_then(|r|crate::appearance::body(r.race,r.sex)).is_some_and(|b|b.starters.contains(&skill)))
    {
        let result = (|| -> io::Result<()> {
            let saved = &srv.store.characters[&character];
            let aim = if [0, u64::MAX, character].contains(&target) {
                crate::abilities::Aim::Own
            } else if srv.encounters
                .as_ref()
                .is_some_and(|runtime| runtime.is_creature(target))
            {
                crate::abilities::Aim::Creature(target)
            } else if srv.store.characters.contains_key(&target)
                && srv.scene.can_observe(character, target)
            {
                let distance = srv.scene
                    .player_placement(character)
                    .zip(srv.scene.player_placement(target))
                    .map_or(f32::MAX, |(a, b)| {
                        a.distance(b).unwrap_or(f32::MAX)
                    });
                crate::abilities::Aim::Player(target, distance)
            } else {
                crate::abilities::Aim::Other
            };
            let aim = srv.abilities.as_ref().unwrap().normalize_aim(skill, aim);
            let hand = |slot| {
                saved
                    .inventory
                    .items()
                    .find(|item| item.location == crate::inventory::Location::Equipped(slot))
                    .map_or(crate::abilities::Hand::Empty, |item| {
                        match srv.inventory_metadata
                            .get(&item.definition)
                            .and_then(|entry| *entry)
                            .and_then(|metadata| metadata.implement)
                        {
                            Some(bits) => crate::abilities::Hand::Implement(bits),
                            None => crate::abilities::Hand::Unknown,
                        }
                    })
            };
            if !matches!(aim,crate::abilities::Aim::Own) {
                let from=srv.scene.player_placement(character);
                let to=srv.scene.player_placement(target).or_else(||srv.scene.content_placement(target));
                if let Some(world)=srv.bootstrap.as_mut().and_then(|b|b.geometry.as_mut()) {
                    if !from.zip(to).is_some_and(|(a,b)|world.clear(a,b,1.)) {return Err(invalid("Your target is obstructed."));}
                }
                if let crate::abilities::Aim::Player(_,_) = aim {
                    if !srv.store.characters[&target].progression.alive() {return Err(invalid("Your target must be alive."));}
                }
            }
            let cast = srv.abilities
                .as_mut()
                .unwrap()
                .prepare(
                    character,
                    skill,
                    aim,
                    (hand(0x40), hand(0x80)),
                    &saved.skills,
                    &saved.progression,
                    srv.now_ms,
                )
                .map_err(io::Error::other)?;
            // The use's world-clock second: SkillData's last-use stamp, from
            // which the client counts the skill's reset down.
            let used = srv.world_clock.seconds();
            let (story, fx) = cast.presentation();
            let instance = saved.instance;
            if let crate::abilities::Outcome::Travel(_) = cast.outcome() {
                return Err(invalid("This recall destination is not yet available."));
            }
            if let Some(runtime)=srv.encounters.as_ref() {runtime.admit_effects()?;}
            if let crate::abilities::Outcome::Support {
                target,
                health,
                vigor,
                mend,
                regen,
            } = cast.outcome().clone()
            {
                let runtime = srv.encounters
                    .as_mut()
                    .ok_or_else(|| invalid("Healing others requires combat rules on this server."))?;
                let next = cast.next().clone();
                let mut events = next.private_changes(cast.previous());
                events.push(
                    crate::ui::skill_update(skill, &saved.skills, Some(used))
                        .map_err(other)?,
                );
                let mut restored = Vec::new();
                if health > 0 {
                    restored.push(format!("{health} health"));
                }
                if vigor > 0 {
                    restored.push(format!("{vigor} vigor"));
                }
                let mut line = if restored.is_empty() {
                    "The effect continues on your target.".to_owned()
                } else {
                    format!("You restore {} on your target.", restored.join(" and "))
                };
                if mend.is_some() || regen.is_some() {
                    line.push_str(" It keeps working for a while.");
                }
                let ratings = rating_sentence(&cast);
                if !ratings.is_empty() {
                    line.push(' ');
                    line.push_str(&ratings);
                }
                events.push(script::chat(&line, 1).map_err(other)?);
                events.push(script::skill_finished(request_id).map_err(other)?);
                peer.validate_scene_batch(&events)?;
                if !cast.commands().is_empty() {
                    srv.store.progression_batch(&peer.session.account, character, cast.commands())?;
                }
                if health > 0 || vigor > 0 {
                    runtime.mend(
                        character,
                        skill,
                        target,
                        crate::encounters::Over { health, vigor, period_ms: 1000, duration_ms: 0 },
                        fx,
                        srv.now_ms,
                    )?;
                }
                if let Some(over) = mend {
                    runtime.mend(character, skill, target, over, 0, srv.now_ms)?;
                }
                if let Some(regen) = regen {
                    runtime.boost_regen(target, skill, regen, srv.now_ms)?;
                }
                store_ratings(&mut srv.encounters, &cast, srv.now_ms)?;
                store_effects(&mut srv.encounters, &cast, srv.now_ms);
                cast.commit();
                srv.store.characters.get_mut(&character).unwrap().skill_used.insert(skill, used);
                if story != 0 {
                    outbox.deliveries.extend(skill_visual(peer, &mut srv.scene, character, instance, story));
                }
                outbox.deliveries.extend(srv.scene.player_stats(character, &next));
                peer.queue_ordered_batch(events)?;
                return Ok(());
            }
            if let crate::abilities::Outcome::Strike {
                target,
                bonus,
                vigor_damage,
                weapon_damage,
                cost,
                range,
                burn,
            } = cast.outcome().clone()
            {
                let ratings = rating_sentence(&cast);
                let mut events = vec![
                    crate::ui::skill_update(
                        skill,
                        &saved.skills,
                        Some(used),
                    )
                    .map_err(other)?,
                    script::private_call(
                        script::COMBAT_STATE,
                        &script::integer(1),
                    )
                    .map_err(other)?,
                ];
                if !ratings.is_empty() {
                    events.push(script::chat(&ratings, 1).map_err(other)?);
                }
                peer.validate_scene_batch(&events)?;
                srv.encounters
                    .as_mut()
                    .ok_or_else(|| invalid("Combat rules are not enabled on this server."))?
                    .strike(
                        character,
                        target,
                        srv.now_ms,
                        &srv.scene,
                        &srv.store,
                        crate::encounters::Strike {
                            ratings: cast.other_ratings().map(|(_,r,ms)|(r,ms)),
                            skill,
                            bonus,
                            vigor_damage,
                            weapon_damage,
                            cost,
                            range,
                            behavior: story,
                            fx,
                            burn,
                            effects: cast.creature_effects(),
                        },
                    )?;
                store_ratings(&mut srv.encounters, &cast, srv.now_ms)?;
                store_effects(&mut srv.encounters, &cast, srv.now_ms);
                cast.commit();
                srv.store.characters.get_mut(&character).unwrap().skill_used.insert(skill, used);
                peer.queue_ordered_batch(events)?;
                peer.combat = true;
                peer.target = Some(target);
                // The strike's Swing or Stop finishes the skill.
                peer.pending_skill = Some(request_id);
                outbox.deliveries.extend(training_combat_visual(
                    peer, character, &srv.store, &mut srv.scene, true,
                    false, false,
                )?);
                return Ok(());
            }
            let next = cast.next().clone();
            let mut events =
                next.private_changes(cast.previous());
            events.push(
                crate::ui::skill_update(
                    skill,
                    &saved.skills,
                    Some(used),
                )
                .map_err(other)?,
            );
            let recovered_vigor = next
                .vigor
                .saturating_sub(cast.previous().vigor);
            let ratings = rating_sentence(&cast);
            let mut text = match (cast.healed(), recovered_vigor) {
                (0, 0) => String::new(),
                (0, vigor) => format!("You recover {vigor} vigor."),
                (health, 0) => format!("You recover {health} health."),
                (health, vigor) => format!("You recover {health} health and {vigor} vigor."),
            };
            if !ratings.is_empty() {
                if !text.is_empty() { text.push(' '); }
                text.push_str(&ratings);
            }
            if text.is_empty() {
                text = "The skill takes effect.".into();
            }
            events.push(script::chat(&text, 1).map_err(other)?);
            events.push(
                script::skill_finished(request_id)
                    .map_err(other)?,
            );
            peer.validate_scene_batch(&events)?;
            let healed = cast.healed();
            let (mend, regen) = match cast.outcome() {
                crate::abilities::Outcome::Restore { mend, regen, .. } => (*mend, *regen),
                _ => (None, None),
            };
            if (mend.is_some() || regen.is_some() || cast.caster_ratings().is_some() || cast.other_ratings().is_some()) && srv.encounters.is_none() {
                return Err(invalid("Lasting skill effects require combat rules on this server."));
            }
            if !cast.commands().is_empty() {
                srv.store.progression_batch(
                    &peer.session.account,
                    character,
                    cast.commands(),
                )?;
            }
            store_ratings(&mut srv.encounters, &cast, srv.now_ms)?;
                store_effects(&mut srv.encounters, &cast, srv.now_ms);
            cast.commit();
            srv.store.characters.get_mut(&character).unwrap().skill_used.insert(skill, used);
            if let Some(runtime) = srv.encounters.as_mut() {
                if let Some(over) = mend {
                    runtime.mend(character, skill, character, over, 0, srv.now_ms)?;
                }
                if let Some(regen) = regen {
                    runtime.boost_regen(character, skill, regen, srv.now_ms)?;
                }
            }
            if story != 0 {
                outbox.deliveries.extend(skill_visual(peer, &mut srv.scene, character, instance, story));
            }
            if fx != 0 {
                outbox.deliveries.extend(player_fx(peer, &mut srv.scene, character, instance, fx));
            }
            if healed > 0 {
                if let Ok(healed) = i32::try_from(healed) {
                    outbox.deliveries.extend(
                        publish_health_story(
                            peer, &mut srv.scene, character,
                            instance, character, healed,
                        ),
                    );
                }
            }
            outbox.deliveries.extend(
                srv.scene.player_stats(character, &next),
            );
            peer.queue_ordered_batch(events)?;
            Ok(())
        })(
        );
        if let Err(error) = result {
            peer.queue_ordered_batch(vec![
                script::chat(&error.to_string(), 1)
                    .map_err(other)?,
                script::skill_finished(request_id)
                    .map_err(other)?,
            ])?;
        }
        return Ok(replies);
    }
    if srv.config.initialize_ui
        && srv.starter_equipment
        && srv.store.characters[&character].creation.as_ref()
            .and_then(|r| crate::appearance::body(r.race, r.sex))
            .is_some_and(|body| body.starters.contains(&skill))
    {
        let saved = &srv.store.characters[&character];
        let attack = (|| -> io::Result<()> {
            if saved.skills.get(skill).is_none() {
                return Err(invalid(
                    "The development training attack is not learned.",
                ));
            }
            let body = saved.creation.as_ref().and_then(|r| crate::appearance::body(r.race, r.sex)).unwrap();
            let category = body.starters.iter().position(|id| *id == skill).unwrap();
            if category != crate::starter_gear::equipped_category(&saved.inventory) {
                return Err(invalid("Equip the matching melee, missile or magic weapon before using this skill."));
            }
            let events = vec![
                crate::ui::skill_update(
                    skill,
                    &saved.skills,
                    saved.skill_used.get(&skill).copied(),
                )
                .map_err(other)?,
                script::private_call(
                    script::COMBAT_STATE,
                    &script::integer(1),
                )
                .map_err(other)?,
            ];
            peer.validate_scene_batch(&events)?;
            srv.encounters
                .as_mut()
                .ok_or_else(|| invalid("Combat rules are not enabled on this server."))?
                .basic_skill(character, target, srv.now_ms, &srv.scene, &srv.store)?;
            peer.queue_ordered_batch(events)?;
            peer.combat = true;
            peer.target = Some(target);
            peer.pending_skill = Some(request_id);
            outbox.deliveries.extend(training_combat_visual(
                peer, character, &srv.store, &mut srv.scene, true,
                false, false,
            )?);
            Ok(())
        })(
        );
        if let Err(error) = attack {
            // A queued strike owns its completion until
            // the tick accepts it or cancels it.
            peer.pending_skill = None;
            peer.queue_ordered_batch(vec![
                script::chat(&error.to_string(), 1)
                    .map_err(other)?,
                script::skill_finished(request_id)
                    .map_err(other)?,
            ])?;
        }
        return Ok(replies);
    }
    if !srv.config.initialize_ui
        || !srv.config.lifestones
        || skill != crate::skills::LIFESTONE_RECALL
    {
        peer.queue_ordered_batch(vec![script::chat("This skill requires an effect handler that is not implemented yet.",1).map_err(other)?,script::skill_finished(request_id).map_err(other)?])?;
        return Ok(replies);
    }
    if srv.store.characters[&character]
        .skills
        .get(skill)
        .is_none()
    {
        peer.queue_ordered_batch(vec![script::chat("Bind to a lifestone to unlock development Recall.",1).map_err(other)?,script::skill_finished(request_id).map_err(other)?])?;
        return Ok(replies);
    }
    match recall_character(
        peer,
        character,
        &mut srv.store,
        &mut srv.scene,
        &mut srv.encounters,
        &mut srv.bootstrap,
        srv.now_ms,
    ) {
        Ok(updates) => outbox.deliveries.extend(updates),
        Err(error) => replies.extend(
            peer.send_event(
                &script::chat(&error.to_string(), 1)
                    .map_err(other)?,
                srv.now,
            )?,
        ),
    }
    peer.queue_ordered_batch(vec![
        script::skill_finished(request_id)
            .map_err(other)?,
    ])?;
    Ok(replies)
}
