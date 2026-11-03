//! MC-DPL8R 2.2.1.1/2/5, 2.2.2 and 2.2.3. All multi-byte fields are LE.
use super::{Error, MAX_DATAGRAM, MAX_MESSAGE, RECEIVE_WINDOW};

pub const VERSION_15: u32 = 0x0001_0005;
pub const VERSION_16: u32 = 0x0001_0006;
pub const DATA: u8 = 0x01;
pub const RELIABLE: u8 = 0x02;
pub const SEQUENTIAL: u8 = 0x04;
pub const POLL: u8 = 0x08;
pub const NEW: u8 = 0x10;
pub const END: u8 = 0x20;
pub const USER_FLAGS: u8 = 0xc0;
pub const RETRY: u8 = 0x01;
pub const KEEPALIVE: u8 = 0x02;
pub const COALESCE: u8 = 0x04;
pub const END_STREAM: u8 = 0x08;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Masks {
    pub sack: [Option<u32>; 2],
    pub send: [Option<u32>; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Connect,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    pub kind: ConnectionKind,
    pub poll: bool,
    pub message_id: u8,
    pub response_id: u8,
    pub version: u32,
    pub session: u32,
    pub timestamp: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sack {
    pub poll: bool,
    pub response: bool,
    pub retry: u8,
    pub next_send: u8,
    pub next_receive: u8,
    pub timestamp: u32,
    pub masks: Masks,
}

// Deliberately no Debug: a frame can contain a ticket or private chat.
#[derive(Clone, Copy)]
pub struct DataFrame<'a> {
    pub command: u8,
    pub control: u8,
    pub sequence: u8,
    pub next_receive: u8,
    pub masks: Masks,
    pub session: Option<u32>,
    pub payload: &'a [u8],
}

pub enum Frame<'a> {
    Connection(Connection),
    Sack(Sack),
    Data(DataFrame<'a>),
}

fn take_u32(data: &[u8], pos: &mut usize) -> Result<u32, Error> {
    let bytes: [u8; 4] = data
        .get(*pos..*pos + 4)
        .ok_or(Error::Truncated)?
        .try_into()
        .map_err(|_| Error::Truncated)?;
    *pos += 4;
    Ok(u32::from_le_bytes(bytes))
}

fn read_masks(data: &[u8], pos: &mut usize, flags: u8, first: u8) -> Result<Masks, Error> {
    let mut words = [None; 4];
    for (i, word) in words.iter_mut().enumerate() {
        if flags & (first << i) != 0 {
            *word = Some(take_u32(data, pos)?);
        }
    }
    Ok(Masks {
        sack: [words[0], words[1]],
        send: [words[2], words[3]],
    })
}

/// Decode an unsigned connection. Callers must select the negotiated version;
/// signed connections need signature validation before using this decoder.
pub fn decode(data: &[u8], version: u32) -> Result<Frame<'_>, Error> {
    if data.len() > MAX_DATAGRAM {
        return Err(Error::Limit);
    }
    if data.len() < 4 {
        return Err(Error::Truncated);
    }
    if version >> 16 != 1 {
        return Err(Error::Version);
    }
    let command = data[0];
    // USER_2 shares CFRAME's bit; DATA takes precedence (MC-DPL8R 2.2.2).
    if command & DATA != 0 {
        let control = data[1];
        let mut pos = 4;
        let masks = read_masks(data, &mut pos, control, 0x10)?;
        let session = if version >= VERSION_15 && control & KEEPALIVE != 0 {
            if command & (RELIABLE | SEQUENTIAL | END) != RELIABLE | SEQUENTIAL | END
                || control & COALESCE != 0
            {
                return Err(Error::Flags);
            }
            let session = take_u32(data, &mut pos)?;
            if pos != data.len() {
                return Err(Error::Length);
            }
            Some(session)
        } else {
            None
        };
        if control & COALESCE != 0 {
            if version < VERSION_15 || command & (NEW | END) != NEW | END {
                return Err(Error::Flags);
            }
            coalesced(&data[pos..])?;
        }
        return Ok(Frame::Data(DataFrame {
            command,
            control,
            sequence: data[2],
            next_receive: data[3],
            masks,
            session,
            payload: &data[pos..],
        }));
    }
    if command & !POLL != 0x80 {
        return Err(Error::Flags);
    }
    match data[1] {
        1 | 2 => {
            if data.len() != 16 {
                return Err(Error::Length);
            }
            let mut pos = 4;
            let version = take_u32(data, &mut pos)?;
            let session = take_u32(data, &mut pos)?;
            if version >> 16 != 1 {
                return Err(Error::Version);
            }
            if version >= VERSION_15 && session == 0 {
                return Err(Error::Session);
            }
            Ok(Frame::Connection(Connection {
                kind: if data[1] == 1 {
                    ConnectionKind::Connect
                } else {
                    ConnectionKind::Connected
                },
                poll: command & POLL != 0,
                message_id: data[2],
                response_id: data[3],
                version,
                session,
                timestamp: take_u32(data, &mut pos)?,
            }))
        }
        6 => {
            if data.len() < 12 {
                return Err(Error::Truncated);
            }
            if data[2] & !0x1f != 0 {
                return Err(Error::Flags);
            }
            let mut pos = 8;
            let timestamp = take_u32(data, &mut pos)?;
            let masks = read_masks(data, &mut pos, data[2], 0x02)?;
            if pos != data.len() {
                return Err(Error::Length);
            }
            Ok(Frame::Sack(Sack {
                poll: command & POLL != 0,
                response: data[2] & 1 != 0,
                retry: data[3],
                next_send: data[4],
                next_receive: data[5],
                timestamp,
                masks,
            }))
        }
        // Signed CONNECTED and HARD_DISCONNECT have distinct layouts. Never
        // interpret them as unsigned CONNECTED or application bytes.
        _ => Err(Error::UnsupportedCommand),
    }
}

impl Connection {
    pub fn encode(self) -> Result<Vec<u8>, Error> {
        let mut out = vec![
            0x80 | if self.poll { POLL } else { 0 },
            if self.kind == ConnectionKind::Connect {
                1
            } else {
                2
            },
            self.message_id,
            if self.kind == ConnectionKind::Connect {
                0
            } else {
                self.response_id
            },
        ];
        for value in [self.version, self.session, self.timestamp] {
            out.extend(value.to_le_bytes());
        }
        decode(&out, self.version)?;
        Ok(out)
    }
}

impl Sack {
    pub fn encode(self) -> Vec<u8> {
        let mut out = vec![
            0x80 | if self.poll { POLL } else { 0 },
            6,
            u8::from(self.response),
            self.retry,
            self.next_send,
            self.next_receive,
            0,
            0,
        ];
        out.extend(self.timestamp.to_le_bytes());
        for (i, word) in self
            .masks
            .sack
            .into_iter()
            .chain(self.masks.send)
            .enumerate()
        {
            if let Some(word) = word {
                out[2] |= 2 << i;
                out.extend(word.to_le_bytes());
            }
        }
        out
    }
}

/// Split an application message across reliable ordered DFRAMEs. The GUN opcode
/// belongs to the message, so it appears once, not again in each continuation.
pub fn fragment(
    payload: &[u8],
    user_flags: u8,
    mtu: usize,
    sequence: u8,
    next_receive: u8,
) -> Result<Vec<Vec<u8>>, Error> {
    if user_flags & !USER_FLAGS != 0 {
        return Err(Error::Flags);
    }
    if !(5..=MAX_DATAGRAM).contains(&mtu) || payload.len() > MAX_MESSAGE {
        return Err(Error::Limit);
    }
    let count = payload.len().max(1).div_ceil(mtu - 4);
    if count > usize::from(RECEIVE_WINDOW) {
        return Err(Error::Limit);
    }
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let last = index + 1 == count;
        let command = DATA
            | RELIABLE
            | SEQUENTIAL
            | user_flags
            | if index == 0 { NEW } else { 0 }
            | if last { END | POLL } else { 0 };
        let mut frame = vec![command, 0, sequence.wrapping_add(index as u8), next_receive];
        let start = index * (mtu - 4);
        frame.extend(&payload[start..payload.len().min(start + mtu - 4)]);
        frames.push(frame);
    }
    Ok(frames)
}

/// Borrow coalesced messages, excluding all alignment padding.
pub fn coalesced(payload: &[u8]) -> Result<Vec<(u8, &[u8])>, Error> {
    let mut headers = Vec::new();
    let mut pos = 0;
    loop {
        let header = payload.get(pos..pos + 2).ok_or(Error::Truncated)?;
        pos += 2;
        headers.push((
            header[1],
            usize::from(header[0]) | (usize::from(header[1] & 0x38) << 5),
        ));
        if header[1] & 1 != 0 {
            break;
        }
        if headers.len() == 32 {
            return Err(Error::Limit);
        }
    }
    pos = (pos + 3) & !3;
    let mut messages = Vec::new();
    let count = headers.len();
    for (index, (command, size)) in headers.into_iter().enumerate() {
        let bytes = payload.get(pos..pos + size).ok_or(Error::Truncated)?;
        messages.push((command, bytes));
        pos += size;
        if index + 1 < count {
            pos = (pos + 3) & !3;
        }
    }
    if pos != payload.len() {
        return Err(Error::Length);
    }
    Ok(messages)
}
