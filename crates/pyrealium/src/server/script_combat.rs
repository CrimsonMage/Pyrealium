//! Combat script calls: skill training actions, targeting, combat mode and attacks.
use super::*;

pub(super) fn on_skill(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    action: crate::skills::Action,
    skill: u32,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if !srv.config.initialize_ui {
        replies.extend(peer.send_event(&script::chat("Skills are not available in this session.",1).map_err(other)?,srv.now)?);
        return Ok(replies);
    }
    if action == crate::skills::Action::Raise {
        if peer.last_inventory_ms.is_some_and(|last| {
            srv.now_ms.saturating_sub(last) < 100
        }) {
            return Err(invalid(
                "skill advancement cooldown",
            ));
        }
        let saved = &srv.store.characters[&character];
        let race = saved
            .creation
            .as_ref()
            .map_or(1, |request| request.race);
        match saved.skills.advance(
            skill,
            &saved.progression,
            race,
        ) {
            Ok((skills, progression)) => {
                let mut events = progression
                    .private_updates()
                    .into_iter()
                    .filter(|wire| {
                        wire[..4]
                            == 0x100d4u32.to_le_bytes()
                    })
                    .collect::<Vec<_>>();
                events.push(
                    crate::ui::skill_update(
                        skill,
                        &skills,
                        saved.skill_used.get(&skill).copied(),
                    )
                    .map_err(other)?,
                );
                peer.validate_scene_batch(&events)?;
                srv.store.raise_skill(
                    &peer.session.account,
                    character,
                    skill,
                )?;
                peer.last_inventory_ms = Some(srv.now_ms);
                peer.queue_ordered_batch(events)?;
            }
            Err(error) => replies.extend(
                peer.send_event(
                    &script::chat(error.message(), 1)
                        .map_err(other)?,
                    srv.now,
                )?,
            ),
        }
        return Ok(replies);
    }
    if srv.store.debit_untraining(character)? {
        for wire in srv.store.characters[&character].progression.private_updates() {
            if wire[..4]==0x100d4u32.to_le_bytes(){outbox.deliveries.push(crate::scene::Delivery{recipient:character,data:wire.into()});}
        }
        let saved=&srv.store.characters[&character];
        outbox.deliveries.push(crate::scene::Delivery{recipient:character,
            data:crate::ui::skills_state(&saved.skills,&saved.progression,&saved.skill_used).map_err(other)?.into()});
    }
    let command = match action {
        crate::skills::Action::Train => {
            crate::skills::Command::Train(skill)
        }
        crate::skills::Action::Untrain => {
            crate::skills::Command::Untrain(skill)
        }
        crate::skills::Action::CancelUntrain => crate::skills::Command::CancelUntrain,
        crate::skills::Action::Raise => {
            return Err(invalid(
                "advancement dispatch invariant",
            ));
        }
    };
    let saved = &srv.store.characters[&character];
    let race = saved
        .creation
        .as_ref()
        .map_or(1, |request| request.race);
    if let Err(error) = saved.skills.apply(
        command,
        &saved.progression,
        race,
    ) {
        replies.extend(
            peer.send_event(
                &script::chat(error.message(), 1)
                    .map_err(other)?,
                srv.now,
            )?,
        );
        return Ok(replies);
    }
    srv.store.skill_command(
        &peer.session.account,
        character,
        command,
    )?;
    let saved = &srv.store.characters[&character];
    let wire = crate::ui::skills_state(&saved.skills, &saved.progression, &saved.skill_used).map_err(other)?;
    outbox.deliveries.push(crate::scene::Delivery {
        recipient: character,
        data: wire.into(),
    });
    Ok(replies)
}

pub(super) fn on_target(
    srv: &mut Server,
    peer: &mut Peer,
    character: u64,
    target: Option<u64>,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if target.is_some_and(|target| {
        !srv.scene.can_observe(character, target)
    }) {
        return Err(invalid("target is not visible"));
    }
    let packet = match target {
        Some(target) => selection_status(
            character, target, &srv.scene, &srv.store,
        )?,
        None => crate::ui::selection(u64::MAX, None)
            .map_err(other)?,
    };
    peer.queue_ordered_batch(vec![packet])?;
    peer.target = target;
    if peer.combat {
        if let Some(target) = target.filter(|target| {
            srv.encounters
                .as_ref()
                .is_some_and(|runtime| runtime.is_creature(*target))
        }) {
            if let Err(error) = srv.encounters.as_mut().unwrap().start(
                character,
                target,
                srv.now_ms,
                &srv.scene,
                &srv.store,
            ) {
                replies.extend(peer.send_event(
                    &script::chat(&error.to_string(), 1)
                        .map_err(other)?,
                    srv.now,
                )?);
            }
        } else {
            if let Some(runtime) = srv.encounters.as_mut() {
                runtime.stop(character);
            }
        }
    }
    Ok(replies)
}

pub(super) fn on_combat(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: script::Request,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    // A delayed stop for an old target must not
    // cancel a newly selected/current attack.
    if let script::Request::StopAttack(target) = &request {
        let current = srv.encounters
            .as_ref()
            .and_then(|runtime| {
                runtime.attacking_target(character)
            })
            .or(peer.target);
        if current != Some(*target) {
            return Ok(replies);
        }
    }
    if let Some(request_id) = peer.pending_skill.take() {
        peer.queue_ordered_batch(vec![
            script::skill_finished(request_id)
                .map_err(other)?,
        ])?;
    }
    let enabled = match request {
        script::Request::StopAttack(_) => {
            if let Some(runtime) = &mut srv.encounters {
                runtime.stop(character);
            }
            false
        }
        script::Request::Combat(enabled) => {
            let enabled = enabled;
            if enabled
                && !srv.store.characters[&character]
                    .progression
                    .alive()
            {
                replies.extend(
                    peer.send_event(
                        &script::chat(
                            "You cannot fight while dead.",
                            1,
                        )
                        .map_err(other)?,
                        srv.now,
                    )?,
                );
                return Ok(replies);
            }
            if !enabled {
                if let Some(runtime) = &mut srv.encounters {
                    runtime.stop(character);
                }
            } else if let Some(target) = peer.target.filter(|target| {
                srv.encounters.as_ref().is_some_and(|runtime| {
                    runtime.is_creature(*target)
                })
            }) {
                if let Err(error) = srv.encounters.as_mut().unwrap().start(
                    character, target, srv.now_ms, &srv.scene, &srv.store,
                ) {
                    // Stance and selection survive an interrupted attack.
                    srv.encounters.as_mut().unwrap().stop(character);
                    replies.extend(peer.send_event(
                        &script::chat(&error.to_string(), 1)
                            .map_err(other)?,
                        srv.now,
                    )?);
                }
            } else if srv.encounters.is_some() {
                replies.extend(peer.send_event(
                    &script::chat(
                        "Combat stance. Select a creature and move within your weapon's reach.",
                        1,
                    )
                    .map_err(other)?,
                    srv.now,
                )?);
            }
            enabled
        }
        script::Request::Attack(target) => {
            if target.is_some_and(|target| {
                target == character
                    || !srv.scene.can_observe(character, target)
            }) {
                return Err(invalid(
                    "attack target is not a visible other object",
                ));
            }
            if let Some(target) = target {
                let started=srv.encounters.as_mut().ok_or_else(||invalid("Combat rules are not enabled on this server."))
                    .and_then(|runtime|runtime.start(character,target,srv.now_ms,&srv.scene,&srv.store));
                if let Err(error) = started {
                    if let Some(runtime) = &mut srv.encounters {
                        runtime.stop(character);
                    }
                    peer.target = Some(target);
                    replies.extend(
                        peer.send_event(
                            &script::chat(
                                &error.to_string(),
                                1,
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    );
                    replies.extend(
                        peer.send_event(
                            &script::private_call(
                                script::COMBAT_STATE,
                                &script::integer(
                                    u32::from(peer.combat),
                                ),
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    );
                    // A rejected target stops attacking. Publish the
                    // matching pose as well as the private HUD state,
                    // including to observers and late scene snapshots.
                    if srv.config.initialize_ui {
                        outbox.deliveries.extend(training_combat_visual(
                            peer, character, &srv.store, &mut srv.scene,
                            peer.combat, false, false,
                        )?);
                    }
                    return Ok(replies);
                }
            } else if let Some(runtime) = &mut srv.encounters {
                runtime.stop(character);
            }
            peer.target = target;
            true
        }
        _ => unreachable!(),
    };
    let response = script::private_call(
        script::COMBAT_STATE,
        &script::integer(u32::from(enabled)),
    )
    .map_err(other)?;
    replies.extend(peer.send_event(&response, srv.now)?);
    peer.combat = enabled;
    if srv.config.initialize_ui {
        outbox.deliveries.extend(training_combat_visual(
            peer, character, &srv.store, &mut srv.scene, enabled,
            false, false,
        )?);
    }
    Ok(replies)
}
