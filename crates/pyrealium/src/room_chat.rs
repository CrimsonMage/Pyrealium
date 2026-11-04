//! Room chat over Turbine chat (`0x100D3`): the fellowship's `/f` and the channels' `/cg` and
//! `/trade`.
//!
//! A character set with the Turbine chat flag (retail's, every Legions-era `CharacterSet` of the
//! captures) makes the client send room chat through its chat client (`chatclient.dll`,
//! `Turbine::Client::StartupTurbineChat`, admin 0x00722600) instead of GUN:
//! `gmCWInterface::RequestGUN_SendToRoom` -> `CSendToTurbineRoomByID` ->
//! `ChatInterfaceProvider::SendData`, the event `0x100D3`, a u32 length and the chat packet, on
//! queue 4 (AC2_Rusted ac2-net `protocol/turbine_chat.rs`, `pui.rs`).
//!
//! A chat packet is seven u32s (blob type, dispatch type, target type, target id, transport
//! type, transport id, cookie), a u32 payload size and the payload. Strings are an
//! `SB_As32Bit_Compressed` unit count and UTF-16 units, unaligned.
//!
//! Retail (2005 `group.chat.elog`, `group.chat.2.elog`, 25 sends; the 2017-02-12 captures,
//! thousands of events): the request is `(3, 2, 1, 0, 0, 0, 0)`, then the context, the method 2
//! twice, the room, the text and the sender's `GunChatExtraInfo` blob. The server answered every
//! member of the room, the sender too, with the event `(1, 1, 1, id, 1, id, 0)`: the room, the
//! speaker's name, the text and the sender's blob unchanged; then the sender alone with the
//! response `(5, 2, 1, id, 1, id, 0)`: the context, 2, 2 and result 0. `id` is the receiving
//! connection's (2005: `0x00DD0001`, `0x00DD0007`; 2017: `0x00F10017`; the server's byte, then
//! the connection's number) and the server's chat blobs use the unordered `0x23` id space.
//!
//! Rooms: a fellowship's room is field 3 of its `Fellowship` package (`groups`; the captures
//! number them from one server-wide counter, `0x3A`..`0x3F` in successive 2005 fellowships), and
//! the channels' are the `ChannelData` table `0x8205001B` ([`channel_table`]).
//!
//! Pyrealium policy: the connection number is Pyrealium's transport id with server byte 0 (the
//! client does not read it); General and Trade 1 are every player's (see [`channel_table`]);
//! a request for a room the sender is not in, or a blob that is not a version-1
//! `GunChatExtraInfo` with misc flags 0 (the only kind the client builds; flag 1 hides the
//! speaker's name), is dropped without a response, as no retail refusal code is known.

use crate::world::WorldError;

pub const EVENT: u32 = 0x100d3;
/// `WM_Communication` 0x1B -> `CPlayer` 0x147: the channel table.
pub const CHANNEL_TABLE: u32 = 0x8205_001b;
/// Every retail table's General (channel `0x80000`) is room 1.
pub const GENERAL: u32 = 1;
/// Trade 1 (channel `0x100000`) was room 8 on most 2005 servers (7 on some).
pub const TRADE: u32 = 8;
/// Fellowship rooms follow the channels' (retail's dynamic rooms, regions and fellowships,
/// came from one counter above them).
pub const FIRST_DYNAMIC_ROOM: u32 = 9;
/// Pyrealium's limit on a line (the client's chat entry is shorter).
pub const MAX_TEXT_UNITS: usize = 512;
const MAX_EXTRA: usize = 256;
/// The world's name in the channels' names (retail: the shard's, "Abendgrau General").
pub const WORLD: &str = "Pyrealium";

const REQUEST_BINARY: u32 = 3;
const SEND_TO_ROOM_BY_ID: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomRequest {
    pub context: u32,
    pub room: u32,
    pub text: String,
    /// The sender's packed `GunChatExtraInfo`, relayed unchanged.
    pub extra: Vec<u8>,
}

struct In<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WorldError> {
        let end = self.pos.checked_add(n).ok_or(WorldError::LimitExceeded)?;
        let slice = self.bytes.get(self.pos..end).ok_or(WorldError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }
    fn u32(&mut self) -> Result<u32, WorldError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn compressed(&mut self) -> Result<u32, WorldError> {
        let b0 = u32::from(self.take(1)?[0]);
        if b0 & 0x80 == 0 {
            return Ok(b0);
        }
        let b1 = u32::from(self.take(1)?[0]);
        if b0 & 0x40 == 0 {
            return Ok((b0 & 0x7f) << 8 | b1);
        }
        let low = u32::from(u16::from_le_bytes(self.take(2)?.try_into().unwrap()));
        Ok(((b0 & 0x3f) << 8 | b1) << 16 | low)
    }
    fn string(&mut self) -> Result<String, WorldError> {
        let units = self.compressed()? as usize;
        if units > MAX_TEXT_UNITS {
            return Err(WorldError::LimitExceeded);
        }
        let data = self.take(units * 2)?;
        let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16(&units).map_err(|_| WorldError::InvalidValue)
    }
}

impl RoomRequest {
    /// The chat packet (the bytes after `0x100D3` and its length).
    pub fn decode(packet: &[u8]) -> Result<Self, WorldError> {
        let mut r = In { bytes: packet, pos: 0 };
        let header: Vec<u32> = (0..7).map(|_| r.u32()).collect::<Result<_, _>>()?;
        if header != [REQUEST_BINARY, SEND_TO_ROOM_BY_ID, 1, 0, 0, 0, 0] {
            return Err(WorldError::InvalidValue);
        }
        let size = r.u32()? as usize;
        if size != packet.len() - r.pos {
            return Err(WorldError::InvalidValue);
        }
        let context = r.u32()?;
        if r.u32()? != SEND_TO_ROOM_BY_ID || r.u32()? != SEND_TO_ROOM_BY_ID {
            return Err(WorldError::InvalidValue);
        }
        let room = r.u32()?;
        let text = r.string()?;
        let extra_size = r.u32()? as usize;
        if extra_size > MAX_EXTRA {
            return Err(WorldError::LimitExceeded);
        }
        let extra = r.take(extra_size)?.to_vec();
        if r.pos != packet.len() {
            return Err(WorldError::TrailingBytes);
        }
        if room == 0 || text.is_empty() {
            return Err(WorldError::InvalidValue);
        }
        Ok(Self { context, room, text, extra })
    }

    /// The blob is what the client builds (`GunChatExtraInfo::GunChatExtraInfo` and
    /// `StreamPack`): version 1, misc flags 0, the weenie flags, a 4-aligned string.
    pub fn extra_is_the_clients(&self) -> bool {
        let e = &self.extra;
        if e.len() < 12 || e.len() % 4 != 0 || e[0..2] != [1, 0] || e[2..4] != [0, 0] {
            return false;
        }
        let units = usize::from(u16::from_le_bytes([e[8], e[9]]));
        (10 + units * 2).div_ceil(4) * 4 == e.len().max(12)
    }
}

/// Who hears `request` from `sender`, with the deliveries (each member's event, then the
/// sender's response); `None` when it is dropped (module notes).
pub fn relay(
    sender: u64,
    speaker: &str,
    request: &RoomRequest,
    groups: &crate::groups::Runtime,
    online: &[u64],
) -> Result<Option<Vec<(u64, Vec<u8>)>>, WorldError> {
    let members = match request.room {
        GENERAL | TRADE => Some(online.to_vec()),
        room if groups.room(sender) == Some(room) => groups.room_members(room),
        _ => None,
    };
    let Some(members) = members.filter(|_| request.extra_is_the_clients()) else {
        return Ok(None);
    };
    let event = heard(request.room, speaker, &request.text, &request.extra)?;
    let mut out: Vec<(u64, Vec<u8>)> = members.into_iter().map(|m| (m, event.clone())).collect();
    out.push((sender, sent(request.context)));
    Ok(Some(out))
}

fn header(blob_type: u32, dispatch: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(payload.len() + 40);
    let size = (32 + payload.len()) as u32;
    for word in [EVENT, size, blob_type, dispatch, 1, 0, 1, 0, 0, payload.len() as u32] {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(payload);
    bytes
}
fn string(out: &mut Vec<u8>, text: &str) -> Result<(), WorldError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > MAX_TEXT_UNITS {
        return Err(WorldError::LimitExceeded);
    }
    // Every length here is below 0x80 or 0x4000: one or two bytes.
    let n = units.len() as u32;
    if n < 0x80 {
        out.push(n as u8);
    } else {
        out.extend([(n >> 8) as u8 | 0x80, n as u8]);
    }
    out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    Ok(())
}

/// The room event every member hears (target ids left 0; see [`addressed`]).
pub fn heard(room: u32, speaker: &str, text: &str, extra: &[u8]) -> Result<Vec<u8>, WorldError> {
    let mut payload = room.to_le_bytes().to_vec();
    string(&mut payload, speaker)?;
    string(&mut payload, text)?;
    payload.extend((extra.len() as u32).to_le_bytes());
    payload.extend(extra);
    Ok(header(1, 1, &payload))
}

/// The sender's response: the context, 2, 2 and the result (retail: always 0).
pub fn sent(context: u32) -> Vec<u8> {
    let payload: Vec<u8> = [context, 2, 2, 0].into_iter().flat_map(u32::to_le_bytes).collect();
    header(5, 2, &payload)
}

pub fn is_chat(data: &[u8]) -> bool {
    data.get(..4) == Some(&EVENT.to_le_bytes()[..])
}

/// `event` with the receiving connection's id as target and transport id.
pub fn addressed(event: &[u8], connection: u32) -> Vec<u8> {
    let mut bytes = event.to_vec();
    if is_chat(&bytes) && bytes.len() >= 40 {
        bytes[20..24].copy_from_slice(&connection.to_le_bytes());
        bytes[28..32].copy_from_slice(&connection.to_le_bytes());
    }
    bytes
}

/// `0x8205001B`, sent after login as retail did (with the allegiance calls `0x8203002E` and
/// `0x82030030`, before the first world objects): a `0x11` hash, channel -> `ChannelData`
/// (`0x2023`: seven fields, field 3 the room, field 6 the name). Every retail table holds
/// "<world> General" (channel `0x80000`, room 1) and "<world> Trade 1" (`0x100000`, room 8; field
/// 2 the trade number); "Region N", "PvP" and "Faction" depended on the player's place and
/// faction and are not sent (Pyrealium has neither). Fields and their tags are the captured ones
/// (`__Packviewer_Test_File.elog`, Abendgrau). Pyrealium policy: the world's name is `world`.
pub fn channel_table(world: &str) -> Result<Vec<u8>, WorldError> {
    use crate::groups::{graph, Object};
    let channels = [
        (0x8_0000u32, [(1, 4), (0x8_0000, 2), (0, 2), (GENERAL, 4), (1, 2), (0, 2)], "General"),
        (0x10_0000, [(0, 2), (0x10_0000, 2), (1, 4), (TRADE, 2), (1, 2), (0, 2)], "Trade 1"),
    ];
    let mut hash = Vec::new();
    hash.extend((0x0200_0000u32 | channels.len() as u32).to_le_bytes());
    let mut objects = vec![];
    for (index, (channel, ..)) in channels.iter().enumerate() {
        hash.extend(channel.to_le_bytes());
        hash.extend((2 + index as u32 * 2).to_le_bytes());
    }
    objects.push(Object::native(0x11, hash));
    for (index, (_, fields, name)) in channels.iter().enumerate() {
        let mut fields = fields.to_vec();
        fields.push((3 + index as u32 * 2, 1));
        objects.push(Object::interpreted(0x2023, fields));
        let literal = crate::world::literal_name(&format!("{world} {name}"))?;
        objects.push(Object::native(0x76, literal[12..].to_vec()));
    }
    crate::script::private_call(CHANNEL_TABLE, &graph(&objects))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }
    // group.chat.elog: Xe Mon's "Hallo Welt" (context 0x3D) to the fellowship's room 0x3F, the
    // event it heard back and the response, connection 0x00DD0001.
    const REQUEST: &str = "d3000100550000000300000002000000010000000000000000000000000000000000000035000000\
3d00000002000000020000003f0000000a480061006c006c006f002000570065006c0074000c00000001000000000000000100d400";
    const HEARD: &str = "d3000100560000000100000001000000010000000100dd00010000000100dd000000000036000000\
3f000000065800650020004d006f006e000a480061006c006c006f002000570065006c0074000c00000001000000000000000100d400";
    const SENT: &str = "d3000100300000000500000002000000010000000100dd00010000000100dd0000000000100000003d000000020000000200000000000000";

    #[test]
    fn the_captured_send_decodes() {
        let wire = hex(REQUEST);
        let event = crate::world::decode_client_event(&wire).unwrap();
        let crate::world::ClientEvent::RoomChat(request) = event else { panic!("{event:?}") };
        assert_eq!((request.context, request.room, request.text.as_str()), (0x3d, 0x3f, "Hallo Welt"));
        assert_eq!(request.extra, hex("01000000000000000100d400"));
        assert!(request.extra_is_the_clients());
        // Truncated, trailing, by name, or a misc flag that hides the speaker.
        assert!(crate::world::decode_client_event(&wire[..wire.len() - 1]).is_err());
        let mut by_name = wire.clone();
        by_name[12] = 1;
        assert!(crate::world::decode_client_event(&by_name).is_err());
        let mut hidden = request.clone();
        hidden.extra[2] = 1;
        assert!(!hidden.extra_is_the_clients());
    }

    #[test]
    fn the_event_and_the_response_are_the_captured_bytes() {
        let request = RoomRequest::decode(&hex(REQUEST)[8..]).unwrap();
        let event = heard(request.room, "Xe Mon", &request.text, &request.extra).unwrap();
        assert_eq!(addressed(&event, 0x00dd_0001), hex(HEARD));
        assert_eq!(addressed(&sent(request.context), 0x00dd_0001), hex(SENT));
        // Not a chat event: unchanged.
        assert_eq!(addressed(b"\x66\x00\x01\x00abcd", 7), b"\x66\x00\x01\x00abcd");
    }

    #[test]
    fn the_channel_table_has_the_captured_channel_data() {
        let table = channel_table("Abendgrau").unwrap();
        // The captured General and Trade 1 ChannelData objects, references renumbered (the
        // capture's heap ids 0x1AB489 and 0x1AB48F are this graph's 3 and 5).
        let general = hex("0001004400000000000023200700000001000000000008000000000001000000010000000000000003000000040202040202010000010044000000007600ffff11002c92a2e52b76cbeb45dd79fc81a653cbece6237fbccdf0f5f4ad8f22f9d4b5b2c600000000");
        let trade = hex("00010044000000000000232007000000000000000000100001000000080000000100000000000000050000000202040202020100");
        let find = |needle: &[u8]| table.windows(needle.len()).any(|w| w == needle);
        assert!(find(&general) && find(&trade));
        // Root: the hash 0x80000 -> 2, 0x100000 -> 4.
        assert!(find(&hex("1100ffff020000020000080002000000000010000400000000000000")));
        assert_eq!(&table[..8], &hex("660001001b000582")[..]);
    }

    #[test]
    fn two_fellows_hear_each_other_and_a_stranger_does_not() {
        use crate::groups::{Member, Request, Runtime};
        const A: u64 = 0x2120_0000_0000_0001;
        const B: u64 = 0x2120_0000_0000_0002;
        const C: u64 = 0x2120_0000_0000_0003;
        let member = |id, name: &str| Member {
            id,
            name: name.into(),
            level: 10,
            health: 1,
            max_health: 1,
            vigor: 1,
            max_vigor: 1,
            cell: 0x7ab1_0010,
        };
        let online: std::collections::BTreeMap<u64, Member> =
            [member(A, "Xe Mon"), member(B, "Faulpelz"), member(C, "Stranger")].into_iter().map(|m| (m.id, m)).collect();
        let mut groups = Runtime::default();
        for (actor, request) in [
            (A, Request::Create { name: "Wanderers".into(), looting: 1, social: false }),
            (A, Request::Invite(B)),
            (B, Request::Accept(A)),
        ] {
            let plan = groups.prepare(actor, &request, 0, &online).unwrap();
            groups.commit(plan).unwrap();
        }
        let room = groups.room(A).unwrap();
        let mut request = RoomRequest::decode(&hex(REQUEST)[8..]).unwrap();
        request.room = room;
        let everyone = [A, B, C];
        let sent_by_a = relay(A, "Xe Mon", &request, &groups, &everyone).unwrap().unwrap();
        let event = heard(room, "Xe Mon", "Hallo Welt", &request.extra).unwrap();
        assert_eq!(
            sent_by_a,
            vec![(A, event.clone()), (B, event), (A, sent(0x3d))]
        );
        // The stranger's line to the fellowship's room goes nowhere; General reaches everyone.
        assert_eq!(relay(C, "Stranger", &request, &groups, &everyone).unwrap(), None);
        request.room = GENERAL;
        let general = relay(C, "Stranger", &request, &groups, &everyone).unwrap().unwrap();
        assert_eq!(general.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![A, B, C, C]);
    }
}
