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
