//! A datagram's handling up to its replies: logons, connect acknowledgements, and the
//! client events of a connected peer.
use super::*;

pub(super) fn receive(
    srv: &mut Server,
    peers: &mut BTreeMap<SocketAddr, Peer>,
    outbox: &mut Outbox,
    port_index: usize,
    addr: SocketAddr,
    h: &Header,
    bytes: &[u8],
) -> io::Result<Vec<Vec<u8>>> {
    if h.flags == wire::LOGON {
        if port_index != 0 {
            return Err(invalid("login on wrong port"));
        }
        if srv.auth_data.is_some() && !srv.login_limits.allow(addr.ip(), srv.now_ms / 1000) {
            return Err(invalid("Login rate limit."));
        }
        let (_, payload) = wire::decode_handshake(bytes).map_err(other)?;
        let login = wire::Login::decode(payload).map_err(other)?;
        // Type 1 is the recovered native admin client's no-ticket path.
        // This is an explicit local development identity, not password authentication.
        if srv.auth_data.is_none() && !login.supports_developer_auth(&srv.config.account) {
            return Err(invalid(&format!(
                "unsupported login: build={:?} type={:#x} flags={:#x} account_match={} crypto_bytes={} extra_bytes={}",
                login.version,
                login.auth_type,
                login.auth_flags,
                login.account == srv.config.account,
                login.crypto_data.len(),
                login.extra_data.len()
            )));
        }
        let proof = crate::auth::login_proof(&login);
        if let Some(peer) = peers.get(&addr) {
            if !peer.session.expired(srv.now_ms) && peer.login_proof == proof {
                return Ok(vec![peer.connect_packet.clone()]);
            }
        }
        if peers.len() >= 64
            && !peers.contains_key(&addr)
            && !(srv.auth_data.is_some()
                && peers
                    .values()
                    .any(|peer| peer.session.account == login.account))
        {
            return Err(invalid("session capacity"));
        }
        if let Some(root) = srv.auth_data.as_mut() {
            if !(srv.local_accounts && login.supports_developer_auth(&srv.config.account)) {
            let token = zeroize::Zeroizing::new(
                login
                    .native_ticket()
                    .map_err(|_| invalid("Public login rejected."))?,
            );
            crate::auth::consume(root, &login.account, addr.ip(), &token)?;
            }
            // A fresh authenticated login replaces this account's
            // previous endpoint; it cannot occupy every player slot.
            let previous: Vec<_> = peers
                .iter()
                .filter_map(|(old_addr, peer)| {
                    (*old_addr != addr && peer.session.account == login.account)
                        .then_some(*old_addr)
                })
                .collect();
            for old_addr in previous {
                let old = peers.remove(&old_addr).unwrap();
                if let Phase::Entering { character }
                | Phase::InWorld { character } = old.session.phase
                {
                    if srv.active_characters.get(&character) == Some(&old_addr) {
                        srv.active_characters.remove(&character);
                        departed(&mut srv.store, character);
                        if let Some(runtime) = &mut srv.encounters {
                            runtime.stop(character);
                        }
                        outbox.deliveries.extend(srv.scene.remove(character));
                    }
                }
            }
        }
        srv.generation = srv.generation
            .checked_add(1)
            .ok_or_else(|| invalid("session generation exhausted"))?;
        let id = (1..=u16::MAX)
            .find(|id| !peers.values().any(|p| p.id == *id))
            .ok_or_else(|| invalid("session IDs exhausted"))?;
        let iteration = u16::try_from(srv.generation)
            .map_err(|_| invalid("restart required before iteration reuse"))?;
        let secrets = crate::auth::random::<16>()?;
        let cookie = u64::from_le_bytes(secrets[..8].try_into().unwrap());
        let incoming = u32::from_le_bytes(secrets[8..12].try_into().unwrap());
        let outgoing = u32::from_le_bytes(secrets[12..].try_into().unwrap());
        let connect = wire::Connect {
            server_time: srv.world_clock.seconds(),
            cookie,
            net_id: u32::from(id),
            outgoing_seed: outgoing,
            incoming_seed: incoming,
        };
        let packet = wire::encode_handshake(
            Header {
                flags: wire::CONNECT,
                recipient: id,
                iteration,
                ..Default::default()
            },
            &connect.encode(),
        )
        .map_err(other)?;
        let previous = peers.insert(
            addr,
            Peer {
                client_version: login.version.clone(),
                session: Session::new(login.account.clone(), cookie, srv.now_ms),
                login_proof: proof,
                transport: Transport::new(incoming, outgoing, id, iteration),
                connect_packet: packet.clone(),
                id,
                iteration,
                blob: 0,
                selection_pending: None,
                ordered_stamp: 0,
                motion: None,
                target: None,
                combat: false,
                pending_skill: None,
                last_chat_ms: None,
                emotes: crate::emotes::State::default(),
                last_inventory_ms: None,
                last_examine_ms: None,
                last_world_use_ms: None,
            trade: Default::default(),
            deposits: BTreeMap::new(),
                last_npc_dialogue_ms: None,
                guide_page: 0,
                npc_dialogue: Default::default(),
                merchant_open_ms: None,
                last_shop_refresh_ms: None,
                native_shop: None,
                unhandled_scripts: BTreeSet::new(),
                scene_outbox: VecDeque::new(),
                scene_bytes: 0,
                last_time_sync_ms: srv.now_ms,
            },
        );
        if let Some(previous) = previous {
            if let Phase::Entering { character } | Phase::InWorld { character } =
                previous.session.phase
            {
                if srv.active_characters.get(&character) == Some(&addr) {
                    srv.active_characters.remove(&character);
                    departed(&mut srv.store, character);
                    if let Some(runtime) = &mut srv.encounters {
                        runtime.stop(character);
                    }
                    outbox.deliveries.extend(srv.scene.remove(character));
                }
            }
        }
        srv.counters.logins += 1;
        return Ok(vec![packet]);
    }
    let peer = peers
        .get_mut(&addr)
        .ok_or_else(|| invalid("unknown peer"))?;
    if h.recipient != peer.id || h.iteration != peer.iteration {
        return Err(invalid("stale or wrong session"));
    }
    if h.flags == wire::CONNECT_ACK {
        if port_index != 1 {
            return Err(invalid("connection acknowledgement on wrong port"));
        }
        let (_, payload) = wire::decode_handshake(bytes).map_err(other)?;
        let cookie = wire::decode_connect_ack(payload).map_err(other)?;
        let first = peer.session.phase == Phase::Connecting;
        peer.session.connect_ack(cookie, srv.now_ms).map_err(other)?;
        if !first {
            return Ok(vec![peer.transport.send_time_sync(srv.world_clock.seconds(), srv.now).map_err(other)?]);
        }
        srv.counters.connected += 1;
        let characters: Vec<_> = srv.store
            .characters
            .values()
            .filter(|c| c.account == peer.session.account && !c.deleted)
            .collect();
        let list = if let Some(profile) = &srv.bootstrap {
            let selection: Vec<_> = characters
                .iter()
                .filter_map(|c| {
                    c.creation
                        .as_ref()
                        .filter(|r| profile.supports(r))
                        .map(|r| {let mut selection=profile.selection(c.id,r);selection.appearance=crate::starter_gear::appearance(c).ok();selection})
                })
                .collect();
            world::character_set(peer.session.account.as_bytes(), 5, &selection)
                .map_err(other)?
        } else if let Some(did) = srv.config.visual_did {
            let selection: Vec<_> = characters
                .iter()
                .map(|c| world::SelectionCharacter {
                    id: c.id,
                    name: c.name.clone(),
                    seconds_greyed_out: 0,
                    parent_visual_did: did,
                    appearance: None,
                })
                .collect();
            world::character_set(peer.session.account.as_bytes(), 5, &selection)
                .map_err(other)?
        } else {
            world::empty_character_set(peer.session.account.as_bytes(), 5)
                .map_err(other)?
        };
        // Local asset mode: finish the zero-download DAT phase before
        // selection can invoke CharacterGenSystem. OnEndDDD initializes
        // the native VM; skipping it dereferences a null resolver.
        peer.selection_pending =
            Some(world::character_set_for(&peer.client_version, list));
        let mut packets = vec![peer.transport.send_time_sync(srv.world_clock.seconds(), srv.now).map_err(other)?];
        packets.extend(peer.send_to_queue(5, &0x1003bu32.to_le_bytes(), srv.now)?);
        return Ok(packets);
    }
    if peer.session.phase == Phase::Connecting || peer.session.expired(srv.now_ms) {
        return Err(invalid("session not connected"));
    }
    let received = peer.transport.receive(bytes, srv.now).map_err(other)?;
    outbox.recovery_packets.extend(received.retransmissions);
    if received.logoff {
        // Validation includes checksum and current session identity.
        // The end-of-loop removal releases presence and ownership.
        srv.store.save()?;
        srv.failed_peers.insert(addr);
        return Ok(Vec::new());
    }
    peer.session.touch(srv.now_ms);
    let mut replies = Vec::new();
    // Wait for delivery of the DAT notification so a lost first
    // packet cannot let queue1 race ahead of queue5 initialization.
    if !peer.transport.has_unacknowledged() {
        if let Some(list) = peer.selection_pending.take() {
            replies.extend(peer.send_event(&list, srv.now)?);
        }
    }
    if h.sequence != 0 {
        replies.push(peer.control(wire::ACK, &received.ack.to_le_bytes())?);
    }
    if let Some(echo) = received.echo_request {
        let mut body = echo.to_le_bytes().to_vec();
        body.extend(0.0f32.to_le_bytes());
        replies.push(peer.control(wire::ECHO_RESPONSE, &body)?);
    }
    for message in received.messages {
        if !matches!(message.queue, 3 | 4) {
            srv.counters.rejected += 1;
            let opcode = message
                .data
                .get(..4)
                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                .unwrap_or(0);
            if srv.diagnosed_queues.len() < 16
                && srv.diagnosed_queues.insert((message.queue, opcode))
            {
                eprintln!(
                    "client queue rejected: queue={} opcode={opcode:#x} bytes={} blob={:#x}",
                    message.queue,
                    message.data.len(),
                    message.blob_id
                );
            }
            continue;
        }
        srv.counters.events += 1;
        let event = match world::decode_client_event(&message.data) {
            Ok(event) => event,
            Err(error) => {
                srv.counters.rejected += 1;
                let opcode = message
                    .data
                    .get(..4)
                    .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                    .unwrap_or(0);
                if srv.diagnosed_events.len() < 32 && srv.diagnosed_events.insert(opcode) {
                    eprintln!(
                        "application decode rejected: opcode={opcode:#x} queue={} bytes={} reason={error}",
                        message.queue,
                        message.data.len()
                    );
                }
                continue;
            }
        };
        let application = client_event(srv, peer, outbox, addr, event);
        match application {
            Ok(events) => replies.extend(events),
            Err(error) => {
                srv.counters.rejected += 1;
                let reason: String = error.to_string().chars().take(512).collect();
                if srv.diagnosed_application_errors.len() < 32
                    && srv.diagnosed_application_errors.insert(reason)
                {
                    eprintln!("application event rejected: {error}");
                }
            }
        }
    }
    Ok(replies)
}
