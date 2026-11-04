//! Secure trade (`WM_Trade`, functions `0x8215xxxx`): two players, their offers, both
//! acceptances, one swap.
//!
//! The client's requests (AC2_Rusted ac2-net `gameplay/request.rs`, the 7230 player script's
//! `WM_Trade` stubs): `0x2F` open (the other party, a long int), `0x15` offer (amount int, item
//! long int), `0x35` accept (a reference to the client's `Trade` package `0x235`), `0x23`
//! decline (no arguments), `0x3E` close (none), `0x3F` revoke (the item). The server's notices
//! (ac2-net `gameplay/trade.rs` `TradeMessage`): `0x46` registered, `0x4E` opened, `0x44`
//! offered, `0x50` accepted, `0x4F` (unnamed; see below), `0x52` reset, `0x53` closed, `0x51`
//! acceptance cleared, `0x48` revoked.
//!
//! Retail, the 2017-02-12 captures (eight trades in `pkt-acct1-batch4`..`pkt-acct3-batch2`; each
//! shows one side):
//! - the initiator's open names the other party; it gets `0x46` with the other party's name, the
//!   responder and the initiator (`pkt_2017-1-27_1485500185`), and the responder gets the same
//!   with the initiator's name (`pkt_2017-1-23_1485231840`), whose own open of the initiator
//!   then opens the trade: `0x4E` (no arguments).
//! - an offer is answered with `0x44` (amount, item, who offered), the partner's offers reach the
//!   other side the same way; an accept with `0x50` (who); when the second side accepts, `0x52`
//!   follows at once (the items have moved) and the window stays until a side closes: `0x53`
//!   (kind 0, who closed), also to the other side (`pkt_2017-1-27_1485500185`, 13 s later).
//! - a decline after an accept is answered with `0x4F` (who), for either side; a repeated
//!   accept by an accepted side is not answered.
//! - an open nobody answered ends with `0x53` (kind 0, the initiator) (`pkt_2017-1-21_1485040337`).
//! - the accept carries the client's own `Trade` (`0x235`): the responder's offers, the
//!   initiator, the responder, the initiator's and responder's acceptance, a state word (2), and
//!   the initiator's offers.
//!
//! Pyrealium policy: every notice goes to both sides (the captures show both sides' actions
//! reaching each); the names carry no retail suffix (the captured names end in a tag such as
//! " [M]" or " [Mv]" whose source is unknown); an offer or a revoke after an accept clears both
//! acceptances with `0x51` first (the client's handler for it; no capture holds one); a revoke
//! is answered with `0x48` (item, who); the other party must be in view; an accept is taken only
//! when its `Trade` names this trade's two parties (its offers are not compared: any change
//! after an accept clears the acceptances); a party that leaves the world closes the trade as
//! its close does; refusals are Pyrealium's chat lines.

use crate::script;
use crate::world::{self, WorldError};

pub const OPEN: u32 = 0x8215_002f;
pub const OFFER: u32 = 0x8215_0015;
pub const ACCEPT: u32 = 0x8215_0035;
pub const DECLINE: u32 = 0x8215_0023;
pub const CLOSE: u32 = 0x8215_003e;
pub const REVOKE: u32 = 0x8215_003f;

pub const OFFERED: u32 = 0x8215_0044;
pub const REGISTERED: u32 = 0x8215_0046;
pub const REVOKED: u32 = 0x8215_0048;
pub const OPENED: u32 = 0x8215_004e;
/// Captured after a decline of an accepted trade, with who declined (the acceptance lapses).
pub const UNACCEPTED: u32 = 0x8215_004f;
pub const ACCEPTED: u32 = 0x8215_0050;
pub const ACCEPTANCE_CLEARED: u32 = 0x8215_0051;
pub const RESET: u32 = 0x8215_0052;
pub const CLOSED: u32 = 0x8215_0053;

/// Pyrealium's limit on one side's offers.
pub const MAX_OFFERS: usize = 16;
const TRADE_PACKAGE: u32 = 0x0235_0000;

pub fn handles(function: u32) -> bool {
    matches!(function, OPEN | OFFER | ACCEPT | DECLINE | CLOSE | REVOKE)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Open(u64),
    Offer { amount: u32, item: u64 },
    /// The two parties the client's `Trade` names (initiator, responder).
    Accept { initiator: u64, responder: u64 },
    Decline,
    Close,
    Revoke(u64),
}

fn word(bytes: &[u8], at: usize) -> Result<u32, WorldError> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or(WorldError::Truncated)
}
fn long(bytes: &[u8], at: usize) -> Result<u64, WorldError> {
    if word(bytes, at)? != 2 {
        return Err(WorldError::InvalidValue);
    }
    Ok(u64::from(word(bytes, at + 4)?) | u64::from(word(bytes, at + 8)?) << 32)
}

pub fn decode(function: u32, bytes: &[u8]) -> Result<Request, WorldError> {
    let exact = |len: usize| if bytes.len() == len { Ok(()) } else { Err(WorldError::InvalidValue) };
    Ok(match function {
        OPEN => {
            exact(12)?;
            Request::Open(long(bytes, 0)?)
        }
        REVOKE => {
            exact(12)?;
            Request::Revoke(long(bytes, 0)?)
        }
        OFFER => {
            exact(20)?;
            if word(bytes, 0)? != 1 {
                return Err(WorldError::InvalidValue);
            }
            let amount = word(bytes, 4)?;
            if amount == 0 {
                return Err(WorldError::InvalidValue);
            }
            Request::Offer { amount, item: long(bytes, 8)? }
        }
        DECLINE => {
            exact(0)?;
            Request::Decline
        }
        CLOSE => {
            exact(0)?;
            Request::Close
        }
        ACCEPT => {
            // A reference: 5, the object count, the ids, then the root `Trade` (header words
            // 0x44000100, 0, the package), nine fields: offers, initiator, responder, ...
            if word(bytes, 0)? != 5 {
                return Err(WorldError::InvalidValue);
            }
            let count = word(bytes, 4)? as usize;
            if !(1..=64).contains(&count) {
                return Err(WorldError::LimitExceeded);
            }
            let root = 8 + count * 4;
            if word(bytes, root)? != 0x4400_0100 || word(bytes, root + 8)? != TRADE_PACKAGE || word(bytes, root + 12)? != 9 {
                return Err(WorldError::InvalidValue);
            }
            let field = |n: usize| word(bytes, root + 16 + n * 4);
            Request::Accept {
                initiator: u64::from(field(1)?) | u64::from(field(2)?) << 32,
                responder: u64::from(field(3)?) | u64::from(field(4)?) << 32,
            }
        }
        _ => return Err(WorldError::UnsupportedOpcode(function)),
    })
}

/// `0x46`: the other party's name, the responder, the initiator.
pub fn registered(other_name: &str, responder: u64, initiator: u64) -> Result<Vec<u8>, WorldError> {
    let mut args = crate::groups::graph(&[crate::groups::Object::native(0x6a, world::literal_name(other_name)?)]);
    args.extend(script::identity(responder));
    args.extend(script::identity(initiator));
    script::private_call(REGISTERED, &args)
}
pub fn offered(amount: u32, item: u64, source: u64) -> Result<Vec<u8>, WorldError> {
    let mut args = script::integer(amount);
    args.extend(script::identity(item));
    args.extend(script::identity(source));
    script::private_call(OFFERED, &args)
}
pub fn revoked(item: u64, source: u64) -> Result<Vec<u8>, WorldError> {
    let mut args = script::identity(item);
    args.extend(script::identity(source));
    script::private_call(REVOKED, &args)
}
pub fn closed(source: u64) -> Result<Vec<u8>, WorldError> {
    let mut args = script::integer(0);
    args.extend(script::identity(source));
    script::private_call(CLOSED, &args)
}
fn bare(function: u32) -> Result<Vec<u8>, WorldError> {
    script::private_call(function, &[])
}
fn who(function: u32, source: u64) -> Result<Vec<u8>, WorldError> {
    script::private_call(function, &script::identity(source))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Trade {
    initiator: u64,
    responder: u64,
    opened: bool,
    /// Each side's offers (item, amount): [initiator, responder].
    offers: [Vec<(u64, u32)>; 2],
    accepted: [bool; 2],
}
impl Trade {
    fn side(&self, player: u64) -> usize {
        usize::from(player == self.responder)
    }
    fn other(&self, player: u64) -> u64 {
        if player == self.initiator { self.responder } else { self.initiator }
    }
}

/// The items each side gives when both accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Swap {
    pub initiator: u64,
    pub responder: u64,
    pub initiator_gives: Vec<(u64, u32)>,
    pub responder_gives: Vec<(u64, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeError {
    NotAvailable,
    Busy,
    NotTrading,
    NotOpen,
    NotHeld,
    TooMany,
    Wrong,
}
impl TradeError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotAvailable => "That player cannot trade with you now.",
            Self::Busy => "One of you is already trading.",
            Self::NotTrading => "You are not trading.",
            Self::NotOpen => "The trade is not open yet.",
            Self::NotHeld => "You can only offer what is in your pack.",
            Self::TooMany => "You cannot offer more items.",
            Self::Wrong => "That is not part of this trade.",
        }
    }
}

/// What a request does: notices before a swap, the swap, notices after it.
#[derive(Debug, Default)]
pub struct Plan {
    pub before: Vec<(u64, Vec<u8>)>,
    pub swap: Option<Swap>,
    pub after: Vec<(u64, Vec<u8>)>,
}

#[derive(Clone, Debug, Default)]
pub struct Runtime {
    trades: Vec<Trade>,
}

/// What the server knows of the players, for one request.
pub trait World {
    fn name(&self, player: u64) -> Option<String>;
    /// `other` is in the world and in `player`'s view.
    fn reachable(&self, player: u64, other: u64) -> bool;
    /// `player`'s pack holds `amount` of `item`.
    fn holds(&self, player: u64, item: u64, amount: u32) -> bool;
}

fn both(trade: &Trade, data: Vec<u8>) -> [(u64, Vec<u8>); 2] {
    [(trade.initiator, data.clone()), (trade.responder, data)]
}

impl Runtime {
    fn find(&self, player: u64) -> Option<usize> {
        self.trades.iter().position(|t| t.initiator == player || t.responder == player)
    }
    /// Every player in a trade.
    pub fn parties(&self) -> Vec<u64> {
        self.trades.iter().flat_map(|t| [t.initiator, t.responder]).collect()
    }
    pub fn trading(&self, player: u64) -> bool {
        self.find(player).is_some()
    }

    /// `request` from `actor`, applied to this runtime (a caller that must undo it keeps a
    /// clone).
    pub fn apply(&mut self, actor: u64, request: &Request, world: &dyn World) -> Result<Plan, TradeError> {
        let mut plan = Plan::default();
        let wire = |r: Result<Vec<u8>, WorldError>| r.map_err(|_| TradeError::Wrong);
        if let Request::Open(other) = *request {
            if let Some(at) = self.find(actor) {
                // The responder's open of its initiator opens the registered trade.
                let trade = &mut self.trades[at];
                if trade.responder != actor || trade.initiator != other || trade.opened {
                    return Err(TradeError::Busy);
                }
                trade.opened = true;
                plan.before.extend(both(trade, wire(bare(OPENED))?));
                return Ok(plan);
            }
            if other == actor || !world.reachable(actor, other) {
                return Err(TradeError::NotAvailable);
            }
            if self.trading(other) {
                return Err(TradeError::Busy);
            }
            let (Some(mine), Some(theirs)) = (world.name(actor), world.name(other)) else {
                return Err(TradeError::NotAvailable);
            };
            self.trades.push(Trade {
                initiator: actor,
                responder: other,
                opened: false,
                offers: [Vec::new(), Vec::new()],
                accepted: [false; 2],
            });
            plan.before.push((actor, wire(registered(&theirs, other, actor))?));
            plan.before.push((other, wire(registered(&mine, other, actor))?));
            return Ok(plan);
        }
        let at = self.find(actor).ok_or(TradeError::NotTrading)?;
        if *request == Request::Close {
            let trade = self.trades.remove(at);
            plan.before.extend(both(&trade, wire(closed(actor))?));
            return Ok(plan);
        }
        let trade = &mut self.trades[at];
        if !trade.opened {
            return Err(TradeError::NotOpen);
        }
        let side = trade.side(actor);
        let clear = |trade: &mut Trade, plan: &mut Plan| -> Result<(), TradeError> {
            if trade.accepted != [false; 2] {
                trade.accepted = [false; 2];
                plan.before.extend(both(trade, bare(ACCEPTANCE_CLEARED).map_err(|_| TradeError::Wrong)?));
            }
            Ok(())
        };
        match *request {
            Request::Offer { amount, item } => {
                if trade.offers[side].iter().any(|(i, _)| *i == item) {
                    return Err(TradeError::Wrong);
                }
                if trade.offers[side].len() >= MAX_OFFERS {
                    return Err(TradeError::TooMany);
                }
                if !world.holds(actor, item, amount) {
                    return Err(TradeError::NotHeld);
                }
                clear(trade, &mut plan)?;
                trade.offers[side].push((item, amount));
                plan.before.extend(both(trade, wire(offered(amount, item, actor))?));
            }
            Request::Revoke(item) => {
                let at = trade.offers[side].iter().position(|(i, _)| *i == item).ok_or(TradeError::Wrong)?;
                clear(trade, &mut plan)?;
                trade.offers[side].remove(at);
                plan.before.extend(both(trade, wire(revoked(item, actor))?));
            }
            Request::Decline => {
                if trade.accepted[side] {
                    trade.accepted[side] = false;
                    plan.before.extend(both(trade, wire(who(UNACCEPTED, actor))?));
                }
            }
            Request::Accept { initiator, responder } => {
                if (initiator, responder) != (trade.initiator, trade.responder) {
                    return Err(TradeError::Wrong);
                }
                if trade.accepted[side] {
                    return Ok(plan);
                }
                trade.accepted[side] = true;
                plan.before.extend(both(trade, wire(who(ACCEPTED, actor))?));
                if trade.accepted == [true; 2] {
                    let [initiator_gives, responder_gives] = std::mem::take(&mut trade.offers);
                    trade.accepted = [false; 2];
                    plan.swap = Some(Swap {
                        initiator: trade.initiator,
                        responder: trade.responder,
                        initiator_gives,
                        responder_gives,
                    });
                    plan.after.extend(both(trade, wire(bare(RESET))?));
                }
            }
            Request::Open(_) | Request::Close => unreachable!(),
        }
        Ok(plan)
    }

    /// The swap could not be made (a full pack, an item gone): both acceptances lapse (`0x51`)
    /// and the offers stay.
    pub fn clear_acceptances(&mut self, player: u64) -> Vec<(u64, Vec<u8>)> {
        let Some(at) = self.find(player) else { return Vec::new() };
        let trade = &mut self.trades[at];
        trade.accepted = [false; 2];
        bare(ACCEPTANCE_CLEARED).map(|data| both(trade, data).to_vec()).unwrap_or_default()
    }

    /// `player` left the world: its trade closes as its close would.
    pub fn leave(&mut self, player: u64) -> Vec<(u64, Vec<u8>)> {
        let Some(at) = self.find(player) else { return Vec::new() };
        let trade = self.trades.remove(at);
        closed(player).map(|data| vec![(trade.other(player), data)]).unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "trade_tests.rs"]
mod tests;
