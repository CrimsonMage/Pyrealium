//! Bounded receive ordering for the reliable sequential delivery used by GUN.
//! No retransmission timer or sender ACK processing is implied by this module.
use super::frame::{
    self, COALESCE, DataFrame, END, END_STREAM, NEW, RELIABLE, SEQUENTIAL, USER_FLAGS,
};
use super::{Error, MAX_DATAGRAM, MAX_MESSAGE, RECEIVE_WINDOW};
use std::collections::BTreeMap;

pub struct Message {
    pub user_flags: u8,
    pub payload: Vec<u8>,
    pub fragments: usize,
}

pub struct Delivery {
    pub messages: Vec<Message>,
    pub duplicate: bool,
    pub closed: bool,
}

#[derive(PartialEq, Eq)]
struct Pending {
    flags: u8,
    control: u8,
    keepalive: bool,
    payload: Vec<u8>,
}

pub struct Receiver {
    next: u8,
    session: Option<u32>,
    pending: BTreeMap<u8, Pending>,
    partial: Option<Message>,
    failed: bool,
    closed: bool,
}

impl Receiver {
    /// A live session starts at sequence 0. Offline captures can start midstream;
    /// their caller must explicitly supply the first observed sequence number.
    pub fn new(next: u8, session: Option<u32>) -> Self {
        Self {
            next,
            session,
            pending: BTreeMap::new(),
            partial: None,
            failed: false,
            closed: false,
        }
    }
    pub fn next_receive(&self) -> u8 {
        self.next
    }
    pub fn buffered_frames(&self) -> usize {
        self.pending.len()
    }
    pub fn incomplete_message(&self) -> bool {
        self.partial.is_some()
    }

    pub fn accept(&mut self, data: DataFrame<'_>) -> Result<Delivery, Error> {
        if self.failed || self.closed {
            return Err(Error::FailedStream);
        }
        if data.payload.len() > MAX_DATAGRAM {
            return Err(Error::Limit);
        }
        if data.command & (RELIABLE | SEQUENTIAL) != RELIABLE | SEQUENTIAL {
            return Err(Error::UnsupportedDelivery);
        }
        if let (Some(expected), Some(actual)) = (self.session, data.session) {
            if expected != actual {
                return Err(Error::Session);
            }
        }
        let distance = data.sequence.wrapping_sub(self.next);
        // Half-range modular comparison handles 255 -> 0. A receive window
        // smaller than 128 prevents ambiguous ordering and bounds memory.
        if distance >= 128 {
            return Ok(Delivery {
                messages: Vec::new(),
                duplicate: true,
                closed: false,
            });
        }
        if distance >= RECEIVE_WINDOW {
            return Err(Error::Window);
        }
        let pending = Pending {
            flags: data.command & (RELIABLE | SEQUENTIAL | NEW | END | USER_FLAGS),
            control: data.control & (COALESCE | END_STREAM),
            keepalive: data.session.is_some(),
            payload: data.payload.to_vec(),
        };
        if let Some(previous) = self.pending.get(&data.sequence) {
            if *previous != pending {
                return self.fail(Error::ConflictingRetry);
            }
            return Ok(Delivery {
                messages: Vec::new(),
                duplicate: true,
                closed: false,
            });
        }
        self.pending.insert(data.sequence, pending);
        let mut result = Delivery {
            messages: Vec::new(),
            duplicate: false,
            closed: false,
        };
        while let Some(frame) = self.pending.remove(&self.next) {
            if let Err(error) = self.consume(&frame, &mut result.messages) {
                return self.fail(error);
            }
            self.next = self.next.wrapping_add(1);
            if frame.control & END_STREAM != 0 {
                if self.partial.is_some() {
                    return self.fail(Error::MissingEnd);
                }
                self.closed = true;
                self.pending.clear();
                result.closed = true;
                break;
            }
        }
        Ok(result)
    }

    fn fail<T>(&mut self, error: Error) -> Result<T, Error> {
        self.failed = true;
        self.pending.clear();
        self.partial = None;
        Err(error)
    }

    fn consume(&mut self, frame: &Pending, messages: &mut Vec<Message>) -> Result<(), Error> {
        if frame.keepalive {
            return Ok(());
        }
        if frame.control & COALESCE != 0 {
            if self.partial.is_some() {
                return Err(Error::MissingEnd);
            }
            for (command, payload) in frame::coalesced(&frame.payload)? {
                if command & (RELIABLE | SEQUENTIAL) != RELIABLE | SEQUENTIAL {
                    return Err(Error::UnsupportedDelivery);
                }
                messages.push(Message {
                    user_flags: command & USER_FLAGS,
                    payload: payload.to_vec(),
                    fragments: 1,
                });
            }
            return Ok(());
        }
        if frame.flags & NEW != 0 {
            if self.partial.is_some() {
                return Err(Error::MissingEnd);
            }
            self.partial = Some(Message {
                user_flags: frame.flags & USER_FLAGS,
                payload: Vec::new(),
                fragments: 0,
            });
        }
        let partial = self.partial.as_mut().ok_or(Error::MissingStart)?;
        if partial.user_flags != frame.flags & USER_FLAGS {
            return Err(Error::FragmentFlags);
        }
        if partial.payload.len() + frame.payload.len() > MAX_MESSAGE {
            return Err(Error::Limit);
        }
        partial.payload.extend(&frame.payload);
        partial.fragments += 1;
        if frame.flags & END != 0 {
            messages.push(self.partial.take().ok_or(Error::MissingStart)?);
        }
        Ok(())
    }
}
