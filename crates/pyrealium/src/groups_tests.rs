//! The fellowship exchanges against the 2005 captures' layouts (`groups` notes).

use super::*;

const LEADER: u64 = 0x2120_0000_0000_0001;
const RECRUIT: u64 = 0x2120_0000_0000_bfaf;
const THIRD: u64 = 0x2120_0000_0000_0003;

fn member(id: u64, name: &str, level: u32) -> Member {
    Member {
        id,
        name: name.into(),
        level,
        health: 50,
        max_health: 100,
        vigor: 40,
        max_vigor: 80,
        cell: 0x7ab1_0010,
    }
}
fn online() -> BTreeMap<u64, Member> {
    [member(LEADER, "Leader", 20), member(RECRUIT, "Recruit", 22), member(THIRD, "Third", 30)]
        .into_iter()
        .map(|m| (m.id, m))
        .collect()
}
fn run(runtime: &mut Runtime, actor: u64, request: Request, now: u64) -> Vec<Delivery> {
    let plan = runtime.prepare(actor, &request, now, &online()).unwrap();
    runtime.commit(plan).unwrap()
}
/// The script function of a delivery (`envelope`: 0x10066, then the function).
fn function(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[4..8].try_into().unwrap())
}
/// The string key of a fellowship line.
fn key(data: &[u8]) -> Option<u32> {
    (function(data) == script::CHAT_STRING).then(|| u32::from_le_bytes(data[36..40].try_into().unwrap()))
}
fn calls(deliveries: &[Delivery], recipient: u64) -> Vec<(u32, Option<u32>)> {
    deliveries
        .iter()
        .filter(|d| d.recipient == recipient)
        .map(|d| (function(&d.data), key(&d.data)))
        .collect()
}
fn create(runtime: &mut Runtime, looting: u32, social: bool) -> Vec<Delivery> {
    run(runtime, LEADER, Request::Create { name: "Wanderers".into(), looting, social }, 0)
}

#[test]
fn the_flags_word_is_the_captured_one_for_every_create() {
    // (looting, social) -> snapshot field 2, every create/snapshot pair of the 2005 captures.
    assert_eq!(flags(1, false), 3);
    assert_eq!(flags(2, true), 4);
    assert_eq!(flags(4, true), 0x10);
    assert_eq!(flags(3, false), 9);
    // `Fellowship` 0x2E reads no loot bit as method 0 (None); bit 0 alone is sharing.
    assert_eq!(flags(0, false), 1);
    assert_eq!(flags(0, true), 0);
}

#[test]
fn every_looting_method_and_sharing_choice_creates_a_fellowship() {
    for looting in 0..LOOT_METHODS {
        for social in [false, true] {
            let mut runtime = Runtime::default();
            create(&mut runtime, looting, social);
            assert_eq!(runtime.sharing(LEADER).unwrap().shared, !social);
        }
    }
    let mut request = script::integer(5);
    request.extend(script::integer(0));
    assert!(decode(CREATE, &request).is_err());
}

#[test]
fn create_invite_accept_send_the_retail_exchange() {
    let mut runtime = Runtime::default();
    let created = create(&mut runtime, 1, false);
    assert_eq!(calls(&created, LEADER), [(SNAPSHOT, None), (script::CHAT_STRING, Some(text::CREATE))]);
    let invited = run(&mut runtime, LEADER, Request::Invite(RECRUIT), 0);
    assert_eq!(calls(&invited, RECRUIT), [(INVITATION, None)]);
    assert_eq!(calls(&invited, LEADER), [(script::CHAT_STRING, Some(text::WAITING))]);
    let accepted = run(&mut runtime, RECRUIT, Request::Accept(LEADER), 0);
    assert_eq!(
        calls(&accepted, RECRUIT),
        [
            (script::CHAT_STRING, Some(text::JOIN)),
            (SNAPSHOT, None),
            (script::CHAT_STRING, Some(text::JOINED)),
        ]
    );
    // The leader: add-fellow, the five vitals, the line.
    let leader = calls(&accepted, LEADER);
    assert_eq!(leader.first(), Some(&(ADD, None)));
    assert_eq!(leader.len(), 7);
    assert_eq!(leader.last(), Some(&(script::CHAT_STRING, Some(text::HAS_JOINED))));
}

#[test]
fn leave_dismiss_and_disband_send_the_retail_exchange() {
    let mut runtime = Runtime::default();
    create(&mut runtime, 1, false);
    for fellow in [RECRUIT, THIRD] {
        run(&mut runtime, LEADER, Request::Invite(fellow), 0);
        run(&mut runtime, fellow, Request::Accept(LEADER), 0);
    }
    let left = run(&mut runtime, THIRD, Request::Leave, 0);
    assert_eq!(calls(&left, THIRD), [(CLEAR, None), (script::CHAT_STRING, Some(text::LEAVE))]);
    for other in [LEADER, RECRUIT] {
        assert_eq!(calls(&left, other), [(REMOVE, None), (script::CHAT_STRING, Some(text::HAS_LEFT))]);
    }
    let dismissed = run(&mut runtime, LEADER, Request::Dismiss(RECRUIT), 0);
    assert_eq!(calls(&dismissed, LEADER), [(REMOVE, None), (script::CHAT_STRING, Some(text::DISMISS))]);
    assert_eq!(calls(&dismissed, RECRUIT), [(CLEAR, None), (script::CHAT_STRING, Some(text::DISMISSED))]);
    run(&mut runtime, LEADER, Request::Invite(RECRUIT), 0);
    run(&mut runtime, RECRUIT, Request::Accept(LEADER), 0);
    let disbanded = run(&mut runtime, LEADER, Request::Disband, 0);
    for player in [LEADER, RECRUIT] {
        assert_eq!(calls(&disbanded, player), [(CLEAR, None), (script::CHAT_STRING, Some(text::DISBANDED))]);
    }
    assert!(!runtime.contains(LEADER) && !runtime.contains(RECRUIT));
}

#[test]
fn a_fellow_that_is_not_the_leader_cannot_disband_and_sees_the_clients_line() {
    let mut runtime = Runtime::default();
    create(&mut runtime, 1, false);
    run(&mut runtime, LEADER, Request::Invite(RECRUIT), 0);
    run(&mut runtime, RECRUIT, Request::Accept(LEADER), 0);
    let Err(error) = runtime.prepare(RECRUIT, &Request::Disband, 0, &online()) else {
        panic!("a fellow disbanded");
    };
    // Captured: "You must be the leader to disband a Fellowship." as chat type 1.
    let wire = error.wire().unwrap();
    assert_eq!(key(&wire), Some(text::NOT_LEADER_DISBAND));
    assert_eq!(&wire[wire.len() - 8..], &script::integer(1)[..]);
}

#[test]
fn a_fellowship_holds_nine() {
    let mut runtime = Runtime::default();
    let mut people = online();
    for n in 0..10u64 {
        let id = 0x2120_0000_0001_0000 + n;
        people.insert(id, member(id, &format!("Fellow{}", (b'a' + n as u8) as char), 20));
    }
    let plan = runtime
        .prepare(LEADER, &Request::Create { name: "Nine".into(), looting: 1, social: false }, 0, &people)
        .unwrap();
    runtime.commit(plan).unwrap();
    let mut refused = None;
    for n in 0..10u64 {
        let id = 0x2120_0000_0001_0000 + n;
        match runtime.prepare(LEADER, &Request::Invite(id), 0, &people) {
            Ok(plan) => {
                runtime.commit(plan).unwrap();
                let plan = runtime.prepare(id, &Request::Accept(LEADER), 0, &people).unwrap();
                runtime.commit(plan).unwrap();
            }
            Err(error) => {
                refused = Some(error);
                break;
            }
        }
    }
    assert_eq!(runtime.members(LEADER).len(), MAX_MEMBERS);
    assert!(matches!(refused, Some(GroupError::Full)));
}

#[test]
fn a_lapsed_invitation_closes_the_recruits_dialog() {
    let mut runtime = Runtime::default();
    create(&mut runtime, 1, false);
    run(&mut runtime, LEADER, Request::Invite(RECRUIT), 0);
    assert!(runtime.prepare_expiry(INVITATION_MS - 1, &online()).unwrap().is_none());
    let plan = runtime.prepare_expiry(INVITATION_MS, &online()).unwrap().unwrap();
    let sent = runtime.commit(plan).unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].recipient, RECRUIT);
    // `0x31` carries the inviter (captured 2017: 12 bytes, an id).
    assert_eq!(function(&sent[0].data), EXPIRED);
    assert_eq!(&sent[0].data[12..], &script::identity(LEADER)[..]);
}

#[test]
fn the_wire_forms_match_the_captured_layouts() {
    // `0x26`: the id alone (captured: 0c000000 02000000 <id>).
    let remove = script::private_call(REMOVE, &script::identity(RECRUIT)).unwrap();
    assert_eq!(&remove[8..12], &12u32.to_le_bytes());
    assert_eq!(&remove[12..], &script::identity(RECRUIT)[..]);
    // `0x32`: the level then the id (captured 14000000 01000000 <level> 02000000 <id>).
    let mut runtime = Runtime::default();
    create(&mut runtime, 1, false);
    let before = member(LEADER, "Leader", 20);
    let after = member(LEADER, "Leader", 21);
    let level = runtime.member_changes(LEADER, &after, Some(&before)).unwrap();
    assert_eq!(level.len(), 1);
    assert_eq!(function(&level[0].data), LEVEL);
    let mut args = script::integer(21);
    args.extend(script::identity(LEADER));
    assert_eq!(&level[0].data[12..], &args[..]);
    // `0x27`: a three-object graph (Fellow, FellowVitals with only the cell, the name) and the id.
    let added = add(&member(RECRUIT, "Recruit", 22)).unwrap();
    let a = &added[12..];
    let word = |at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
    assert_eq!((word(0), word(4)), (5, 3));
    assert_eq!(word(28), 0x01ee_0000);
    assert_eq!(&a[a.len() - 12..], &script::identity(RECRUIT)[..]);
    // A named line: key, table, one variable (NAME, type 9) holding the literal, then 0 and the
    // chat type (captured "You have joined the '<NAME>' Fellowship.").
    let joined = named(text::JOINED, "Wanderers", 8).unwrap();
    let w = |at: usize| u32::from_le_bytes(joined[at..at + 4].try_into().unwrap());
    assert_eq!((w(36), w(40), w(44), w(48), w(52)), (text::JOINED, text::TABLE, 1, script::VAR_NAME, 9));
    let (name, _) = world::read_literal_name(&joined[56..]).unwrap();
    assert_eq!(name, "Wanderers");
    assert_eq!(&joined[joined.len() - 12..], &[0u8, 0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0]);
}
