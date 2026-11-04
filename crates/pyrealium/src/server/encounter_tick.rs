//! The creatures' tick (`Encounters::tick`) and what its events do in the world: creature
//! presentations, hits, kills and their loot and quest credit, and the scene updates.
use super::*;

pub(super) fn encounter_tick(srv: &mut Server, peers: &mut BTreeMap<SocketAddr, Peer>) -> io::Result<()> {
    if let Some(runtime) = &mut srv.encounters {
        runtime.set_shared_xp(srv.groups.sharing_players());
        let events = runtime.tick(
            srv.now_ms,
            &srv.scene,
            &mut srv.store,
            srv.active_characters.keys().copied(),
            srv.bootstrap.as_mut().and_then(|b|b.geometry.as_mut()),
        )?;
        let mut updates = Vec::new();
        for event in events {
            match event {
                crate::encounters::Event::IdolVisual(actor, action) => {
                    match srv.scene.idol_presentation(actor, action) {
                        Ok(deliveries) => updates.extend(deliveries),
                        Err(error) => {
                            let reason = format!("Creature presentation {actor:#x}: {error}");
                            if srv.diagnosed_application_errors.len() < 32
                                && srv.diagnosed_application_errors.insert(reason.clone())
                            {
                                eprintln!("{reason}");
                            }
                        }
                    }
                }
                crate::encounters::Event::Hit {
                    victim,
                    attacker,
                    health_change,
                } => {
                    if srv.store.characters.contains_key(&victim) {
                        if let Some(peer) = srv.active_characters
                            .get(&victim)
                            .and_then(|addr| peers.get_mut(addr))
                        {
                            updates.extend(publish_health_story(
                                peer,
                                &mut srv.scene,
                                victim,
                                srv.store.characters[&victim].instance,
                                attacker,
                                health_change,
                            ));
                        }
                    } else {
                        if let Some((_,fx))=srv.store.characters.get(&attacker)
                            .filter(|c|crate::starter_gear::equipped_category(&c.inventory)==2)
                            .and_then(|c|c.creation.as_ref()).and_then(|r|crate::combat_visual::magic_effects(r.race)) {
                            updates.extend(srv.scene.content_effect(victim,fx)?);
                        }
                        match srv.scene.content_damage_story(victim, attacker, health_change) {
                            Ok(deliveries) => updates.extend(deliveries),
                            Err(error) => {
                                let reason = format!("Damage story {victim:#x}: {error}");
                                if srv.diagnosed_application_errors.len() < 32
                                    && srv.diagnosed_application_errors.insert(reason.clone())
                                {
                                    eprintln!("{reason}");
                                }
                            }
                        }
                    }
                }
                crate::encounters::Event::Move {actor,placement} => {
                    // Native PositionPack time shares the synchronized world clock;
                    // pursuit scheduling above remains on process monotonic time.
                    updates.extend(srv.scene.move_creature(actor,placement,(srv.world_clock.seconds()*1000.0) as u64)?);
                }
                crate::encounters::Event::Warp {actor,placement} => {
                    updates.extend(srv.scene.warp_creature(actor,placement,(srv.world_clock.seconds()*1000.0) as u64)?);
                }
                crate::encounters::Event::Projectile { player, target, profile, flight_ms } => {
                    // A saturated cosmetic pool must never terminate the world server.
                    match srv.projectiles.fire(&srv.scene, player, target, profile, flight_ms, srv.now_ms) {
                        Ok(deliveries) => updates.extend(deliveries),
                        Err(error) => {
                            let reason=format!("Projectile presentation: {error}");
                            if srv.diagnosed_application_errors.len()<32 && srv.diagnosed_application_errors.insert(reason.clone()) {
                                eprintln!("{reason}");
                            }
                        }
                    }
                }
                crate::encounters::Event::SkillSwing{player,behavior,skill,target} => {
                    if let Some(peer)=srv.active_characters.get(&player).and_then(|addr|peers.get_mut(addr)) {
                        if let Some(request)=peer.pending_skill.take() {
                            updates.push(crate::scene::Delivery{recipient:player,data:script::skill_finished(request).map_err(other)?.into()});
                        }
                        let instance=srv.store.characters[&player].instance;
                        if let Some(hook)=crate::skill_missiles::missile_hook(skill).filter(|_|behavior!=0) {
                            updates.extend(skill_missile_visual(peer,&mut srv.scene,player,instance,behavior,skill,target,hook));
                        }
                        else if behavior!=0 { updates.extend(skill_visual(peer,&mut srv.scene,player,instance,behavior)); }
                        else {updates.extend(training_combat_visual(peer,player,&srv.store,&mut srv.scene,true,true,true)?);}
                    }
                }
                crate::encounters::Event::Fx{target,fx} => {
                    if let Some(peer)=srv.active_characters.get(&target).and_then(|addr|peers.get_mut(addr)) {
                        updates.extend(player_fx(peer,&mut srv.scene,target,srv.store.characters[&target].instance,fx));
                    } else if let Ok(deliveries)=srv.scene.content_effect(target,fx) {updates.extend(deliveries);}
                }
                crate::encounters::Event::Swing(player) => {
                    if let Some(addr) = srv.active_characters.get(&player) {
                        if let Some(peer) = peers.get_mut(addr) {
                            let casting=peer.pending_skill.is_some();
                            if let Some(request_id) = peer.pending_skill.take() {
                                updates.push(crate::scene::Delivery {
                                    recipient: player,
                                    data: script::skill_finished(request_id)
                                        .map_err(other)?
                                        .into(),
                                });
                            }
                            match training_combat_visual(
                                peer, player, &srv.store, &mut srv.scene, true, true, casting,
                            ) {
                                Ok(deliveries) => updates.extend(deliveries),
                                Err(error) => {
                                    eprintln!(
                                        "Combat presentation failed for character {player}: {error}"
                                    );
                                    srv.failed_peers.insert(*addr);
                                }
                            }
                        }
                    }
                }
                crate::encounters::Event::Loot {actor,player,entity,ai}=>{
                    let saved=&srv.store.characters[&player];
                    let hammer=entity==crate::arwic_mines::OUTCAST && !ai.loot_proof
                        && srv.scene.examination_content(player,actor).is_some_and(|o|o.placement.cell>>16==0x7cb9 && o.placement.cell&65535>=256)
                        && saved.quests.arwic.can_loot(&saved.inventory);
                    let quest_item=if hammer {Some(crate::arwic_mines::item(crate::arwic_mines::HAMMER,1,0,1)?)}else{None};
                    let mut count=srv.corpse_loot.add_with_quest(actor,player,srv.now_ms,entity,ai,quest_item);
                    if count==0 && std::env::var("PYREALIUM_TEST_CORPSE_LOOT").as_deref()==Ok("1") {count=srv.corpse_loot.add_test_potions(actor,player,srv.now_ms,2);}
                    if count>0 {updates.push(crate::scene::Delivery{recipient:player,data:script::chat("Loot remains on the defeated creature. Double-click its body to open it.",1).map_err(other)?.into()});}
                }
                crate::encounters::Event::Item {
                    player,
                    item,
                    instance,
                } => {
                    let metadata = srv.inventory_metadata
                        .get(&item.definition)
                        .and_then(|entry| *entry)
                        .ok_or_else(|| {
                            invalid("Encounter item metadata was not initialized")
                        })?;
                    let spawn =
                        world::create_inventory_item(&item, player, instance).map_err(other)?;
                    let acquired = crate::item_services::acquired(player, &item, metadata)
                        .map_err(other)?;
                    srv.scene
                        .inventory_granted(player, item.id, instance)
                        .map_err(other)?;
                    updates.push(crate::scene::Delivery {
                        recipient: player,
                        data: spawn.into(),
                    });
                    updates.push(crate::scene::Delivery {
                        recipient: player,
                        data: acquired.into(),
                    });
                    updates.push(crate::scene::Delivery {
                        recipient: player,
                        data: script::chat(
                            match item.definition {crate::shelter::TOOL=>"Collected Imbued Digging Tool: return to the Shelter Scout.",crate::shelter::GLYPH=>"Collected Menhir Glyph: target the Menhir Portal and use the glyph.",crate::item_services::POTION_DEFINITION=>"Training reward: a health potion was placed in your bag.",_=>"Collected equipment: open your backpack to inspect and equip it."},
                            1,
                        )
                        .map_err(other)?
                        .into(),
                    });
                }
                crate::encounters::Event::EscortLost { player, entity } => {
                    // The escort falls in a fight, so this is not an item service (those wait for
                    // a player out of combat): only quest progress, lines and tells, as a
                    // fellow's kill credit is applied.
                    let unix = crate::recovered_quests::now();
                    if let Some((tables, c)) = crate::quest_emotes::tables().zip(srv.store.characters.get(&player).filter(|c| !c.deleted)) {
                        if let Some(outcome) = tables.run(crate::quest_emotes::Trigger::EscortLost, entity, 0, None, &c.quests, &c.inventory,
                            &c.progression, 0, 0., unix, &crate::quest_emotes::item).ok().flatten()
                            .filter(|o| o.items.is_empty() && o.changes.is_empty())
                        {
                            let previous = c.quests.clone();
                            let account = c.account.clone();
                            if srv.store.emote_transaction(&account, player, &outcome).is_ok() {
                                for line in &outcome.lines {
                                    updates.push(crate::scene::Delivery { recipient: player, data: script::chat(line, 1).map_err(other)?.into() });
                                }
                                for (table, key) in &outcome.tells {
                                    updates.push(crate::scene::Delivery { recipient: player, data: crate::quest_emotes::tell(*table, *key).map_err(other)?.into() });
                                }
                                for event in srv.store.characters[&player].quests.updates_since(&previous).map_err(other)? {
                                    updates.push(crate::scene::Delivery { recipient: player, data: event.into() });
                                }
                            }
                        }
                    }
                }
                crate::encounters::Event::Kill { actor, player, entity } => {
                    // The corpse's examination names its killer ("Killed by").
                    srv.corpse_loot.killed(actor, player);
                    // A fellowship quest's kill counts for the killer's fellows in the world
                    // (registry credit `fellowship`; Pyrealium policy: no range limit is recovered).
                    let unix = crate::recovered_quests::now();
                    for fellow in srv.groups.members(player).into_iter().filter(|f| *f != player && srv.active_characters.contains_key(f)) {
                        let Some(c) = srv.store.characters.get(&fellow).filter(|c| !c.deleted && c.progression.alive()) else { continue };
                        let Some(outcome) = crate::quest_emotes::fellow_kill(&c.quests, &c.inventory, &c.progression, entity, unix) else { continue };
                        // Only quest progress is shared: items and experience stay the killer's.
                        if !outcome.items.is_empty() || !outcome.changes.is_empty() { continue; }
                        let previous = c.quests.clone();
                        let account = c.account.clone();
                        if srv.store.emote_transaction(&account, fellow, &outcome).is_err() { continue; }
                        for (table, key) in &outcome.tells {
                            updates.push(crate::scene::Delivery { recipient: fellow, data: crate::quest_emotes::tell(*table, *key).map_err(other)?.into() });
                        }
                        for event in srv.store.characters[&fellow].quests.updates_since(&previous).map_err(other)? {
                            updates.push(crate::scene::Delivery { recipient: fellow, data: event.into() });
                        }
                    }
                }
                crate::encounters::Event::KillXp { player, xp } => {
                    updates.extend(share_kill_xp(player, xp, &srv.groups, &mut srv.store, &mut srv.scene, &srv.active_characters, srv.config.initialize_ui)?);
                }
                crate::encounters::Event::Quest { player, previous, book } => {
                    for event in book.updates_since(&previous).map_err(other)? {
                        updates.push(crate::scene::Delivery {recipient:player,data:event.into()});
                    }
                    if book.ready() {
                        updates.push(crate::scene::Delivery {recipient:player,data:script::chat(&book.guidance(),1).map_err(other)?.into()});
                    }
                }
                crate::encounters::Event::Vigor { actor, vigor } => {
                    updates.extend(srv.scene.content_stat(actor, 0x103, vigor)?);
                }
                crate::encounters::Event::Health { actor, health } => {
                    updates.extend(srv.scene.content_stat(actor, 0x102, health)?);
                    if health==0 {
                        // The body is the creature at zero health. A tool is used on a target
                        // whose `Usage_TargetType` (int 721) shares a bit with the tool's
                        // `Usage_ValidTargetTypes` (722; the hunting knives' DAT value is 8,
                        // `SkillValidTargets::MonsterItem`), so the body carries that bit and a
                        // double-clicked knife can be used on it by a click.
                        updates.extend(srv.scene.content_stat(actor, crate::butchery::TARGET_TYPE_QUALITY, crate::butchery::BUTCHERABLE_TARGET)?);
                        updates.extend(srv.scene.corpse_name(actor,&srv.examination_names)?);
                    }
                    for (&recipient, addr) in &srv.active_characters {
                        if peers
                            .get(addr)
                            .is_some_and(|peer| peer.target == Some(actor))
                            && srv.scene.can_observe(recipient, actor)
                        {
                            updates.push(crate::scene::Delivery {
                                recipient,
                                data: selection_status(recipient, actor, &srv.scene, &srv.store)?
                                    .into(),
                            });
                        }
                    }
                }
                crate::encounters::Event::Stats { actor, max_health, health, level } => {
                    updates.extend(srv.scene.content_stat(actor, 0x107, max_health)?);
                    updates.extend(srv.scene.content_stat(actor, 0x102, health)?);
                    updates.extend(srv.scene.content_stat(actor, 0x12e, level.unwrap_or(0))?);
                }
                crate::encounters::Event::Despawn(actor) => {
                    if srv.scene.content_placement(actor).is_some(){updates.extend(srv.scene.despawn_content(actor)?);}
                    runtime.removed(actor,srv.now_ms);
                }
                crate::encounters::Event::Respawn { actor, health, vigor } => {
                    srv.corpse_loot.remove(actor);
                    trade_actions::respawned(actor);
                    srv.scene.content_stat(actor, 0x102, health)?;
                    if let Some(max_vigor) = vigor {
                        srv.scene.content_stat(actor, 0x103, max_vigor)?;
                    }
                    updates.extend(srv.scene.respawn_content(actor)?);
                    runtime.spawned(actor)?;
                }
                crate::encounters::Event::Owner {
                    player,
                    previous,
                    state,
                } => {
                    updates.extend(srv.scene.player_stats(player, &state));
                    let saved = &srv.store.characters[&player];
                    let skills = &saved.skills;
                    if srv.config.initialize_ui
                        && skills.available_credits(&previous)
                            != skills.available_credits(&state)
                    {
                        updates.push(crate::scene::Delivery {
                            recipient: player,
                            data: crate::ui::skills_state(skills, &state, &saved.skill_used)
                                .map_err(other)?
                                .into(),
                        });
                    }
                    for wire in state.private_changes(&previous) {
                        updates.push(crate::scene::Delivery {
                            recipient: player,
                            data: wire.into(),
                        });
                    }
                    if previous.alive() && !state.alive() {
                        for wire in [
                            crate::ui::death_notice("You were defeated by a creature.")
                                .map_err(other)?,
                            crate::ui::lock_player(true).map_err(other)?,
                        ] {
                            updates.push(crate::scene::Delivery {
                                recipient: player,
                                data: wire.into(),
                            });
                        }
                        if let Some(peer) = srv.active_characters.get(&player).and_then(|addr| peers.get_mut(addr)) {
                            updates.extend(player_body_alive(peer, &mut srv.scene, player, srv.store.characters[&player].instance, false));
                        }
                    }
                }
                crate::encounters::Event::Note { player, text } => {
                    updates.push(crate::scene::Delivery {
                        recipient: player,
                        data: script::chat(&text, 1).map_err(other)?.into(),
                    })
                }
                crate::encounters::Event::EffectAdded { owner, effect } => {
                    // Only players have a list on their client; a creature's is shown in its
                    // examination.
                    if srv.store.characters.contains_key(&owner) {
                        let cast = srv.world_clock.seconds() - srv.now_ms.saturating_sub(effect.cast_ms) as f64 / 1000.0;
                        updates.push(crate::scene::Delivery {
                            recipient: owner,
                            data: crate::ui::effect_added(&crate::ui::EffectView {
                                eid: effect.eid,
                                did: effect.did,
                                time_cast: cast,
                                duration: effect.duration_ms as f32 / 1000.0,
                                power: effect.power,
                                pulsed: effect.pulsed,
                            })
                            .map_err(other)?
                            .into(),
                        });
                    }
                }
                crate::encounters::Event::EffectRemoved { owner, eid, expired: _ } => {
                    if srv.store.characters.contains_key(&owner) && srv.active_characters.contains_key(&owner) {
                        updates.push(crate::scene::Delivery {
                            recipient: owner,
                            data: crate::ui::effect_removed(eid).map_err(other)?.into(),
                        });
                    }
                }
                crate::encounters::Event::Stop(player) => {
                    if let Some(peer) = srv.active_characters
                        .get(&player)
                        .and_then(|addr| peers.get_mut(addr))
                    {
                        if !srv.store.characters[&player].progression.alive(){peer.combat=false;}
                        if let Some(request_id) = peer.pending_skill.take() {
                            updates.push(crate::scene::Delivery {
                                recipient: player,
                                data: script::skill_finished(request_id).map_err(other)?.into(),
                            });
                        }
                        match training_combat_visual(
                            peer, player, &srv.store, &mut srv.scene, peer.combat, false, false,
                        ) {
                            Ok(deliveries) => updates.extend(deliveries),
                            Err(_) => {
                                if let Some(addr) = srv.active_characters.get(&player) {
                                    srv.failed_peers.insert(*addr);
                                }
                            }
                        }
                    }
                    let combat=srv.active_characters.get(&player).and_then(|addr|peers.get(addr)).is_some_and(|p|p.combat);
                    updates.push(crate::scene::Delivery {recipient:player,
                        data:script::private_call(script::COMBAT_STATE,&script::integer(u32::from(combat))).map_err(other)?.into()});
                }
            }
        }
        deliver_scene(
            updates,
            &srv.active_characters,
            &mut *peers,
            &srv.primary,
            srv.now,
            &mut srv.failed_peers,
        )?;
    }
    Ok(())
}
