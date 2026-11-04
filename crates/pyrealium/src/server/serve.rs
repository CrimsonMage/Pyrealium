//! The server loop: receives datagrams, runs the world's timed steps, and stops on
//! `stop.request` or `run_seconds`.
use super::*;

pub(super) fn serve(mut srv: Server, mut peers: BTreeMap<SocketAddr, Peer>, mut buffer: [u8; 65536]) -> io::Result<()> {
    loop {
        srv.now = Instant::now();
        srv.now_ms = srv.start.elapsed().as_millis() as u64;
        if srv.config
            .run_seconds
            .is_some_and(|seconds| srv.start.elapsed() >= Duration::from_secs(seconds))
        {
            break;
        }
        if srv.now_ms >= srv.next_stop_check {
            let stop = srv.config.data.join("stop.request");
            if stop.exists() {
                std::fs::remove_file(stop)?;
                break;
            }
            srv.next_stop_check = srv.now_ms + 250;
        }
        for port_index in 0..2 {
            for _ in 0..64 {
                let (len, addr) = match [&srv.primary, &srv.secondary][port_index].recv_from(&mut buffer) {
                    Ok(value) => value,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::ConnectionReset | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                srv.counters.received += 1;
                if (srv.auth_data.is_none() && !addr.ip().is_loopback()) || len > 4096 {
                    srv.counters.rejected += 1;
                    continue;
                }
                let bytes = &buffer[..len];
                let h = match Header::decode(bytes) {
                    Ok(h) => h,
                    Err(_) => {
                        srv.counters.rejected += 1;
                        continue;
                    }
                };
                // Endpoint-dependent NAT may map the same client socket to a
                // different source port when it contacts our secondary port.
                // Only a valid current cookie + net ID + iteration on the same
                // IP can map this ACK back to its original login endpoint.
                let addr = if h.flags == wire::CONNECT_ACK
                    && port_index == 1
                    && !peers.contains_key(&addr)
                {
                    let cookie = wire::decode_handshake(bytes)
                        .ok()
                        .and_then(|(_, payload)| wire::decode_connect_ack(payload).ok());
                    cookie
                        .and_then(|cookie| {
                            peers.iter().find_map(|(original, peer)| {
                                if original.ip() != addr.ip()
                                    || peer.id != h.recipient
                                    || peer.iteration != h.iteration
                                {
                                    return None;
                                }
                                let mut candidate = peer.session.clone();
                                candidate
                                    .connect_ack(cookie, srv.now_ms)
                                    .ok()
                                    .map(|()| *original)
                            })
                        })
                        .unwrap_or(addr)
                } else {
                    addr
                };
                if srv.failed_peers.contains(&addr) {
                    continue;
                }
                datagram(&mut srv, &mut peers, port_index, addr, &h, bytes, len)?;
            }
        }
        if let Some(admin) = &srv.admin {
            admin.players.store(srv.active_characters.len(), std::sync::atomic::Ordering::Relaxed);
            // Live edits arrive prepared; applying one costs microseconds per object.
            while let Ok(job) = admin.jobs.try_recv() {
                let started = Instant::now();
                let reply = match crate::live_edit::apply(&job.change, &mut srv.scene, srv.encounters.as_mut(), &mut srv.corpse_loot, srv.now_ms) {
                    Ok((deliveries, mut summary)) => {
                        summary.apply_us = started.elapsed().as_micros() as u64;
                        deliver_scene(deliveries, &srv.active_characters, &mut peers, &srv.primary, srv.now, &mut srv.failed_peers)?;
                        println!("Live edit: +{} -{} ~{} objects, {} rule targets, applied in {} µs.",
                            summary.added, summary.removed, summary.replaced, summary.rules, summary.apply_us);
                        Ok(summary)
                    }
                    Err(error) => Err(error.to_string()),
                };
                let _ = job.reply.send(reply);
            }
        }
        if srv.now_ms>=srv.next_resource_cleanup{
            srv.next_resource_cleanup=srv.now_ms+10_000;let time=crate::recovered_quests::now();
            let live:std::collections::BTreeSet<_>=srv.store.characters.values().filter(|c|!c.deleted).flat_map(|c|c.professions.1.jobs.iter()).filter(|(_,j)|j.point.is_some()&&j.expires>time&&j.remaining>0).map(|(id,_)|crate::harvesting::node_id(*id)).collect();
            let expired:Vec<_>=srv.scene.content_objects().filter(|o|o.id>>48==0x8190&&!live.contains(&o.id)).map(|o|o.id).collect();
            for id in expired{let updates=srv.scene.remove_content_live(id)?;deliver_scene(updates,&srv.active_characters,&mut peers,&srv.primary,srv.now,&mut srv.failed_peers)?;}
        }
        let chest_updates=srv.scene.expire_chest_views(srv.now_ms,|id|srv.store.characters.get(&id).is_some_and(|c|!c.deleted && c.progression.alive()))?;
        deliver_scene(chest_updates,&srv.active_characters,&mut peers,&srv.primary,srv.now,&mut srv.failed_peers)?;
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
                &mut peers,
                &srv.primary,
                srv.now,
                &mut srv.failed_peers,
            )?;
        }
        deliver_scene(srv.projectiles.retire(srv.now_ms), &srv.active_characters, &mut peers, &srv.primary, srv.now, &mut srv.failed_peers)?;
        if srv.now_ms >= srv.next_untraining_tick {
            srv.next_untraining_tick = srv.now_ms + 250;
            let mut updates=Vec::new();
            for &character in srv.active_characters.keys() {
                if srv.store.debit_untraining(character)? {
                    let saved=&srv.store.characters[&character];
                    updates.push(crate::scene::Delivery {recipient:character,
                        data:crate::ui::skills_state(&saved.skills,&saved.progression,&saved.skill_used).map_err(other)?.into()});
                    for wire in saved.progression.private_updates() {
                        if wire[..4]==0x100d4u32.to_le_bytes() {
                            updates.push(crate::scene::Delivery {recipient:character,data:wire.into()});
                        }
                    }
                }
            }
            deliver_scene(updates,&srv.active_characters,&mut peers,&srv.primary,srv.now,&mut srv.failed_peers)?;
        }
        let mut remove = std::mem::take(&mut srv.failed_peers);
        for (addr, peer) in &mut peers {
            if peer.native_shop.is_some() {
                let valid = match peer.session.phase {
                    Phase::InWorld { character } => {
                        srv.store
                            .characters
                            .get(&character)
                            .is_some_and(|saved| saved.progression.alive())
                            && !peer.combat
                            && peer
                                .merchant_open_ms
                                .is_some_and(|last| srv.now_ms.saturating_sub(last) <= 30_000)
                            && merchant_ready(character, &srv.scene, peer.native_shop.as_ref().map(|shop| shop.merchant())).ok()
                                == peer.native_shop.as_ref().map(|shop| shop.merchant())
                    }
                    _ => false,
                };
                if !valid {
                    let merchant = peer.native_shop.take().unwrap().merchant();
                    peer.merchant_open_ms = None;
                    if peer
                        .queue_ordered_batch(
                            vec![crate::merchants::close(merchant).map_err(other)?],
                        )
                        .is_err()
                    {
                        remove.insert(*addr);
                    }
                }
            }
            if peer.session.expired(srv.now_ms) {
                remove.insert(*addr);
                continue;
            }
            if peer.session.phase != Phase::Connecting && srv.now_ms.saturating_sub(peer.last_time_sync_ms) >= 5000 &&
                peer.transport.send_available(8).map_err(other)? {
                srv.primary.send_to(&peer.transport.send_time_sync(srv.world_clock.seconds(), srv.now).map_err(other)?, addr)?;
                peer.last_time_sync_ms = srv.now_ms;
            }
            match peer.transport.retransmit_due(srv.now) {
                Ok(packets) => {
                    for packet in packets {
                        srv.primary.send_to(&packet, addr)?;
                    }
                }
                Err(_) => {
                    remove.insert(*addr);
                }
            }
            if !remove.contains(addr) && peer.flush_scene(&srv.primary, addr, srv.now).is_err() {
                remove.insert(*addr);
            }
        }
        let mut departures = Vec::new();
        for addr in remove {
            if let Some(peer) = peers.remove(&addr) {
                if let Phase::Entering { character } | Phase::InWorld { character } =
                    peer.session.phase
                {
                    if srv.active_characters.get(&character) == Some(&addr) {
                        srv.active_characters.remove(&character);
                        if let Some(runtime) = &mut srv.encounters {
                            runtime.stop(character);
                        }
                        departures.extend(srv.scene.remove(character));
                    }
                }
            }
        }
        let online = group_online(&srv.active_characters, &srv.store, &srv.scene);
        // Invitations whose time ran out close the recruit's dialog (`groups::EXPIRED`).
        match srv.groups.prepare_expiry(srv.now_ms, &online) {
            Ok(Some(plan)) => {
                let result = group_admit(plan.deliveries(), &srv.active_characters, &peers)
                    .and_then(|packets| {
                        srv.groups.commit(plan).map_err(io::Error::other)?;
                        group_publish(packets, &mut peers)
                    });
                if let Err(error) = result {
                    eprintln!("Fellowship invitation expiry deferred: {error}");
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("Fellowship invitation expiry deferred: {error}"),
        }
        for player in srv.groups
            .participant_ids()
            .into_iter()
            .filter(|id| !online.contains_key(id))
        {
            let result = (|| -> io::Result<()> {
                let plan = srv.groups
                    .prepare_disconnect(player, srv.now_ms, &online)
                    .map_err(io::Error::other)?;
                let packets = group_admit(plan.deliveries(), &srv.active_characters, &peers)?;
                srv.groups.commit(plan).map_err(io::Error::other)?;
                group_publish(packets, &mut peers)
            })();
            if let Err(error) = result {
                eprintln!("Group departure deferred: {error}");
            }
        }
        // A party that left the world closes its trade (`trade::Runtime::leave`).
        for player in srv.trades.parties().into_iter().filter(|id| !srv.active_characters.contains_key(id)) {
            let closed: Vec<crate::scene::Delivery> = srv.trades
                .leave(player)
                .into_iter()
                .map(|(recipient, data)| crate::scene::Delivery { recipient, data: data.into() })
                .collect();
            deliver_scene(closed, &srv.active_characters, &mut peers, &srv.primary, srv.now, &mut srv.failed_peers)?;
        }
        if srv.now_ms >= srv.next_group_refresh {
            let mut refreshed = BTreeSet::new();
            for player in srv.groups.member_ids() {
                if refreshed.contains(&player) {
                    continue;
                }
                let members = srv.groups.members(player);
                let snapshot = members.iter().any(|id| {
                    online.get(id).is_none_or(|member| {
                        srv.group_cache
                            .get(id)
                            .is_none_or(|old| old.name != member.name)
                    })
                });
                if !snapshot
                    && members
                        .iter()
                        .all(|id| srv.group_cache.get(id) == online.get(id))
                {
                    refreshed.extend(members);
                    continue;
                }
                let result = (|| -> io::Result<()> {
                    let updates = if snapshot {
                        srv.groups.refresh(player, &online).map_err(io::Error::other)?
                    } else {
                        let mut updates = Vec::new();
                        for id in &members {
                            let member = online
                                .get(id)
                                .ok_or_else(|| invalid("Group member is unavailable."))?;
                            updates.extend(
                                srv.groups
                                    .member_changes(*id, member, srv.group_cache.get(id))
                                    .map_err(io::Error::other)?,
                            );
                        }
                        updates
                    };
                    let packets = group_admit(&updates, &srv.active_characters, &peers)?;
                    group_publish(packets, &mut peers)
                })();
                if result.is_ok() {
                    for id in members {
                        refreshed.insert(id);
                        if let Some(m) = online.get(&id) {
                            srv.group_cache.insert(id, m.clone());
                        }
                    }
                }
            }
            srv.group_cache.retain(|id, _| srv.groups.contains(*id));
            srv.next_group_refresh = srv.now_ms.saturating_add(1000);
        }
        deliver_scene(
            departures,
            &srv.active_characters,
            &mut peers,
            &srv.primary,
            srv.now,
            &mut srv.failed_peers,
        )?;
        if srv.now_ms >= srv.next_checkpoint {
            srv.store.save_due()?;
            srv.next_checkpoint = srv.now_ms + 5000;
        }
        if srv.now_ms >= srv.next_report {
            println!(
                "sessions={} received={} rejected={} logins={} connected={} events={}",
                peers.len(),
                srv.counters.received,
                srv.counters.rejected,
                srv.counters.logins,
                srv.counters.connected,
                srv.counters.events
            );
            srv.next_report = srv.now_ms + 10_000;
        }
        if srv.now_ms >= srv.next_gun_publish {
            srv.next_gun_publish = srv.now_ms + 500;
            // Friends see a character in the world as logged in and in game.
            crate::gun::publish_online(srv.active_characters.keys().filter_map(|id| {
                srv.store
                    .characters
                    .get(id)
                    .map(|character| (character.name.clone(), character.account.clone()))
            }));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    srv.store.save_now()?;
    println!(
        "Stopped: received={} rejected={} logins={} connected={} events={}",
        srv.counters.received, srv.counters.rejected, srv.counters.logins, srv.counters.connected, srv.counters.events
    );
    Ok(())
}
