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
                        summary.quests = job.change.quests.as_ref().map(|q| crate::live_edit::apply_quests(q, &mut srv.inventory_metadata));
                        summary.apply_us = started.elapsed().as_micros() as u64;
                        deliver_scene(deliveries, &srv.active_characters, &mut peers, &srv.primary, srv.now, &mut srv.failed_peers)?;
                        println!("Live edit: +{} -{} ~{} objects, {} rule targets{}, applied in {} µs.",
                            summary.added, summary.removed, summary.replaced, summary.rules,
                            summary.quests.map_or(String::new(), |n| format!(", quest tables replaced ({n} quests)")), summary.apply_us);
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
        encounter_tick(&mut srv, &mut peers)?;
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
