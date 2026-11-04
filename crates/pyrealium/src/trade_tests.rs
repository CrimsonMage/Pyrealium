//! The trade exchange against `pkt_2017-1-27_1485500185_log.pcap` (2017-02-12 captures), the
//! initiator's side, byte for byte.

use super::*;

const SYTIS: u64 = 0x2130_0000_0000_a40c;
const DERFENHERF: u64 = 0x2130_0000_0000_4f10;
const STRANGER: u64 = 0x2130_0000_0000_0001;
const ITEM_4: u64 = 0x8130_0000_56e7_770b;
const ITEM_6: u64 = 0x8130_0000_55e1_d9f6;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

struct Test;
impl World for Test {
    fn name(&self, player: u64) -> Option<String> {
        match player {
            SYTIS => Some("Sytis [F]".into()),
            DERFENHERF => Some("Derfenherf [M]".into()),
            _ => None,
        }
    }
    fn reachable(&self, _: u64, other: u64) -> bool {
        other != STRANGER
    }
    fn holds(&self, player: u64, item: u64, _: u32) -> bool {
        player == SYTIS && [ITEM_4, ITEM_6].contains(&item)
    }
}

fn to(plan: &Plan, player: u64) -> Vec<Vec<u8>> {
    plan.before.iter().chain(&plan.after).filter(|(p, _)| *p == player).map(|(_, d)| d.clone()).collect()
}

// The captured client requests' arguments.
const OPEN_ARGS: &str = "02000000104f000000003021";
const OFFER_4: &str = "0100000004000000020000000b77e75600003081";
const ACCEPT_ARGS: &str = "05000000030000008bbd00008cbd00008dbd0000000100440000000000003502090000008cbd00000ca4000000003021104f0000000030210000000001000000020000008dbd00000182cd82cd0202020100000000010044000000003200ffff000000020000000000010044000000003200ffff020000020b77e7560000308104000000f6d9e155000030810600000000000000";

#[test]
fn the_captured_requests_decode() {
    assert_eq!(decode(OPEN, &hex(OPEN_ARGS)), Ok(Request::Open(DERFENHERF)));
    assert_eq!(decode(OFFER, &hex(OFFER_4)), Ok(Request::Offer { amount: 4, item: ITEM_4 }));
    assert_eq!(
        decode(ACCEPT, &hex(ACCEPT_ARGS)),
        Ok(Request::Accept { initiator: SYTIS, responder: DERFENHERF })
    );
    assert_eq!(decode(CLOSE, &[]), Ok(Request::Close));
    assert_eq!(decode(DECLINE, &[]), Ok(Request::Decline));
    assert!(decode(OFFER, &hex(&OFFER_4.replace("04000000", "00000000"))).is_err());
    assert!(decode(OPEN, &hex(&OPEN_ARGS[..16])).is_err());
    assert!(decode(CLOSE, &[0]).is_err());
}

#[test]
fn both_sides_see_the_captured_exchange_and_the_swap_happens_once() {
    let mut trades = Runtime::default();
    let mut sytis = Vec::new();
    let mut derfenherf = Vec::new();
    let mut swaps = Vec::new();
    let mut step = |trades: &mut Runtime, actor: u64, request: Request| {
        let plan = trades.apply(actor, &request, &Test).unwrap();
        sytis.extend(to(&plan, SYTIS));
        derfenherf.extend(to(&plan, DERFENHERF));
        swaps.extend(plan.swap);
    };
    step(&mut trades, SYTIS, decode(OPEN, &hex(OPEN_ARGS)).unwrap());
    step(&mut trades, DERFENHERF, Request::Open(SYTIS));
    step(&mut trades, SYTIS, decode(OFFER, &hex(OFFER_4)).unwrap());
    step(&mut trades, SYTIS, Request::Offer { amount: 6, item: ITEM_6 });
    step(&mut trades, DERFENHERF, Request::Accept { initiator: SYTIS, responder: DERFENHERF });
    step(&mut trades, SYTIS, decode(ACCEPT, &hex(ACCEPT_ARGS)).unwrap());
    step(&mut trades, SYTIS, Request::Close);
    // The capture's server calls to Sytis, in order (the name's heap id 0x1599E9 is this
    // graph's 1).
    let captured = [
        "66000100460015826000000005000000010000000100000000010044000000006a00ffff0000000000000000000001000e007c92d2e55a764beb55dde9fc20a613cb9ce6477fcacd13f5c6ad0c2200000000000002000000104f000000003021020000000ca4000000003021",
        "660001004e00158200000000",
        "6600010044001582200000000100000004000000020000000b77e75600003081020000000ca4000000003021",
        "660001004400158220000000010000000600000002000000f6d9e15500003081020000000ca4000000003021",
        "66000100500015820c00000002000000104f000000003021",
        "66000100500015820c000000020000000ca4000000003021",
        "660001005200158200000000",
        "6600010053001582140000000100000000000000020000000ca4000000003021",
    ];
    assert_eq!(sytis, captured.map(hex).to_vec());
    // The other side hears the same, but its registration names Sytis.
    assert_eq!(derfenherf[1..], sytis[1..]);
    assert_eq!(derfenherf[0], registered("Sytis [F]", DERFENHERF, SYTIS).unwrap());
    assert_eq!(
        swaps,
        vec![Swap {
            initiator: SYTIS,
            responder: DERFENHERF,
            initiator_gives: vec![(ITEM_4, 4), (ITEM_6, 6)],
            responder_gives: vec![],
        }]
    );
    assert!(!trades.trading(SYTIS) && !trades.trading(DERFENHERF));
}

#[test]
fn a_decline_after_an_accept_is_the_captured_0x4f_and_a_change_clears_both() {
    let mut trades = Runtime::default();
    trades.apply(SYTIS, &Request::Open(DERFENHERF), &Test).unwrap();
    trades.apply(DERFENHERF, &Request::Open(SYTIS), &Test).unwrap();
    let accept = Request::Accept { initiator: SYTIS, responder: DERFENHERF };
    trades.apply(SYTIS, &accept, &Test).unwrap();
    // A repeated accept is not answered.
    assert!(to(&trades.apply(SYTIS, &accept, &Test).unwrap(), SYTIS).is_empty());
    // pkt_2017-1-27_1485500185: 0x23 -> 0x4F with who declined.
    let declined = trades.apply(SYTIS, &Request::Decline, &Test).unwrap();
    assert_eq!(to(&declined, SYTIS), vec![hex("660001004f0015820c000000020000000ca4000000003021")]);
    trades.apply(SYTIS, &accept, &Test).unwrap();
    let changed = trades.apply(SYTIS, &Request::Offer { amount: 4, item: ITEM_4 }, &Test).unwrap();
    let functions: Vec<u32> = to(&changed, DERFENHERF).iter().map(|d| u32::from_le_bytes(d[4..8].try_into().unwrap())).collect();
    assert_eq!(functions, vec![ACCEPTANCE_CLEARED, OFFERED]);
    // The swap waits for both again.
    trades.apply(SYTIS, &accept, &Test).unwrap();
    assert!(trades.apply(DERFENHERF, &accept, &Test).unwrap().swap.is_some());
}

#[test]
fn refusals_and_leaving() {
    let mut trades = Runtime::default();
    assert_eq!(trades.apply(SYTIS, &Request::Open(STRANGER), &Test).unwrap_err(), TradeError::NotAvailable);
    assert_eq!(trades.apply(SYTIS, &Request::Open(SYTIS), &Test).unwrap_err(), TradeError::NotAvailable);
    assert_eq!(trades.apply(SYTIS, &Request::Close, &Test).unwrap_err(), TradeError::NotTrading);
    trades.apply(SYTIS, &Request::Open(DERFENHERF), &Test).unwrap();
    // Not open yet; Derfenherf holds nothing; a third party's trade is busy.
    assert_eq!(
        trades.apply(SYTIS, &Request::Offer { amount: 4, item: ITEM_4 }, &Test).unwrap_err(),
        TradeError::NotOpen
    );
    trades.apply(DERFENHERF, &Request::Open(SYTIS), &Test).unwrap();
    assert_eq!(
        trades.apply(DERFENHERF, &Request::Offer { amount: 1, item: ITEM_4 }, &Test).unwrap_err(),
        TradeError::NotHeld
    );
    assert_eq!(
        trades.apply(SYTIS, &Request::Accept { initiator: DERFENHERF, responder: SYTIS }, &Test).unwrap_err(),
        TradeError::Wrong
    );
    // Revoke: the item and who.
    trades.apply(SYTIS, &Request::Offer { amount: 4, item: ITEM_4 }, &Test).unwrap();
    let revoked_plan = trades.apply(SYTIS, &Request::Revoke(ITEM_4), &Test).unwrap();
    assert_eq!(to(&revoked_plan, DERFENHERF), vec![revoked(ITEM_4, SYTIS).unwrap()]);
    // pkt_2017-1-21_1485040337: an unanswered open ends with 0x53 (kind 0, the initiator).
    let left = trades.leave(SYTIS);
    assert_eq!(left, vec![(DERFENHERF, hex("6600010053001582140000000100000000000000020000000ca4000000003021"))]);
    assert!(!trades.trading(DERFENHERF));
}
