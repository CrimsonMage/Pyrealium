//! Bounded session reliability. State commits only after checksum and structure validation.
use crate::{
    crypto::Isaac,
    wire::{self, Fragment, Header, WireError},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

const WINDOW: u32 = 512;
const MAX_MESSAGE: usize = 64 * 1024;
const FRAGMENT_DATA: usize = 448;
const MAX_PENDING: usize = 256;
const RECOVERY_LIFETIME: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum TransportError {
    Wire(WireError),
    Sequence,
    Capacity,
    OrderedCapacity {
        expected: u32,
        received: u16,
        buffered: usize,
        bytes: usize,
    },
    Fragment,
    RetryLimit,
}
impl From<WireError> for TransportError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}
impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for TransportError {}

#[derive(Debug, Clone)]
pub struct Message {
    pub blob_id: u64,
    pub queue: u16,
    pub data: Vec<u8>,
}
#[derive(Debug)]
pub struct Received {
    pub logoff: bool,
    pub messages: Vec<Message>,
    pub duplicate: bool,
    pub ack: u32,
    pub echo_request: Option<f32>,
    pub naks: Vec<u32>,
    /// Validated retry bytes staged before the same packet retires ACKed data.
    pub retransmissions: Vec<Vec<u8>>,
}
#[derive(Clone)]
struct Assembly {
    queue: u16,
    pieces: Vec<Option<Vec<u8>>>,
    bytes: usize,
}
#[derive(Clone)]
struct Pending {
    // Transactional receive clones metadata, not the immutable retry payload.
    bytes: Arc<[u8]>,
    sent: Instant,
    retries: u8,
}
#[derive(Clone)]
struct Retired {
    packet: Pending,
    acknowledged: Instant,
}
#[derive(Clone)]
pub struct Transport {
    incoming: Isaac,
    outgoing: Isaac,
    recipient: u16,
    iteration: u16,
    generated: u32,
    contiguous: u32,
    seen: BTreeSet<u32>,
    keys: BTreeMap<u32, u32>,
    next_outgoing: u32,
    pending: BTreeMap<u32, Pending>,
    // Highest-observed ACKs can precede a separate NAK for an earlier hole.
    // Keep immutable bytes briefly without occupying the active send window.
    retired: BTreeMap<u32, Retired>,
    assemblies: BTreeMap<u64, Assembly>,
    completed: VecDeque<u64>,
    next_ordered: u32,
    ordered: BTreeMap<u16, Message>,
    ordered_bytes: usize,
}
impl Transport {
    /// Bounded counters for rejection diagnostics; never exposes cipher keys.
    pub fn diagnostic(&self) -> String {
        format!(
            "received_through={} generated_through={} next_send={} next_ordered={} pending={} recovery={} assemblies={} ordered={}",
            self.contiguous,
            self.generated,
            self.next_outgoing,
            self.next_ordered,
            self.pending.len(),
            self.retired.len(),
            self.assemblies.len(),
            self.ordered.len()
        )
    }
    pub fn new(incoming_seed: u32, outgoing_seed: u32, recipient: u16, iteration: u16) -> Self {
        Self {
            incoming: Isaac::new(incoming_seed),
            outgoing: Isaac::new(outgoing_seed),
            recipient,
            iteration,
            generated: 1,
            contiguous: 1,
            seen: BTreeSet::new(),
            keys: BTreeMap::new(),
            next_outgoing: 2,
            pending: BTreeMap::new(),
            retired: BTreeMap::new(),
            assemblies: BTreeMap::new(),
            completed: VecDeque::new(),
            next_ordered: 1,
            ordered: BTreeMap::new(),
            ordered_bytes: 0,
        }
    }
    pub fn receive(&mut self, bytes: &[u8], now: Instant) -> Result<Received, TransportError> {
        let header = Header::decode(bytes)?;
        if header.sequence == 0
            && header.flags & (wire::ENCRYPTED_CHECKSUM | wire::BLOB_FRAGMENTS) == 0
        {
            if header.recipient != self.recipient || header.iteration != self.iteration {
                return Err(TransportError::Sequence);
            }
            // Fully decode/checksum the packet before mutation. These controls
            // consume neither ISAAC keys nor assembly/order state, so copying
            // pending payloads provides no rollback benefit.
            let packet = wire::decode_session_packet(bytes, None)?;
            let retransmissions = self.receive_ack_naks(packet.ack, &packet.naks, now)?;
            return Ok(Received {
                logoff: packet.header.flags == wire::LOGOFF,
                messages: Vec::new(),
                duplicate: false,
                ack: self.contiguous,
                echo_request: packet.echo_request,
                naks: packet.naks,
                retransmissions,
            });
        }
        // A malformed packet must never consume ISAAC state or partially publish a message.
        let mut candidate = self.clone();
        let result = candidate.receive_inner(bytes, now)?;
        *self = candidate;
        Ok(result)
    }
    fn receive_inner(&mut self, bytes: &[u8], now: Instant) -> Result<Received, TransportError> {
        let header = Header::decode(bytes)?;
        if header.recipient != self.recipient || header.iteration != self.iteration {
            return Err(TransportError::Sequence);
        }
        let seq = header.sequence;
        if seq > self.contiguous.saturating_add(WINDOW) {
            return Err(TransportError::Sequence);
        }
        let encrypted = header.flags & wire::ENCRYPTED_CHECKSUM != 0;
        if encrypted && seq < 2 {
            return Err(TransportError::Sequence);
        }
        let key = if encrypted {
            if seq.saturating_add(WINDOW) < self.contiguous {
                return Err(TransportError::Sequence);
            }
            while self.generated < seq {
                self.generated += 1;
                self.keys.insert(self.generated, self.incoming.next_key());
            }
            Some(*self.keys.get(&seq).ok_or(TransportError::Sequence)?)
        } else {
            None
        };
        let packet = wire::decode_session_packet(bytes, key)?;
        // Local emulator policy: application data must use reliable checksum masking.
        if !encrypted && !packet.fragments.is_empty() {
            return Err(TransportError::Sequence);
        }
        let duplicate = encrypted && (seq <= self.contiguous || self.seen.contains(&seq));
        let retransmissions = self.receive_ack_naks(packet.ack, &packet.naks, now)?;
        let mut messages = Vec::new();
        // Received fragment packets are acknowledged independently of blob
        // completion. Silently expiring their pieces would lose acknowledged
        // data and permanently block a later ordered application stamp.
        // Incomplete blobs remain bounded by count/bytes until session removal.
        if !duplicate {
            let mut ordered = Vec::new();
            for fragment in packet.fragments {
                if let Some(message) = self.add_fragment(fragment)? {
                    if matches!(message.queue, 3 | 7) {
                        ordered.push(message);
                    } else {
                        self.deliver(message, &mut messages)?;
                    }
                }
            }
            // A datagram can finish the missing ordered blob after another
            // future blob. Admit the gap-closing completion first so a full
            // reorder buffer cannot reject the very packet that drains it.
            // Unordered completions retain their original relative order.
            ordered.sort_by_key(|message| (((message.blob_id >> 32) & 0xffff) as u16).wrapping_sub(self.next_ordered as u16));
            for message in ordered {
                self.deliver(message, &mut messages)?;
            }
            if encrypted {
                self.seen.insert(seq);
                while self.contiguous < u32::MAX && self.seen.remove(&(self.contiguous + 1)) {
                    self.contiguous += 1;
                }
                let floor = self.contiguous.saturating_sub(WINDOW);
                self.keys.retain(|&id, _| id >= floor);
            }
        }
        Ok(Received {
            logoff: false,
            messages,
            duplicate,
            ack: self.contiguous,
            echo_request: packet.echo_request,
            naks: packet.naks,
            retransmissions,
        })
    }
    fn deliver(
        &mut self,
        message: Message,
        ready: &mut Vec<Message>,
    ) -> Result<(), TransportError> {
        if !matches!(message.queue, 3 | 7) {
            // `Proto_UI::SendEnterWorld` restarts the client's ordered stamps
            // (`ClientNet::EnterWorld`: `Proto_UI::SetOrderingStamp(0)`), so the first ordered
            // blob of a world entry after a log off carries stamp 1 again. The expected stamp
            // restarts with it; without this every ordered blob of the second entry looked old
            // and was dropped (no scene-ready, no logon content).
            if message.queue == 4
                && message.data.get(..4) == Some(&crate::world::ENTER_WORLD.to_le_bytes()[..])
            {
                self.next_ordered = 1;
                self.ordered.clear();
                self.ordered_bytes = 0;
            }
            ready.push(message);
            return Ok(());
        }
        // Proto_UI::SendBuff shares a single ushort counter across queues 3 and 7.
        let stamp = ((message.blob_id >> 32) & 0xffff) as u16;
        if (message.blob_id >> 56) & 0x1f != 1
        {
            return Err(TransportError::Sequence);
        }
        let distance = stamp.wrapping_sub(self.next_ordered as u16);
        if distance >= 0x8000 {
            return Ok(());
        }
        if distance == 0 {
            ready.push(message);
            self.next_ordered = u32::from(stamp.wrapping_add(1));
            loop {
                let Some(message) = self.ordered.remove(&(self.next_ordered as u16)) else {
                    break;
                };
                self.ordered_bytes -= message.data.len();
                ready.push(message);
                self.next_ordered = u32::from((self.next_ordered as u16).wrapping_add(1));
            }
            return Ok(());
        }
        if let Some(old) = self.ordered.get(&stamp) {
            if old.blob_id != message.blob_id
                || old.queue != message.queue
                || old.data != message.data
            {
                return Err(TransportError::Fragment);
            }
            return Ok(());
        }
        if u32::from(distance) >= WINDOW
            || self.ordered.len() >= WINDOW as usize
            || self.ordered_bytes + message.data.len() > MAX_MESSAGE * 4
        {
            return Err(TransportError::OrderedCapacity {
                expected: self.next_ordered,
                received: stamp,
                buffered: self.ordered.len(),
                bytes: self.ordered_bytes,
            });
        }
        self.ordered_bytes += message.data.len();
        self.ordered.insert(stamp, message);
        Ok(())
    }
    fn add_fragment(&mut self, f: Fragment<'_>) -> Result<Option<Message>, TransportError> {
        if self.completed.contains(&f.blob_id) {
            return Ok(None);
        }
        if f.count == 0 || f.count > 256 || f.index >= f.count || f.data.len() > MAX_MESSAGE {
            return Err(TransportError::Fragment);
        }
        if !self.assemblies.contains_key(&f.blob_id) && self.assemblies.len() >= 16 {
            return Err(TransportError::Capacity);
        }
        let a = self
            .assemblies
            .entry(f.blob_id)
            .or_insert_with(|| Assembly {
                queue: f.queue,
                pieces: vec![None; f.count as usize],
                bytes: 0,
            });
        if a.queue != f.queue || a.pieces.len() != f.count as usize {
            return Err(TransportError::Fragment);
        }
        match &a.pieces[f.index as usize] {
            Some(old) if old != f.data => return Err(TransportError::Fragment),
            Some(_) => return Ok(None),
            None => {}
        }
        a.bytes = a
            .bytes
            .checked_add(f.data.len())
            .ok_or(TransportError::Capacity)?;
        if a.bytes > MAX_MESSAGE {
            return Err(TransportError::Capacity);
        }
        a.pieces[f.index as usize] = Some(f.data.to_vec());
        if a.pieces.iter().any(Option::is_none) {
            return Ok(None);
        }
        let a = self.assemblies.remove(&f.blob_id).unwrap();
        let mut data = Vec::with_capacity(a.bytes);
        for piece in a.pieces {
            data.extend(piece.unwrap());
        }
        self.completed.push_back(f.blob_id);
        if self.completed.len() > 512 {
            self.completed.pop_front();
        }
        Ok(Some(Message {
            blob_id: f.blob_id,
            queue: f.queue,
            data,
        }))
    }
    pub fn send_available(&self, bytes: usize) -> Result<bool, TransportError> {
        if bytes == 0 || bytes > MAX_MESSAGE {
            return Err(TransportError::Capacity);
        }
        let count = bytes.div_ceil(FRAGMENT_DATA);
        if self.next_outgoing.checked_add(count as u32).is_none() {
            return Err(TransportError::Sequence);
        }
        Ok(self.pending.len() + count <= MAX_PENDING)
    }
    /// Native CTimeSyncHeader is reliable (m_Flags=0x18), so it needs a sequence
    /// and an ISAAC checksum, even without a fragment. A plain sequence-zero
    /// packet is rejected by the retail SplitPacketData header policy.
    pub fn send_time_sync(&mut self, seconds: f64, now: Instant) -> Result<Vec<u8>, TransportError> {
        if !seconds.is_finite() || seconds < 0.0 || !self.send_available(8)? {
            return Err(TransportError::Capacity);
        }
        let mut outgoing = self.outgoing.clone();
        let bytes = wire::encode_control_packet(Header {
            sequence: self.next_outgoing,
            flags: wire::TIME_SYNC | wire::ENCRYPTED_CHECKSUM,
            recipient: self.recipient,
            iteration: self.iteration,
            ..Default::default()
        }, &seconds.to_le_bytes(), Some(outgoing.next_key()))?;
        self.pending.insert(self.next_outgoing, Pending {
            bytes: Arc::from(bytes.as_slice()), sent: now, retries: 0,
        });
        self.outgoing = outgoing;
        self.next_outgoing += 1;
        Ok(bytes)
    }
    pub fn send(
        &mut self,
        blob_id: u64,
        queue: u16,
        data: &[u8],
        now: Instant,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        if data.is_empty() || data.len() > MAX_MESSAGE {
            return Err(TransportError::Capacity);
        }
        let count = data.len().div_ceil(FRAGMENT_DATA);
        if self.pending.len() + count > MAX_PENDING
            || self.next_outgoing.checked_add(count as u32).is_none()
        {
            return Err(TransportError::Capacity);
        }
        let mut output = Vec::with_capacity(count);
        for (index, part) in data.chunks(FRAGMENT_DATA).enumerate() {
            let fragment = Fragment {
                blob_id,
                count: count as u16,
                index: index as u16,
                queue,
                data: part,
            }
            .encode()?;
            let body =
                wire::checksum(&fragment[..16]).wrapping_add(wire::checksum(&fragment[16..]));
            let mut h = Header {
                sequence: self.next_outgoing,
                flags: wire::BLOB_FRAGMENTS | wire::ENCRYPTED_CHECKSUM,
                recipient: self.recipient,
                iteration: self.iteration,
                payload_len: fragment.len() as u16,
                ..Default::default()
            };
            h.checksum = h
                .header_checksum()
                .wrapping_add(body ^ self.outgoing.next_key());
            let mut bytes = h.encode().to_vec();
            bytes.extend(fragment);
            self.pending.insert(
                self.next_outgoing,
                Pending {
                    bytes: Arc::from(bytes.as_slice()),
                    sent: now,
                    retries: 0,
                },
            );
            self.next_outgoing += 1;
            output.push(bytes);
        }
        Ok(output)
    }
    pub fn acknowledge(&mut self, sequence: u32) -> Result<(), TransportError> {
        self.acknowledge_at(sequence, Instant::now())
    }
    fn acknowledge_at(&mut self, sequence: u32, now: Instant) -> Result<(), TransportError> {
        if sequence >= self.next_outgoing {
            return Err(TransportError::Sequence);
        }
        self.retired.retain(|_, retired| {
            now.saturating_duration_since(retired.acknowledged) < RECOVERY_LIFETIME
        });
        while self
            .pending
            .first_key_value()
            .is_some_and(|(&seq, _)| seq <= sequence)
        {
            let (seq, packet) = self.pending.pop_first().unwrap();
            self.retired.insert(
                seq,
                Retired {
                    packet,
                    acknowledged: now,
                },
            );
            if self.retired.len() > MAX_PENDING {
                self.retired.pop_first();
            }
        }
        Ok(())
    }
    pub fn has_unacknowledged(&self) -> bool {
        !self.pending.is_empty()
    }
    fn receive_ack_naks(
        &mut self,
        ack: Option<u32>,
        sequences: &[u32],
        now: Instant,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        // Native optional headers process HandleNak (004c5d60) before
        // HandlePak (004c5d20); the latter defers retirement through QueueFlush.
        // PAK is highest observed, so its packet may also NAK an earlier hole.
        if ack.is_some_and(|sequence| sequence >= self.next_outgoing) {
            return Err(TransportError::Sequence);
        }
        let retransmissions = self.retransmit_requested_inner(sequences, now, ack)?;
        // All fallible validation above precedes mutation on the control path.
        // The encrypted path additionally runs on a transactional candidate.
        if let Some(ack) = ack {
            self.acknowledge_at(ack, now)?;
        }
        Ok(retransmissions)
    }
    pub fn retransmit_requested(
        &mut self,
        sequences: &[u32],
        now: Instant,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        self.retransmit_requested_inner(sequences, now, None)
    }
    fn retransmit_requested_inner(
        &mut self,
        sequences: &[u32],
        now: Instant,
        retiring_through: Option<u32>,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        if sequences.len() > 256 {
            return Err(TransportError::Capacity);
        }
        let mut staged = Vec::new();
        for seq in sequences.iter().copied().collect::<BTreeSet<_>>() {
            let packet = self.pending.get(&seq).or_else(|| {
                self.retired
                    .get(&seq)
                    .filter(|retired| {
                        now.saturating_duration_since(retired.acknowledged) < RECOVERY_LIFETIME
                    })
                    .map(|retired| &retired.packet)
            });
            if let Some(p) = packet {
                if p.retries >= 8 {
                    return Err(TransportError::RetryLimit);
                }
                // Retiring a requested packet must not defeat its NAK merely
                // because it was sent recently. It can be staged only once:
                // the accompanying ACK removes it from the active window.
                // Cached retries always retain the normal rate limit.
                if (self.pending.contains_key(&seq)
                    && retiring_through.is_some_and(|ack| seq <= ack))
                    || now.saturating_duration_since(p.sent) >= Duration::from_millis(100)
                {
                    staged.push((seq, p.bytes.to_vec()));
                }
            }
        }
        // Preflight every requested packet before changing any retry counters.
        let mut result = Vec::with_capacity(staged.len());
        for (seq, bytes) in staged {
            let pending = match self.pending.get_mut(&seq) {
                Some(pending) => pending,
                None => &mut self.retired.get_mut(&seq).unwrap().packet,
            };
            pending.retries += 1;
            pending.sent = now;
            result.push(bytes);
        }
        Ok(result)
    }
    pub fn retransmit_due(&mut self, now: Instant) -> Result<Vec<Vec<u8>>, TransportError> {
        let mut result = Vec::new();
        for p in self.pending.values_mut() {
            let delay = Duration::from_millis(500u64 << p.retries.min(3));
            if now.saturating_duration_since(p.sent) >= delay {
                if p.retries >= 8 {
                    return Err(TransportError::RetryLimit);
                }
                p.retries += 1;
                p.sent = now;
                result.push(p.bytes.to_vec());
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_time_sync_is_reliable_and_keeps_following_blob_keys_aligned() {
        let now = Instant::now();
        let mut sender = Transport::new(11, 22, 1, 1);
        let mut receiver = Transport::new(22, 11, 1, 1);
        let before = sender.diagnostic();
        assert!(sender.send_time_sync(f64::NAN, now).is_err());
        assert_eq!(sender.diagnostic(), before);
        let sync = sender.send_time_sync(7347.25, now).unwrap();
        let mut keys = Isaac::new(22);
        let decoded = wire::decode_session_packet(&sync, Some(keys.next_key())).unwrap();
        assert_eq!(decoded.header.flags, wire::TIME_SYNC | wire::ENCRYPTED_CHECKSUM);
        assert_eq!(decoded.time_sync, Some(7347.25));
        assert_eq!(receiver.receive(&sync, now).unwrap().ack, 2);
        let blob = sender.send(7, 1, b"after clock", now).unwrap();
        let result = receiver.receive(&blob[0], now).unwrap();
        assert_eq!(result.ack, 3);
        assert_eq!(result.messages[0].data, b"after clock");
        assert!(sender.has_unacknowledged());
        sender.acknowledge(3).unwrap();
        assert!(!sender.has_unacknowledged());
    }
    #[test]
    fn control_fast_path_validates_before_retiring_pending_payloads() {
        let mut transport = Transport::new(7, 9, 1, 1);
        let now = Instant::now();
        for id in 1..=16 {
            transport.send(id, 1, &[42; 1000], now).unwrap();
        }
        let before = transport.pending.len();
        let control = |ack: u32| {
            wire::encode_control_packet(
                Header {
                    flags: wire::ACK,
                    recipient: 1,
                    iteration: 1,
                    ..Default::default()
                },
                &ack.to_le_bytes(),
                None,
            )
            .unwrap()
        };
        assert!(transport.receive(&control(u32::MAX), now).is_err());
        assert_eq!(transport.pending.len(), before);
        let mut corrupt = control(3);
        corrupt[8] ^= 1;
        assert!(transport.receive(&corrupt, now).is_err());
        assert_eq!(transport.pending.len(), before);
        transport.receive(&control(3), now).unwrap();
        assert_eq!(transport.pending.len(), before - 2);
        transport.receive(&control(1), now).unwrap();
        assert_eq!(transport.pending.len(), before - 2);
        assert_eq!(transport.generated, 1);
        assert_eq!(transport.contiguous, 1);
    }
    fn pair() -> (Transport, Transport) {
        (Transport::new(7, 9, 1, 1), Transport::new(9, 7, 1, 1))
    }
    #[test]
    fn reordering_reassembly_duplicates_and_ack() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let data = vec![42; 1000];
        let packets = a.send(1, 2, &data, now).unwrap();
        assert_eq!(b.receive(&packets[2], now).unwrap().ack, 1);
        assert!(b.receive(&packets[2], now).unwrap().duplicate);
        assert_eq!(b.receive(&packets[0], now).unwrap().ack, 2);
        let result = b.receive(&packets[1], now).unwrap();
        assert_eq!(result.ack, 4);
        assert_eq!(result.messages[0].data, data);
        a.acknowledge(4).unwrap();
        assert!(a
            .retransmit_due(now + Duration::from_secs(1))
            .unwrap()
            .is_empty());
    }
    #[test]
    fn bad_checksum_does_not_advance_crypto() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let packet = a.send(1, 2, b"hello", now).unwrap().remove(0);
        let mut bad = packet.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(b.receive(&bad, now).is_err());
        assert_eq!(b.receive(&packet, now).unwrap().messages[0].data, b"hello");
    }
    #[test]
    fn huge_gap_and_conflicting_fragments_are_transactional() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let packet = a.send(1, 2, b"hello", now).unwrap().remove(0);
        let mut bad = packet.clone();
        bad[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(b.receive(&bad, now).is_err());
        assert_eq!(b.generated, 1);
        assert!(b
            .add_fragment(Fragment {
                blob_id: 2,
                count: 2,
                index: 0,
                queue: 2,
                data: b"a"
            })
            .unwrap()
            .is_none());
        assert!(b
            .add_fragment(Fragment {
                blob_id: 2,
                count: 2,
                index: 0,
                queue: 2,
                data: b"b"
            })
            .is_err());
    }
    #[test]
    fn retransmission_reuses_exact_bytes_and_stops() {
        let (mut a, _) = pair();
        let now = Instant::now();
        let packets = a.send(1, 2, b"hello", now).unwrap();
        for i in 1..=8 {
            assert_eq!(
                a.retransmit_due(now + Duration::from_secs(i * 5)).unwrap(),
                packets
            );
        }
        assert!(matches!(
            a.retransmit_due(now + Duration::from_secs(50)),
            Err(TransportError::RetryLimit)
        ));
    }
    #[test]
    fn invalid_fragment_after_ack_preserves_shared_retry_payload_and_crypto() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let outgoing = b.send(1, 2, b"must survive rollback", now).unwrap();
        let retry_bytes = b.pending[&2].bytes.clone();
        let fragments: Vec<_> = [2, 3]
            .into_iter()
            .map(|count| {
                Fragment {
                    blob_id: 99,
                    count,
                    index: 0,
                    queue: 4,
                    data: b"x",
                }
                .encode()
                .unwrap()
            })
            .collect();
        let ack = 2u32.to_le_bytes();
        let mut sum = wire::checksum(&ack);
        let mut payload = ack.to_vec();
        for fragment in fragments {
            sum = sum
                .wrapping_add(wire::checksum(&fragment[..16]))
                .wrapping_add(wire::checksum(&fragment[16..]));
            payload.extend(fragment);
        }
        let mut header = Header {
            sequence: 2,
            flags: wire::ACK | wire::BLOB_FRAGMENTS | wire::ENCRYPTED_CHECKSUM,
            recipient: 1,
            iteration: 1,
            payload_len: payload.len() as u16,
            ..Default::default()
        };
        header.checksum = header
            .header_checksum()
            .wrapping_add(sum ^ b.incoming.clone().next_key());
        let mut packet = header.encode().to_vec();
        packet.extend(payload);
        assert!(matches!(
            b.receive(&packet, now),
            Err(TransportError::Fragment)
        ));
        assert_eq!(b.pending.len(), 1);
        assert!(Arc::ptr_eq(&b.pending[&2].bytes, &retry_bytes));
        assert_eq!(b.generated, 1);
        assert!(b.assemblies.is_empty());
        assert_eq!(
            b.retransmit_due(now + Duration::from_secs(1)).unwrap(),
            outgoing
        );
        let packet = a
            .send(2, 4, b"valid after rollback", now)
            .unwrap()
            .remove(0);
        assert_eq!(
            b.receive(&packet, now).unwrap().messages[0].data,
            b"valid after rollback"
        );
    }
    #[test]
    fn logoff_requires_exact_control_and_current_session_identity() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let header = Header {
            flags: wire::LOGOFF,
            recipient: 1,
            iteration: 1,
            ..Default::default()
        };
        let logoff = wire::encode_control_packet(header, &[], None).unwrap();
        for length in 0..logoff.len() {
            assert!(b.receive(&logoff[..length], now).is_err());
        }
        for header in [
            Header {
                recipient: 2,
                ..header
            },
            Header {
                iteration: 2,
                ..header
            },
        ] {
            let packet = wire::encode_control_packet(header, &[], None).unwrap();
            assert!(b.receive(&packet, now).is_err());
        }
        let mut corrupt = logoff.clone();
        corrupt[8] ^= 1;
        assert!(b.receive(&corrupt, now).is_err());
        for header in [
            Header {
                sequence: 2,
                ..header
            },
            Header {
                flags: wire::LOGOFF | wire::ACK,
                ..header
            },
            Header {
                flags: wire::LOGOFF | wire::ENCRYPTED_CHECKSUM,
                ..header
            },
        ] {
            assert!(wire::encode_control_packet(header, &[], None).is_err());
            assert!(wire::decode_session_packet(&header.encode(), None).is_err());
        }
        assert!(wire::encode_control_packet(header, &[0], None).is_err());
        let received = b.receive(&logoff, now).unwrap();
        assert!(received.logoff);
        assert!(received.messages.is_empty());
        assert_eq!(received.ack, 1);
        assert_eq!(b.generated, 1);
        // Connection removal belongs to the caller. The transport itself must
        // not advance sequence or ISAAC state on this unsequenced control.
        let packet = a.send(1, 2, b"unchanged state", now).unwrap().remove(0);
        assert_eq!(
            b.receive(&packet, now).unwrap().messages[0].data,
            b"unchanged state"
        );
    }
    #[test]
    fn plain_controls_do_not_consume_reliable_sequence_or_keys() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let control = wire::encode_control_packet(
            Header {
                sequence: 1,
                flags: wire::ACK,
                recipient: 1,
                iteration: 1,
                ..Default::default()
            },
            &1u32.to_le_bytes(),
            None,
        )
        .unwrap();
        assert_eq!(b.receive(&control, now).unwrap().ack, 1);
        assert_eq!(b.generated, 1);
        let packet = a.send(1, 2, b"first reliable", now).unwrap().remove(0);
        assert_eq!(Header::decode(&packet).unwrap().sequence, 2);
        assert_eq!(
            b.receive(&packet, now).unwrap().messages[0].data,
            b"first reliable"
        );
    }
    #[test]
    fn send_capacity_failure_leaves_key_stream_unchanged() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        assert!(a.send(1, 2, &vec![0; MAX_MESSAGE + 1], now).is_err());
        let packet = a.send(2, 2, b"after rejection", now).unwrap().remove(0);
        assert_eq!(
            b.receive(&packet, now).unwrap().messages[0].data,
            b"after rejection"
        );
    }
    #[test]
    fn ordered_messages_share_stamp_across_queues() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let first = a
            .send((0x01000001u64 << 32) | 1, 3, b"first", now)
            .unwrap()
            .remove(0);
        let second = a
            .send((0x01000002u64 << 32) | 2, 7, b"second", now)
            .unwrap()
            .remove(0);
        assert!(b.receive(&second, now).unwrap().messages.is_empty());
        let ready = b.receive(&first, now).unwrap().messages;
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].data, b"first");
        assert_eq!(ready[1].data, b"second");
    }
    #[test]
    fn ordered_counter_wraps_with_reordering_and_duplicates() {
        let (mut a, mut b) = pair();
        b.next_ordered = 65535;
        let now = Instant::now();
        let first = a.send((0x0100ffffu64 << 32) | 1, 3, b"before wrap", now).unwrap().remove(0);
        let second = a.send((0x01000000u64 << 32) | 2, 3, b"after wrap", now).unwrap().remove(0);
        assert!(b.receive(&second, now).unwrap().messages.is_empty());
        let ready = b.receive(&first, now).unwrap().messages;
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].data, b"before wrap");
        assert_eq!(ready[1].data, b"after wrap");
        assert_eq!(b.next_ordered, 1);
        assert!(b.receive(&first, now).unwrap().messages.is_empty());
    }
    #[test]
    fn enter_world_restarts_the_expected_ordered_stamp() {
        // After a log off the client's second world entry restarts its ordered stamps at 1
        // (`ClientNet::EnterWorld`); the `EnterWorld` event (queue 4) restarts the expectation, or
        // the entry's ordered blobs would be dropped as old.
        let (mut a, mut b) = pair();
        let now = Instant::now();
        for stamp in 1..=3u64 {
            let packet = a
                .send((0x01000000u64 << 32) | (stamp << 32) | stamp, 3, b"ordered", now)
                .unwrap()
                .remove(0);
            assert_eq!(b.receive(&packet, now).unwrap().messages.len(), 1);
        }
        assert_eq!(b.next_ordered, 4);
        let mut enter = crate::world::ENTER_WORLD.to_le_bytes().to_vec();
        enter.extend_from_slice(b"rest");
        let packet = a.send(100, 4, &enter, now).unwrap().remove(0);
        assert_eq!(b.receive(&packet, now).unwrap().messages.len(), 1);
        assert_eq!(b.next_ordered, 1);
        let packet = a
            .send((0x01000000u64 << 32) | (1u64 << 32) | 101, 3, b"scene", now)
            .unwrap()
            .remove(0);
        let ready = b.receive(&packet, now).unwrap().messages;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].data, b"scene");
    }
    #[test]
    fn plain_application_fragments_rejected_without_state_change() {
        let (_, mut b) = pair();
        let now = Instant::now();
        let packet = wire::encode_fragment_packet(
            Header {
                sequence: 2,
                flags: wire::BLOB_FRAGMENTS,
                recipient: 1,
                iteration: 1,
                ..Default::default()
            },
            Fragment {
                blob_id: 1,
                count: 1,
                index: 0,
                queue: 2,
                data: b"plain",
            },
        )
        .unwrap();
        assert!(b.receive(&packet, now).is_err());
        assert_eq!(b.contiguous, 1);
        assert_eq!(b.generated, 1);
    }
}
