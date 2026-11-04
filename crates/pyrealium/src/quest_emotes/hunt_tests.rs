//! The hunts as shipped rows (once the hunting catalog's code): Gregor Adulen's chain.
use super::*;
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::{Command as P, Progression};

const GREGOR: u32 = 0x4700_42da;
const HARVESTERS: u32 = 0x4000_fc04;
const NYMPHS: u32 = 0x4000_fc11;
const HARVESTER: u32 = 0x4700_1f4c;
/// An NPC whose quests this server lacks (talk_470032b9).
const TALKER: u32 = 0x4700_32b9;

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
    // His DAT offer line (lina_gregor.stt), not Pyrealium text.
    assert_eq!(o.tells, [(0x2500_8b09, 0x2ae6_124e)]);
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
    // Without the bag, Orven gives the quest's journal line (questkanten.stt).
    let o = t.run(Trigger::Use, orven, 9, None, &book, &Inventory::default(), &at_level(11), ITEM_NAMESPACE + 2, 0.0, 200, &item).unwrap().unwrap();
    assert_eq!(o.tells, [(0x2500_6faf, 0x007a_d82a)]);
    // With nothing for the player, one of his idle lines at random (cumulative probabilities).
    let idle = |roll| t.run(Trigger::Use, orven, 9, None, &Book::default(), &Inventory::default(), &at_level(11), ITEM_NAMESPACE + 2, roll, 200, &item).unwrap().unwrap().tells;
    assert_eq!((idle(0.1), idle(0.6), idle(0.99)), (vec![(0x2500_707a, 0xe7e6_8c7b)], vec![(0x2500_707a, 0xa7e6_8c7a)], vec![(0x2500_707a, 0x27e6_8c7b)]));
}

/// One player's state through a run of triggers.
struct Walker { book: Book, bag: Inventory, p: Progression }
impl Walker {
    fn go(&mut self, t: &Tables, trigger: Trigger, entity: u32, now: u64) -> Outcome {
        let first = self.bag.high_water().max(ITEM_NAMESPACE) + 1;
        let o = if trigger == Trigger::Kill { t.run_kill(entity, 7, &self.book, &self.bag, &self.p, first, now, &item).unwrap() }
            else { t.run(trigger, entity, 9, None, &self.book, &self.bag, &self.p, first, 0.0, now, &item).unwrap().unwrap() };
        self.book = o.quests.iter().try_fold(self.book.clone(), |b, c| b.apply(c)).unwrap();
        self.bag = o.items.iter().try_fold(self.bag.clone(), |b, c| b.apply(c)).unwrap();
        self.p = o.changes.iter().try_fold(self.p.clone(), |q, c| q.apply(c)).unwrap();
        o
    }
}

#[test]
fn an_obelisk_search_its_referral_and_the_arwic_lesson_play_through() {
    let t = &shipped();
    let (explorer, ruby, sapphire, emerald) = (0x4700_159f, 0x4700_0872, 0x4700_0877, 0x4700_087b);
    let (search, referral, lesson) = (0x4000_0016, 0x4000_00c0, 0x4000_00c1);
    let (overseer, outcast, hammer, iron, sword) = (0x4700_15ad, 0x4700_15ac, 0x4700_15a9, 0x4700_10f3, 0x4700_15aa);
    let mut w = Walker { book: Book::default(), bag: Inventory::default(), p: at_level(5) };
    // The search: the obelisks in turn (out of turn changes nothing), then the Explorer pays.
    w.go(t, Trigger::Use, explorer, 100);
    assert_eq!(w.book.stage(search), 1);
    w.go(t, Trigger::Use, sapphire, 101);
    assert_eq!(w.book.stage(search), 1, "out of turn");
    for (obelisk, stage) in [(ruby, 2), (sapphire, 3), (emerald, 4)] { w.go(t, Trigger::Use, obelisk, 102); assert_eq!(w.book.stage(search), stage); }
    let o = w.go(t, Trigger::Use, explorer, 103);
    assert_eq!(format!("{:?}", o.changes), "[AwardXp(80)]");
    assert!(t.solved(&w.book, search) && w.book.stage(referral) == 1, "the referral starts");
    // The Overseer: the referral ends and the lesson begins; an Outcast drops a hammer.
    w.go(t, Trigger::Use, overseer, 104);
    assert!(t.solved(&w.book, referral) && w.book.stage(lesson) == 1);
    let o = w.go(t, Trigger::Kill, outcast, 105);
    assert_eq!(o.given, [hammer]);
    // Back with the hammer: the lesson at once (20 Iron and the recipe), as the Overseer's
    // lines run ("I see you have returned with your flat hammer!", then the Craft Tab).
    let o = w.go(t, Trigger::Use, overseer, 106);
    assert_eq!(o.tells, [(0x2500_169c, 0x8cf4_023e), (0x2500_169c, 0x8cf4_0232)]);
    assert!(w.book.knows(0x6f00_1f0e) && w.book.stage(lesson) == 3);
    assert_eq!(w.bag.items().filter(|i| i.definition == iron).map(|i| i.quantity).sum::<u32>(), 20);
    let first = w.bag.high_water().max(ITEM_NAMESPACE) + 1;
    let craft = t.craft(0x6f00_1f0e, &w.book, &w.bag, &w.p, first, 108, &item).unwrap();
    w.bag = craft.items.iter().try_fold(w.bag.clone(), |b, c| b.apply(c)).unwrap();
    assert!(w.bag.items().any(|i| i.definition == sword) && !w.bag.items().any(|i| i.definition == iron));
    w.go(t, Trigger::Crafted, sword, 109);
    assert_eq!(w.book.stage(lesson), 4);
    // The sword shown: glyph, guide, 150 experience; the lesson done.
    let o = w.go(t, Trigger::Use, overseer, 110);
    assert_eq!(format!("{:?}", o.changes), "[AwardXp(150)]");
    assert_eq!(o.given, [0x4700_109f, 0x4700_2917]);
    assert!(t.solved(&w.book, lesson));
    // An unlearned recipe is refused.
    assert!(t.craft(0x6f00_1f0e, &Book::default(), &w.bag, &w.p, first, 111, &item).is_err());
}

#[test]
fn an_npc_answers_topics_and_talks_from_rows() {
    let t = &shipped();
    let p = at_level(1);
    let mentor = crate::training_dialogue::MENTOR_ENTITY;
    let ask = |topic: &str| t.run(Trigger::Ask, mentor, 9, Some(topic_key(topic)), &Book::default(), &Inventory::default(), &p, ITEM_NAMESPACE + 1, 0.0, 100, &item)
        .unwrap().unwrap().says[0].1.clone();
    // A topic's own set (case and spacing do not matter); any other topic gets the help line.
    assert!(ask("  GEAR ").starts_with("Your starter weapons"));
    assert!(ask("weather").starts_with("I can help with"));
    assert_eq!(ask(""), ask("weather"));
    // An NPC with no task on this server says its DAT "no task today" line (Talk).
    let o = t.run(Trigger::Talk, TALKER, 9, None, &Book::default(), &Inventory::default(), &p, ITEM_NAMESPACE + 1, 0.0, 100, &item).unwrap().unwrap();
    assert_eq!(o.tells[0].1, 0x0920_0efe);
    assert!(t.run(Trigger::Use, TALKER, 9, None, &Book::default(), &Inventory::default(), &p, ITEM_NAMESPACE + 1, 0.0, 100, &item).unwrap().is_none(), "Talk is not Use");
}
