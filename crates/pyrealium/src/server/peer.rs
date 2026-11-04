//! One client connection: its session, transport and per-connection state.
use super::*;

pub(super) struct Peer {
    pub(super) client_version: String,
    pub(super) session: Session,
    pub(super) transport: Transport,
    pub(super) connect_packet: Vec<u8>,
    pub(super) login_proof: String,
    pub(super) id: u16,
    pub(super) iteration: u16,
    pub(super) blob: u32,
    pub(super) selection_pending: Option<Vec<u8>>,
    pub(super) ordered_stamp: u16,
    pub(super) motion: Option<Motion>,
    pub(super) target: Option<u64>,
    pub(super) combat: bool,
    pub(super) pending_skill: Option<u32>,
    pub(super) emotes: crate::emotes::State,
    pub(super) last_chat_ms: Option<u64>,
    pub(super) last_inventory_ms: Option<u64>,
    pub(super) last_examine_ms: Option<u64>,
    pub(super) last_world_use_ms: Option<u64>,
    /// Tradeskills (`tradeskills.rs`): this connection's craft experience and prospected deposits.
    pub(super) trade: crate::tradeskills::Progress,
    pub(super) deposits: BTreeMap<u64, trade_actions::Deposit>,
    pub(super) last_npc_dialogue_ms: Option<u64>,
    pub(super) guide_page: usize,
    pub(super) npc_dialogue: crate::npc_dialogue::Cursor,
    pub(super) merchant_open_ms: Option<u64>,
    pub(super) last_shop_refresh_ms: Option<u64>,
    pub(super) native_shop: Option<crate::merchants::Shop>,
    pub(super) unhandled_scripts: BTreeSet<u32>,
    pub(super) scene_outbox: VecDeque<(Arc<[u8]>, bool)>,
    pub(super) scene_bytes: usize,
    pub(super) last_time_sync_ms: u64,
}
impl Peer {
    pub(super) fn admit_npc_dialogue(&mut self, now: u64) -> io::Result<()> {
        if self.last_npc_dialogue_ms.is_some_and(|last| now.saturating_sub(last) < 1000) {
            return Err(invalid("Wait briefly before speaking again."));
        }
        self.last_npc_dialogue_ms = Some(now);
        Ok(())
    }

    pub(super) fn clear_scene_outbox(&mut self) {
        self.scene_outbox.clear();
        self.scene_bytes = 0;
    }
    pub(super) fn queue_scene(&mut self, data: Arc<[u8]>) -> io::Result<()> {
        let ordered = Self::scene_ordered(&data);
        self.queue_scene_as(data, ordered)
    }
    pub(super) fn scene_ordered(data: &[u8]) -> bool {
        data.get(..4).is_some_and(|opcode| {
            matches!(
                u32::from_le_bytes(opcode.try_into().unwrap()),
                0x10058 | 0x100c2 | 0x10069
                    ..=0x10076 | 0x100d4 | 0x100d5 | 0x10066 | 0x100a8 | 0x10082 | crate::emotes::EVENT
            )
        })
    }
    pub(super) fn queue_scene_as(&mut self, data: Arc<[u8]>, ordered: bool) -> io::Result<()> {
        if data.len() < 4
            || data.len() > 65536
            || self.scene_outbox.len() >= 1024
            || self.scene_bytes.saturating_add(data.len()) > 1024 * 1024
        {
            return Err(invalid("scene outbox limit"));
        }
        self.scene_bytes += data.len();
        self.scene_outbox.push_back((data, ordered));
        Ok(())
    }
    pub(super) fn flush_scene(
        &mut self,
        socket: &UdpSocket,
        addr: &SocketAddr,
        now: Instant,
    ) -> io::Result<()> {
        if !matches!(
            self.session.phase,
            Phase::Entering { .. } | Phase::InWorld { .. }
        ) {
            self.clear_scene_outbox();
            return Ok(());
        }
        for _ in 0..32 {
            let Some((data, ordered)) = self.scene_outbox.front().cloned() else {
                break;
            };
            if !self.transport.send_available(data.len()).map_err(other)? {
                break;
            }
            let packets = if crate::room_chat::is_chat(&data) {
                self.send_chat_event(&data, now)?
            } else {
                self.send_world_event(&data, ordered, now)?
            };
            self.scene_outbox.pop_front();
            self.scene_bytes -= data.len();
            for packet in packets {
                socket.send_to(&packet, addr)?;
            }
        }
        Ok(())
    }
    /// Admit a dependent group completely before changing the outgoing queue.
    /// Fragment-window pressure is handled by flush_scene, without cloning
    /// transport/ISAAC history or requiring every event to fit in one send window.
    pub(super) fn validate_scene_batch(&self, events: &[Vec<u8>]) -> io::Result<usize> {
        let mut total = self.scene_bytes;
        if self.scene_outbox.len().saturating_add(events.len()) > 1024 {
            return Err(invalid("scene batch event limit"));
        }
        for event in events {
            if !(4..=65536).contains(&event.len()) {
                return Err(invalid("scene batch message limit"));
            }
            total = total
                .checked_add(event.len())
                .filter(|size| *size <= 1024 * 1024)
                .ok_or_else(|| invalid("scene batch byte limit"))?;
        }
        Ok(total)
    }
    pub(super) fn queue_event_batch(&mut self, events: Vec<Vec<u8>>) -> io::Result<()> {
        let total = self.validate_scene_batch(&events)?;
        self.scene_outbox.extend(events.into_iter().map(|data| {
            let ordered = Self::scene_ordered(&data);
            (Arc::from(data), ordered)
        }));
        self.scene_bytes = total;
        Ok(())
    }
    pub(super) fn queue_ordered_batch(&mut self, events: Vec<Vec<u8>>) -> io::Result<()> {
        let count = events.len();
        self.queue_event_batch(events)?;
        for (_, ordered) in self.scene_outbox.iter_mut().rev().take(count) {
            *ordered = true;
        }
        Ok(())
    }
    pub(super) fn send_world_event(
        &mut self,
        bytes: &[u8],
        ordered: bool,
        now: Instant,
    ) -> io::Result<Vec<Vec<u8>>> {
        let blob = self
            .blob
            .checked_add(1)
            .ok_or_else(|| invalid("blob sequence exhausted"))?;
        let ordered_stamp = if ordered {
            self.ordered_stamp.wrapping_add(1)
        } else {
            self.ordered_stamp
        };
        let (namespace, stamp) = if ordered {
            (0x0100_0000u32, u32::from(ordered_stamp))
        } else {
            (0x0300_0000u32, blob & 0xffff)
        };
        let id = (u64::from(namespace | stamp) << 32) | u64::from(blob);
        let packets = self.transport.send(id, 1, bytes, now).map_err(other)?;
        self.blob = blob;
        self.ordered_stamp = ordered_stamp;
        Ok(packets)
    }
    /// Turbine chat (`room_chat`): queue 4, the unordered `0x23` id space retail's chat blobs
    /// used (2005 `group.chat.elog`: `0x23DD....`), addressed to this connection.
    pub(super) fn send_chat_event(&mut self, bytes: &[u8], now: Instant) -> io::Result<Vec<Vec<u8>>> {
        let blob = self
            .blob
            .checked_add(1)
            .ok_or_else(|| invalid("blob sequence exhausted"))?;
        let id = (u64::from(0x2300_0000 | (blob & 0xffff)) << 32) | u64::from(blob);
        let data = crate::room_chat::addressed(bytes, u32::from(self.id));
        let packets = self.transport.send(id, 4, &data, now).map_err(other)?;
        self.blob = blob;
        Ok(packets)
    }
    pub(super) fn send_event(&mut self, bytes: &[u8], now: Instant) -> io::Result<Vec<Vec<u8>>> {
        if matches!(
            self.session.phase,
            Phase::Entering { .. } | Phase::InWorld { .. }
        ) {
            self.send_world_event(bytes, true, now)
        } else {
            self.send_to_queue(1, bytes, now)
        }
    }
    pub(super) fn send_to_queue(
        &mut self,
        queue: u16,
        bytes: &[u8],
        now: Instant,
    ) -> io::Result<Vec<Vec<u8>>> {
        let blob = self
            .blob
            .checked_add(1)
            .ok_or_else(|| invalid("blob sequence exhausted"))?;
        // 00494890 drops ordinary world events while NOT_IN_WORLD. Pre-world
        // messages need bit61 and unordered type3, with no player CBookKeep.
        let preworld = !matches!(
            self.session.phase,
            Phase::InWorld { .. } | Phase::Entering { .. }
        );
        let namespace = if preworld { 0x2300_0000 } else { 0x0100_0000 };
        // Native client's CEventQueue is receive queue1; queue4 is its logon
        // chat queue and discards CharacterSet (0071fb10).
        // The upper ordering stamp is only 16 bits; the blob identifier is 32.
        // Long world sessions must still be able to return to selection.
        let id = (u64::from(namespace | (blob & 0xffff)) << 32) | u64::from(blob);
        let packets = self.transport.send(id, queue, bytes, now).map_err(other)?;
        self.blob = blob;
        Ok(packets)
    }
    pub(super) fn control(&self, flag: u32, body: &[u8]) -> io::Result<Vec<u8>> {
        wire::encode_control_packet(
            Header {
                flags: flag,
                recipient: self.id,
                iteration: self.iteration,
                ..Default::default()
            },
            body,
            None,
        )
        .map_err(other)
    }
}
