//! The hunts as shipped rows (once the hunting catalog's code): Gregor Adulen's chain.
use super::*;
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::{Command as P, Progression};

const GREGOR: u32 = 0x4700_42da;
const HARVESTERS: u32 = 0x4000_fc04;
const NYMPHS: u32 = 0x4000_fc11;
const HARVESTER: u32 = 0x4700_1f4c;

fn at_level(level: u16) -> Progression {
    let mut p = Progression::default();
    while p.level().number < level { p = p.apply(&P::AwardXp(p.level_span().unwrap_or(1).max(1))).unwrap(); }
    p
}

#[test]
fn a_hunt_is_offered_counted_paid_and_leads_to_the_next_link() {
    let t = &shipped();
    let mut p = at_level(56);
    let mut book = Book::default();
    let act = |trigger, entity, now, book: &mut Book, p: &mut Progression| -> Option<Outcome> {
        let o = if trigger == Trigger::Kill {
            t.run_kill(entity, 7, book, &Inventory::default(), p, ITEM_NAMESPACE + 1, now, &item)
        } else {
            t.run(trigger, entity, 9, None, book, &Inventory::default(), p, ITEM_NAMESPACE + 1, 0.0, now, &item).unwrap()
        }?;
        *book = o.quests.iter().try_fold(book.clone(), |b, c| b.apply(c)).unwrap();
        *p = o.changes.iter().try_fold(p.clone(), |q, c| q.apply(c)).unwrap();
        Some(o)
    };
    // Gregor offers the first link (the next waits for it).
    let o = act(Trigger::Use, GREGOR, 1000, &mut book, &mut p).unwrap();
    assert_eq!((book.stage(HARVESTERS), book.stage(NYMPHS)), (1, 0));
    assert_eq!(o.says[0].0.as_deref(), Some("Gregor Adulen"));
    // Ten harvesters: counted, then the goal is met.
    for n in 1..=10 { act(Trigger::Kill, HARVESTER, 1001, &mut book, &mut p).unwrap(); assert_eq!(book.count(HARVESTERS), n); }
    assert_eq!(book.stage(HARVESTERS), 2);
    // The reward: 3% of the level's span, capped.
    let span = p.level_span().unwrap();
    let o = act(Trigger::Use, GREGOR, 1002, &mut book, &mut p).unwrap();
    assert_eq!(format!("{:?}", o.changes), format!("[AwardXp({})]", (span * 300 / 10_000).min(10_787_851)));
    assert!(t.solved(&book, HARVESTERS));
    // Solved and not repeatable for a week: Gregor offers the next link.
    act(Trigger::Use, GREGOR, 1003, &mut book, &mut p).unwrap();
    assert_eq!(book.stage(NYMPHS), 1);
    // A kill of a creature the hunt does not list changes nothing.
    assert!(act(Trigger::Kill, 0x4700_0001, 1004, &mut book, &mut p).is_none());
}

#[test]
fn an_elixir_starts_its_quest_and_kantens_bag_goes_to_orven() {
    let t = &shipped();
    let p = at_level(20);
    let elixir = item(0x4700_0f43, ITEM_NAMESPACE + 1, 0).unwrap();
    let bag = Inventory::default().apply(&crate::inventory::Command::Grant(elixir)).unwrap();
    // Wasp Bane: using the elixir takes it and starts the quest.
    let o = t.run(Trigger::UseItem, 0x4700_0f43, 0, None, &Book::default(), &bag, &p, ITEM_NAMESPACE + 2, 0.0, 100, &item).unwrap().unwrap();
    assert!(o.quests.contains(&Q::Stage { quest: 0x4000_000a, stage: 1 }) && o.items.len() == 1);
    assert!(o.lines.iter().any(|l| l == "Wasp Bane accepted. Defeat 10 targets before the time limit."), "{:?}", o.lines);
    // Gambling Debt: Kanten gives the job and his bag; Orven takes the bag and pays.
    let (kanten, orven, debt) = (0x4700_1a3c, 0x4700_1c27, 0x4000_fe01);
    let o = t.run(Trigger::Use, kanten, 9, None, &Book::default(), &Inventory::default(), &at_level(11), ITEM_NAMESPACE + 1, 0.0, 100, &item).unwrap().unwrap();
    assert_eq!(o.given, [0x4700_1c1e]);
    let book = o.quests.iter().try_fold(Book::default(), |b, c| b.apply(c)).unwrap();
    let bag = o.items.iter().try_fold(Inventory::default(), |b, c| b.apply(c)).unwrap();
    let o = t.run(Trigger::Use, orven, 9, None, &book, &bag, &at_level(11), ITEM_NAMESPACE + 2, 0.0, 200, &item).unwrap().unwrap();
    assert_eq!(format!("{:?}", o.changes), "[AwardXp(1000)]");
    assert!(o.quests.contains(&Q::Solved { quest: debt, at: 200 }));
    // Without the bag, Orven says so.
    let o = t.run(Trigger::Use, orven, 9, None, &book, &Inventory::default(), &at_level(11), ITEM_NAMESPACE + 2, 0.0, 200, &item).unwrap().unwrap();
    assert_eq!(o.says[0].1, "Bring Kanten's Bag in your backpack to complete Gambling Debt.");
}
