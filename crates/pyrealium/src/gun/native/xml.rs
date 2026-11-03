//! GUN application opcode 0xC350, a NUL-terminated XML string, and an optional
//! opaque eight-byte trailer observed on retail server messages. The trailer's
//! purpose is not established; parsing it does not validate any signature.
use super::MAX_MESSAGE;
use crate::gun::Elem;

pub struct Message {
    pub element: Elem,
    pub trailer: Option<[u8; 8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Length,
    Opcode,
    Terminator,
    Trailer,
    Utf8,
    Xml,
}

pub fn decode(payload: &[u8]) -> Result<Message, Error> {
    if payload.len() > MAX_MESSAGE {
        return Err(Error::Length);
    }
    if !payload.starts_with(&[0x50, 0xc3]) {
        return Err(Error::Opcode);
    }
    let content = &payload[2..];
    let end = content
        .iter()
        .position(|&b| b == 0)
        .ok_or(Error::Terminator)?;
    let trailer = match &content[end + 1..] {
        [] => None,
        bytes if bytes.len() == 8 => Some(bytes.try_into().map_err(|_| Error::Trailer)?),
        _ => return Err(Error::Trailer),
    };
    let text = std::str::from_utf8(&content[..end]).map_err(|_| Error::Utf8)?;
    Ok(Message {
        element: Elem::parse(text).map_err(|_| Error::Xml)?,
        trailer,
    })
}

/// Encode the observed trailer-free variant. No guessed trailer is generated.
pub fn encode(element: &Elem) -> Result<Vec<u8>, Error> {
    let text = element.encode();
    if text.len() > MAX_MESSAGE - 3 {
        return Err(Error::Length);
    }
    // Validate caller-constructed element names and XML structure before emitting.
    Elem::parse(&text).map_err(|_| Error::Xml)?;
    let mut out = vec![0x50, 0xc3];
    out.extend(text.bytes());
    out.push(0);
    Ok(out)
}
