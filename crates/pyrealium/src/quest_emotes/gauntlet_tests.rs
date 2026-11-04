//! The Gauntlet as shipped rows (`tools/author_gauntlet_quest.py`): a run from the Master's offer
//! through the fights, the reward, an early exit and a death, on the engine alone.
use super::*;
use super::instances::{WorldOp, group_id};
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::{Command as P, Progression};

const QUEST: u32 = 0x4002_00a1;
const MASTER: u32 = 0x4700_326a;
const PORTAL: u32 = 0x4700_326b;
const SLOT_BASE: u64 = 0x8140_6a75_0000_0000;
/// The opponents in fight order, with their levels (`gauntlet-content-v1.json`).
fn opponents() -> Vec<(u32, u64)> {
    let data: serde_json::Value = serde_json::from_str(include_str!("../../../../content/gauntlet-content-v1.json")).unwrap();
    data["opponents"].as_array().unwrap().iter()
        .map(|o| (u32::from_str_radix(o["entity"].as_str().unwrap(), 16).unwrap(), o["level"].as_u64().unwrap())).collect()
}
fn at_level(level: u16) -> Progression {
    let mut p = Progression::default();
    while p.level().number < level { p = p.apply(&P::AwardXp(p.level_span().unwrap_or(1).max(1))).unwrap(); }
    p
}
struct Player { book: Book, bag: Inventory, progression: Progression }
impl Player {
    fn new(level: u16) -> Self { Self { book: Book::default(), bag: Inventory::default(), progression: at_level(level) } }
    fn take(&mut self, o: &Outcome) {
        self.book = o.quests.iter().try_fold(self.book.clone(), |b, c| b.apply(c)).unwrap();
        self.progression = o.changes.iter().try_fold(self.progression.clone(), |p, c| p.apply(c)).unwrap();
        self.bag = o.items.iter().try_fold(self.bag.clone(), |b, c| b.apply(c)).unwrap();
    }
    fn trigger(&mut self, t: &Tables, trigger: Trigger, entity: u32, now: u64) -> Option<Outcome> {
        let first = self.bag.high_water().max(ITEM_NAMESPACE) + 1;
        let o = t.run(trigger, entity, 9, None, &self.book, &self.bag, &self.progression, first, 0.0, now, &item).unwrap()?;
        self.take(&o);
        Some(o)
    }
    /// A delayed part of a set, run when its wait is over.
    fn resume(&mut self, t: &Tables, o: &Outcome, now: u64) -> Outcome {
        let first = self.bag.high_water().max(ITEM_NAMESPACE) + 1;
        let r = t.resume(o.resume.as_ref().unwrap(), &self.book, &self.bag, &self.progression, first, 0.0, now, &item).unwrap();
        self.take(&r);
        r
    }
}

#[test]
fn a_gauntlet_run_from_the_offer_to_the_reward() {
    let t = &shipped();
    let fights = opponents();
    let arenas = group_id("gauntlet_arenas");
    // Too young: the Master turns the player away.
    let mut young = Player::new(10);
    assert_eq!(young.trigger(t, Trigger::Use, MASTER, 1000).unwrap().tells, [(0x2500_7e77, 0xc4f9_188e)]);
    // Level 46: the 45plus bracket starts at fight 12.
    let mut p = Player::new(46);
    let o = p.trigger(t, Trigger::Use, MASTER, 1000).unwrap();
    assert_eq!(o.tells, [(0x2500_7e77, 0x2a6b_091e)]);
    assert_eq!((p.book.stage(QUEST), p.book.subcount(QUEST, 0)), (12, 6));
    // The portal claims an arena, carries the player to fight 12's room and summons its opponent.
    let o = p.trigger(t, Trigger::Use, PORTAL, 1010).unwrap();
    let lane = p.book.lane(arenas).expect("an arena");
    assert_eq!(o.world, [WorldOp::Generate(SLOT_BASE | (fights[11].1 << 8) | u64::from(lane))]);
    assert!(o.moveto.is_some());
    instances::committed(1, &o);
    let _ = instances::take_world();
    // Each kill moves the run on; five seconds later the player is in the next room with its opponent.
    for fight in 12..16 {
        let kill = p.trigger(t, Trigger::Kill, fights[fight - 1].0, 1100).unwrap();
        assert_eq!(p.book.stage(QUEST), fight as u8 + 1);
        assert_eq!(kill.resume.as_ref().map(|r| r.after_ms), Some(5000));
        let next = p.resume(t, &kill, 1105);
        assert_eq!(next.world, [WorldOp::Generate(SLOT_BASE | (fights[fight].1 << 8) | u64::from(lane))]);
        assert!(next.moveto.is_some());
    }
    // The last kill brings the player back to the hub and frees the arena.
    let kill = p.trigger(t, Trigger::Kill, fights[15].0, 1200).unwrap();
    assert_eq!(p.book.stage(QUEST), 17);
    let back = p.resume(t, &kill, 1205);
    assert_eq!(back.moveto.map(|m| m.cell), Some(0x049e_0108));
    assert_eq!(back.releases, [(arenas, lane)]);
    assert_eq!(p.book.lane(arenas), None);
    instances::committed(1, &back);
    let _ = instances::take_world();
    // The reward: 2% of the level's span, 25,000 coins and the Frenzy Potion; then solved.
    let span = p.progression.level_span().unwrap();
    let o = p.trigger(t, Trigger::Use, MASTER, 1300).unwrap();
    let paid = format!("{:?}", o.changes);
    assert!(paid.contains(&format!("AwardXp({})", span * 200 / 10_000)) && paid.contains("AwardGold(25000)"), "{paid}");
    assert_eq!(o.given, [0x4700_326f]);
    assert_eq!(p.book.stage(QUEST), 18);
    assert_eq!(p.book.timer(QUEST).solves, 1);
    // Not again before the 22 hour reset.
    assert_eq!(p.trigger(t, Trigger::Use, MASTER, 1400).unwrap().tells, [(0x2500_7e77, 0x6369_1d8f)]);
}

#[test]
fn an_early_exit_pays_the_share_won_and_a_death_pays_nothing() {
    let t = &shipped();
    let fights = opponents();
    // Level 22 (bracket 20to24, all 16 fights): beat four, then return to the Master.
    let mut p = Player::new(22);
    p.trigger(t, Trigger::Use, MASTER, 1000).unwrap();
    assert_eq!(p.book.stage(QUEST), 1);
    let o = p.trigger(t, Trigger::Use, PORTAL, 1001).unwrap();
    instances::committed(2, &o);
    for (entity, _) in &fights[..4] { p.trigger(t, Trigger::Kill, *entity, 1002).unwrap(); }
    let span = p.progression.level_span().unwrap();
    let o = p.trigger(t, Trigger::Use, MASTER, 1100).unwrap();
    assert_eq!(o.tells, [(0x2500_7e77, 0x2968_fd1e)]);
    // 2% x 4/16 = 0.5% of the span, 25,000 x 4/16 coins.
    let paid = format!("{:?}", o.changes);
    assert!(paid.contains(&format!("AwardXp({})", span * 50 / 10_000)) && paid.contains("AwardGold(6250)"), "{paid}");
    assert_eq!((p.book.stage(QUEST), o.releases.len()), (18, 1));
    instances::committed(2, &o);
    // Another player dies at the first fight: no pay, the arena freed, the run erased.
    let mut q = Player::new(22);
    q.trigger(t, Trigger::Use, MASTER, 1000).unwrap();
    let o = q.trigger(t, Trigger::Use, PORTAL, 1001).unwrap();
    instances::committed(3, &o);
    let died = t.run_personal(Trigger::PlayerDeath, &q.book, &q.bag, &q.progression, ITEM_NAMESPACE + 1, 1010, &item).unwrap();
    assert_eq!(died.len(), 1);
    assert!(died[0].changes.is_empty() && died[0].releases.len() == 1 && died[0].world.iter().all(|w| matches!(w, WorldOp::Dismiss(_))));
    q.take(&died[0]);
    assert_eq!(q.book.stage(QUEST), 0);
    instances::committed(3, &died[0]);
    let _ = instances::take_world();
    // Returning at once, with nothing beaten, is pathetic and unpaid.
    let mut r = Player::new(22);
    r.trigger(t, Trigger::Use, MASTER, 1000).unwrap();
    let o = r.trigger(t, Trigger::Use, MASTER, 1001).unwrap();
    assert_eq!((o.tells.as_slice(), o.changes.is_empty(), r.book.stage(QUEST)), (&[(0x2500_7e77, 0xa964_99a4)][..], true, 0));
    // The portal outside a run: "Nothing happens."
    assert_eq!(Player::new(22).trigger(t, Trigger::Use, PORTAL, 1).unwrap().tells, [(0x2500_7e78, 0x30b9_998d)]);
}
