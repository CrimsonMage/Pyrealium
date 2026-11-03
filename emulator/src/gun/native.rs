//! Native GUN's unsigned DirectPlay 8 framing, independently of the TCP adapter.
//!
//! The recovered 2005 captures use MC-DPL8R versions 1.5/1.6. This module decodes
//! frames and reassembles reliable, sequential messages. It does not authenticate
//! native tickets, implement DirectPlay's core session/name table, or open sockets.
//! See docs/gun-protocol.md for evidence, limits, and the remaining live peer work.

pub mod frame;
pub mod receive;
pub mod xml;

#[cfg(test)]
mod tests;

/// Local resource limits, not claimed retail constants.
pub const MAX_DATAGRAM: usize = 4096;
pub const MAX_MESSAGE: usize = 65536;
pub const RECEIVE_WINDOW: u8 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Length,
    Flags,
    Version,
    UnsupportedCommand,
    UnsupportedDelivery,
    Session,
    Window,
    ConflictingRetry,
    MissingStart,
    MissingEnd,
    FragmentFlags,
    Limit,
    FailedStream,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only enum names: never include captured payloads or authentication data.
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
