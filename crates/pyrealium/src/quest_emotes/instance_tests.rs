//! Instance groups, lanes and spawn slots (`instances`).
use super::*;
use super::instances::{WorldOp, group_id};
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::Progression;

/// A two-lane arena (named per test: the taken lanes are process-wide) with a boss slot.
fn arena(name: &str) -> Tables {
    let d = db::empty().unwrap();
    d.execute_batch(&format!("INSERT INTO quest (name, journal_id, title_table, title_key, title, source) VALUES ('q', '0x40000091', '0x25000001', '0x1', 'Q', 't');
        INSERT INTO quest_stage (quest, stage, completed, journal_text, source) VALUES ('q', 1, 0, 'In the arena.', 't'), ('q', 2, 1, 'Done.', 't');
        INSERT INTO alias (name, kind, value, source) VALUES ('door_b', 'placement', '0x049e0112 120 0 3 128', 't');
        INSERT INTO instance_group (name, source) VALUES ('{name}', 't');
        INSERT INTO instance_lane (instance_group, lane, entry, source) VALUES ('{name}', 0, '0x049e0118 180 0 3 128', 't'), ('{name}', 1, 'door_b', 't');
        INSERT INTO spawn_slot (name, instance_group, lane, iid, source) VALUES ('boss', '{name}', 0, '0x81406a7500001900', 't'), ('boss', '{name}', 1, '0x81406a7500001901', 't');
        INSERT INTO emote_set (name, priority, owner_entity, trigger, quest, stage_min, stage_max, source) VALUES
            ('enter', 1, '0x47000001', 'Use', 'q', '0', '0', 't'),
            ('in', 2, NULL, 'QuestSuccess', NULL, NULL, NULL, 't'), ('full', 3, NULL, 'QuestFailure', NULL, NULL, NULL, 't'),
            ('left', 4, NULL, 'Logout', 'q', '1', '1', 't');
        INSERT INTO emote_action (emote_set, ord, action, p1, p2, p3, source) VALUES
            ('enter', 1, 'ClaimInstance', '{name}', 'in', 'full', 't'),
            ('in', 1, 'SetStage', 'q', '1', '', 't'), ('in', 2, 'MoveToLane', '{name}', '', '', 't'), ('in', 3, 'Generate', 'boss', '', '', 't'),
            ('full', 1, 'Message', 'Every arena is taken.', '', '', 't'),
            ('left', 1, 'ReleaseInstance', '{name}', '', '', 't'), ('left', 2, 'EraseQuest', 'q', '', '', 't');")).unwrap();
    Tables::from_db(&d).unwrap()
}
fn enter(t: &Tables, book: &Book) -> Outcome {
    t.run(Trigger::Use, 0x47000001, 9, None, book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 100, &item).unwrap().unwrap()
}

#[test]
fn each_player_takes_a_free_lane_and_its_slots_until_none_is_left() {
    let t = arena("arena_lanes");
    let id = group_id("arena_lanes");
    let a = enter(&t, &Book::default());
    assert_eq!(a.claims, [(id, 0)]);
    assert_eq!(a.moveto.map(|p| p.cell), Some(0x049e0118));
    assert_eq!(a.world, [WorldOp::Generate(0x81406a7500001900)]);
    assert!(a.quests.contains(&Q::Lane { group: id, lane: 0 }));
    // Not committed yet: the lane is still free. Committed, the next player gets lane 1 (its
    // entry a placement alias), and a third finds none.
    assert_eq!(enter(&t, &Book::default()).claims, [(id, 0)]);
    instances::committed(1, &a);
    let b = enter(&t, &Book::default());
    assert_eq!((b.claims.as_slice(), b.moveto.map(|p| p.cell)), (&[(id, 1)][..], Some(0x049e0112)));
    instances::committed(2, &b);
    assert_eq!(enter(&t, &Book::default()).lines, ["Every arena is taken."]);
    let _ = instances::take_world();
    // Leaving frees the lane and sends its creature away; the lane is free again.
    let book_a = a.quests.iter().try_fold(Book::default(), |bk, c| bk.apply(c)).unwrap();
    assert_eq!(book_a.lane(id), Some(0));
    let left = t.run_personal(Trigger::Logout, &book_a, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 200, &item).unwrap();
    assert_eq!(left[0].world, [WorldOp::Dismiss(0x81406a7500001900)]);
    assert_eq!(left[0].releases, [(id, 0)]);
    instances::committed(1, &left[0]);
    assert_eq!(instances::take_world(), [WorldOp::Dismiss(0x81406a7500001900)]);
    assert_eq!(enter(&t, &Book::default()).claims, [(id, 0)]);
    // The lane records survive the journal; the book holds the lane until released.
    for c in [Q::Lane { group: id, lane: 3 }, Q::LaneOff { group: id }] { assert_eq!(Q::decode(&c.encode()).unwrap(), c); }
    assert_eq!(t.slot_iids().count(), 2);
}

#[test]
fn instance_rows_are_checked() {
    let d = db::empty().unwrap();
    d.execute_batch("INSERT INTO instance_group (name, source) VALUES ('g', 't');
        INSERT INTO instance_lane (instance_group, lane, entry, source) VALUES ('g', 0, 'nowhere', 't');").unwrap();
    assert!(Tables::from_db(&d).unwrap_err().to_string().contains("quests instance_lane g/0: entry must be `cell x y z heading` or a placement alias"));
    // A death or logout may free a lane but not take one or summon.
    let reg = HEADERS[0].to_owned() + "\nq\t0x40000092\t0x25000001\t0x1\tQ\t\tkiller\t\t\t\t\tt\n";
    let sets = HEADERS[2].to_owned() + "\ndied\t\t\tPlayerDeath\t\tq\t1\t1\t\t\tt\n";
    let acts = HEADERS[3].to_owned() + "\ndied\t1\tMoveToLane\t0\tg\t\t\t\t\tt\n";
    let e = Tables::parse([&reg, &(HEADERS[1].to_owned() + "\n"), &sets, &acts], ["r", "s", "e", "a"]).unwrap_err().to_string();
    assert!(e.contains("p1 (group) is not an instance_group"), "{e}");
}
