//! One received datagram: its replies and recovery packets, the trade and group requests it
//! queued, and its scene deliveries.
use super::*;

/// What handling one datagram queues beside its direct replies.
pub(super) struct Outbox {
    pub(super) deliveries: Vec<crate::scene::Delivery>,
    pub(super) group_requests: Vec<(u64, crate::groups::Request)>,
    pub(super) trade_requests: Vec<(u64, crate::trade::Request)>,
    pub(super) recovery_packets: Vec<Vec<u8>>,
}

pub(super) fn datagram(
    srv: &mut Server,
    peers: &mut BTreeMap<SocketAddr, Peer>,
    port_index: usize,
    addr: SocketAddr,
    h: &Header,
    bytes: &[u8],
    len: usize,
) -> io::Result<()> {
    let mut outbox = Outbox {
        deliveries: Vec::new(),
        group_requests: Vec::new(),
        trade_requests: Vec::new(),
    // Keep transport recovery outside fallible application handling.
    // A later world/UI rejection must not discard ACK-covered NAKs.
        recovery_packets: Vec::new(),
    };
    let result = receive(srv, peers, &mut outbox, port_index, addr, h, bytes);
    for packet in outbox.recovery_packets {
        srv.primary.send_to(&packet, addr)?;
    }
    match result {
        Ok(replies) => {
            for reply in replies {
                srv.primary.send_to(&reply, addr)?;
            }
        }
        Err(error) => {
            srv.counters.rejected += 1;
            let reason: String = error.to_string().chars().take(512).collect();
            if srv.diagnosed_packet_errors.len() < 32
                && srv.diagnosed_packet_errors.insert(reason)
            {
                eprintln!(
                    "packet rejected: flags={:#x} sequence={} bytes={} reason={error} state={}",
                    h.flags,
                    h.sequence,
                    len,
                    peers
                        .get(&addr)
                        .map(|peer| peer.transport.diagnostic())
                        .unwrap_or_else(|| "no peer".into())
                );
            }
        }
    }
    for (character, request) in outbox.trade_requests {
        let out = trade_step(character, &request, &mut srv.trades, &mut srv.store, &mut srv.scene, &srv.active_characters, &srv.inventory_metadata);
        outbox.deliveries.extend(out);
    }
    for (character, request) in outbox.group_requests {
        let online = group_online(&srv.active_characters, &srv.store, &srv.scene);
        // A refused request answers with the client's own line (`GroupError::wire`).
        let result = (|| -> io::Result<Result<(), crate::groups::GroupError>> {
            let plan = match srv.groups.prepare(character, &request, srv.now_ms, &online) {
                Ok(plan) => plan,
                Err(refused) => return Ok(Err(refused)),
            };
            let packets = group_admit(plan.deliveries(), &srv.active_characters, peers)?;
            srv.groups.commit(plan).map_err(io::Error::other)?;
            group_publish(packets, &mut *peers)?;
            Ok(Ok(()))
        })();
        let reply = match result {
            Ok(Ok(())) => None,
            Ok(Err(refused)) => Some(refused.wire().map_err(other)?),
            Err(error) => Some(script::chat(&error.to_string(), 1).map_err(other)?),
        };
        if let Some(reply) = reply {
            if let Some(peer) = srv.active_characters
                .get(&character)
                .and_then(|addr| peers.get_mut(addr))
            {
                if peer.queue_ordered_batch(vec![reply]).is_err() {
                    srv.failed_peers.insert(addr);
                }
            }
        }
    }
    deliver_scene(
        outbox.deliveries,
        &srv.active_characters,
        &mut *peers,
        &srv.primary,
        srv.now,
        &mut srv.failed_peers,
    )?;
    Ok(())
}
