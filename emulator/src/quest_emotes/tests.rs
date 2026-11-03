use super::*;
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::{Command as P, Progression};

const SHELTER: u32 = 0x4000ff05;
const G: u32 = 0x4700157d;
const TOOL: u32 = 0x47001587;
const GLYPH: u32 = 0x47001582;

fn book_at(stage: u8) -> Book {
    Book::default().apply(&Q::Stage { quest: SHELTER, stage }).unwrap()
}
fn bag_with(entity: u32) -> Inventory {
    let item = crate::shelter::item(entity, ITEM_NAMESPACE + 1, 0).unwrap();
    Inventory::default().apply(&I::Grant(item)).unwrap()
}
fn first(bag: &Inventory) -> u64 { bag.high_water().max(ITEM_NAMESPACE) + 1 }
fn run(t: &Tables, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book, bag: &Inventory) -> Outcome {
    t.run(trigger, entity, iid, item, book, bag, &Progression::default(), first(bag), 0.0, 0, &crate::shelter::item).unwrap()
        .unwrap_or_else(|| panic!("no set for {entity:08x} {trigger:?}"))
}

#[test]
fn the_shipped_tables_load() {
    let t = shipped();
    assert_eq!(t.quest("shelter").map(|q| q.journal_id), Some(SHELTER));
    assert_eq!(t.stage(SHELTER, 6).map(|(done, _)| done), Some(true));
    assert!(t.gives().all(|(_, e)| crate::shelter::item(e, ITEM_NAMESPACE + 1, 0).is_some()));
}

#[test]
fn every_shipped_table_family_loads() {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
    // The generated AC2 Vault family (tools/import_vault_quests.py) is there for each table.
    for table in FILES {
        let names: Vec<String> = families(&content, table).unwrap().iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&table.replace(".tsv", "-vault.tsv")), "{table}: {names:?}");
    }
    let t = Tables::load(&content).unwrap();
    assert_eq!(t, *shipped());
    assert!(t.gives().all(|(_, e)| crate::shelter::item(e, ITEM_NAMESPACE + 1, 0).is_some()));
}

#[test]
fn a_family_file_adds_to_its_table_and_names_its_own_lines() {
    let dir = std::env::temp_dir().join(format!("pyrealium-quest-families-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, body: &str| std::fs::write(dir.join(name), body).unwrap();
    write("quest-registry.tsv", &(HEADERS[0].to_owned() + "\nbase\t0x40000001\t0x25000001\t0x1\tBase\t\tkiller\t\t\t\t\tsrc\n"));
    write("quest-registry-extra.tsv", &(HEADERS[0].to_owned() + "\n# generated\nextra\t0x40000002\t0x25000001\t0x1\tExtra\t\tkiller\t\t\t\t\tsrc\n"));
    write("quest-stages.tsv", &(HEADERS[1].to_owned() + "\n"));
    // a family's stage may name a quest from the base registry, and the reverse
    write("quest-stages-extra.tsv", &(HEADERS[1].to_owned() + "\nbase\t1\t1\tDone.\tsrc\nextra\t1\t1\tDone.\tsrc\n"));
    write("emote-sets.tsv", &(HEADERS[2].to_owned() + "\ns\t0x47000001\t\tUse\t\textra\t0\t0\t\t\tsrc\n"));
    write("emote-sets-extra.tsv", &(HEADERS[2].to_owned() + "\n"));
    write("emote-actions.tsv", &(HEADERS[3].to_owned() + "\n"));
    write("emote-actions-extra.tsv", &(HEADERS[3].to_owned() + "\ns\t1\tSetStage\t0\textra\t1\t\t\t\tsrc\n"));
    write("emote-actions-extra.txt", "not a table");
    let t = Tables::load(&dir).unwrap();
    assert_eq!(t.quest("extra").map(|q| q.journal_id), Some(0x40000002));
    assert_eq!(t.stage(0x40000001, 1), Some((true, "Done.")));
    let o = t.run(Trigger::Use, 0x47000001, 9, None, &Book::default(), &Inventory::default(), &Progression::default(),
        ITEM_NAMESPACE + 1, 0.0, 0, &|_, _, _| None).unwrap().unwrap();
    assert_eq!(o.quests, [Q::Stage { quest: 0x40000002, stage: 1 }, Q::Solved { quest: 0x40000002, at: 0 }]);
    // A quest or set defined twice across files is refused; errors name the family file and line.
    write("quest-registry-more.tsv", &(HEADERS[0].to_owned() + "\nbase\t0x40000003\t0x25000001\t0x1\tAgain\t\tkiller\t\t\t\t\tsrc\n"));
    let err = Tables::load(&dir).unwrap_err().to_string();
    assert!(err.contains("quest-registry-more.tsv:2: duplicate quest"), "{err}");
    std::fs::remove_file(dir.join("quest-registry-more.tsv")).unwrap();
    write("emote-actions-extra.tsv", &(HEADERS[3].to_owned() + "\ns\t1\tSetStage\t0\textra\t1\t\t\t\tsrc\ns\t1\tTell\t0\t0x25000001\t0x2\t\t\t\tsrc\n"));
    let err = Tables::load(&dir).unwrap_err().to_string();
    assert!(err.contains("emote-actions-extra.tsv:3: duplicate order in set s"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_row_names_its_file_and_line() {
    let ok = |i: usize| HEADERS[i].to_owned() + "\n";
    let reg = ok(0) + "q\t0x40000001\t0x25000001\t0x1\tQ\t\tkiller\t\t\t\t\tsrc\n";
    let sets = ok(2) + "s\t0x47000001\t\tUse\t\t\t\t\t\t\tsrc\n";
    let acts = ok(3) + "s\t1\tTell\t0\t0x25000001\t0x2\t\t\t\tsrc\ns\t2\tAwardXP\t0\tlots\t\t\t\t\tsrc\n";
    let err = Tables::parse([&reg, &ok(1), &sets, &acts], ["r", "st", "se", "a.tsv"]).unwrap_err();
    assert!(err.to_string().starts_with("a.tsv:3: "), "{err}");
    let sets = ok(2) + "s\t0x47000001\t\tWave\t\t\t\t\t\t\tsrc\n";
    let err = Tables::parse([&reg, &ok(1), &sets, &ok(3)], ["r", "st", "se.tsv", "a"]).unwrap_err();
    assert!(err.to_string().starts_with("se.tsv:2: "), "{err}");
    assert!(Tables::parse(["quest\n", &ok(1), &ok(2), &ok(3)], ["r.tsv", "", "", ""]).unwrap_err().to_string().starts_with("r.tsv:1: "));
}

#[test]
fn each_shelter_step_matches_the_quest() {
    let t = shipped();
    let none = Inventory::default();
    // Guard, Lieutenant and Sentry: their DAT lines and the next stage, then the repeat lines.
    for (npc, stage, line, again) in [(G, 0, (0x25001648, 0x2824fd4c), (0x25001648, 0x63691d8f)),
        (0x4700157e, 1, (0x25001649, 0x8b6b186f), (0x25001649, 0x63691d8f)),
        (0x47001581, 2, (0x2500164c, 0x8b6b1869), (0x2500164c, 0x8b6b186b))] {
        let o = run(t, Trigger::Use, npc, 1, None, &book_at(stage), &none);
        // The Guard starts the run (at the time the test runs at, 0).
        let mut want = vec![Q::Stage { quest: SHELTER, stage: stage + 1 }];
        if stage == 0 { want.insert(0, Q::Started { quest: SHELTER, at: 0 }); }
        assert_eq!((o.tells.as_slice(), o.quests.as_slice()), (&[line][..], want.as_slice()));
        assert_eq!(o.lines, [t.stage(SHELTER, stage + 1).unwrap().1]);
        assert_eq!(run(t, Trigger::Use, npc, 1, None, &book_at(stage + 1), &none).tells, [again]);
    }
    assert_eq!(run(t, Trigger::Use, 0x4700157e, 1, None, &book_at(0), &none).tells, [(0x25001649, 0x8b6b186d)]);
    // A Digger drops the tool at stage 3 only, and only to a player without one.
    assert_eq!(kill(&book_at(3), &none, &Progression::default(), 0x4700158b, 0, 0).unwrap().given, [TOOL]);
    assert!(kill(&book_at(3), &bag_with(TOOL), &Progression::default(), 0x4700158b, 0, 0).unwrap().given.is_empty());
    assert!(kill(&book_at(2), &none, &Progression::default(), 0x4700158b, 0, 0).is_none());
    // The Scout: the tool for three potions and 40 XP; without it, her question.
    let bag = bag_with(TOOL);
    let o = run(t, Trigger::Use, 0x47001580, 1, None, &book_at(3), &bag);
    assert_eq!(o.tells, [(0x2500164b, 0x11ded2d4)]);
    assert!(matches!(o.items[0], I::ConsumeDense { item } if item == ITEM_NAMESPACE + 1));
    let granted: Vec<u32> = o.items[1..].iter().map(|c| match c { I::Grant(i) => i.definition, _ => 0 }).collect();
    assert_eq!(granted, [0x47001583, 0x47001584, 0x47001585]);
    assert!(matches!(o.changes.as_slice(), [P::AwardXp(40)]));
    assert_eq!(o.quests, [Q::Stage { quest: SHELTER, stage: 4 }]);
    assert_eq!(run(t, Trigger::Use, 0x47001580, 1, None, &book_at(3), &none).tells, [(0x2500164b, 0x8b6b1865)]);
    // A Sentinel drops the glyph at stage 4; the glyph on the Menhir Portal goes to the shard room.
    assert_eq!(kill(&book_at(4), &none, &Progression::default(), 0x4700158c, 0, 0).unwrap().given, [GLYPH]);
    let portal = 0x814002990005000d;
    assert!(!t.answers(Trigger::ItemUsedOn, 0x47001451, 0x8140029913da1c16, Some(GLYPH), &book_at(4), 0), "the quick exit is not the Menhir Portal");
    let o = run(t, Trigger::ItemUsedOn, 0x47001451, portal, Some(GLYPH), &book_at(4), &bag_with(GLYPH));
    assert!(matches!(o.items.as_slice(), [I::ConsumeDense { .. }]));
    assert_eq!(o.moveto, Some(Placement { cell: 0x0299014d, coordinates: [1.0, 1.0, 1.19], heading: 0 }));
    assert_eq!(o.quests, [Q::Stage { quest: SHELTER, stage: 5 }]);
    assert_eq!(run(t, Trigger::Use, 0x47001451, portal, None, &book_at(4), &none).tells, [(0x2500158c, 0x3745a723)]);
    // The Advance Shelter Scout ends it with 65 XP (105 in all); the shard leads to Dereth.
    let o = run(t, Trigger::Use, 0x4700157f, 1, None, &book_at(5), &none);
    assert_eq!(o.tells, [(0x2500164a, 0x8b6b1863)]);
    assert!(matches!(o.changes.as_slice(), [P::AwardXp(65)]));
    assert!(t.stage(SHELTER, 6).unwrap().0);
    let shard = 0x814002990005000e;
    let dereth = Placement { cell: 0x74ba0008, coordinates: [5.11786, 154.12, 266.168], heading: 96 };
    let o = run(t, Trigger::Use, 0x4700079a, shard, None, &book_at(6), &none);
    assert_eq!((o.moveto, o.quests.as_slice(), o.range), (Some(dereth), &[Q::Stage { quest: SHELTER, stage: 7 }][..], 7.56));
    assert_eq!(run(t, Trigger::Use, 0x4700079a, shard, None, &book_at(7), &none).moveto, Some(dereth));
    assert!(run(t, Trigger::Use, 0x4700079a, shard, None, &book_at(5), &none).moveto.is_none());
    // A training stone reads its own table.
    assert_eq!(run(t, Trigger::Use, 0x47001579, 1, None, &book_at(2), &none).tells, [(0x25001644, 0x6c459364)]);
}

#[test]
fn a_kill_task_counts_to_its_goal_and_pays_once() {
    let reg = HEADERS[0].to_owned() + "\nhunt\t0x40000077\t0x25000001\t0x1\tHunt\t3\tkiller\t\t\t\t\ttest\n";
    let stages = HEADERS[1].to_owned() + "\nhunt\t1\t0\tKill three.\ttest\nhunt\t2\t1\tDone.\ttest\n";
    let sets = HEADERS[2].to_owned() + "\nkill\t0x47000555\t\tKill\t\thunt\t1\t1\t\t\ttest\ndone\t\t\tQuestSuccess\t\t\t\t\t\t\ttest\n";
    let acts = HEADERS[3].to_owned() + "\nkill\t1\tIncrementQuest\t0\thunt\t\t\t\t\ttest\nkill\t2\tInqQuestCount\t0\thunt\t3\tdone\t\t\ttest\n\
        done\t1\tAwardXP\t0\t50\t\t\t\t\ttest\ndone\t2\tSetStage\t0\thunt\t2\t\t\t\ttest\n";
    let t = Tables::parse([&reg, &stages, &sets, &acts], ["r", "s", "e", "a"]).unwrap();
    let quest = 0x40000077;
    let mut book = Book::default().apply(&Q::Stage { quest, stage: 1 }).unwrap();
    let kill = |book: &Book| t.run(Trigger::Kill, 0x47000555, 0, None, book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 0, &|_, _, _| None).unwrap();
    for n in 1..=2 {
        let o = kill(&book).unwrap();
        assert!(o.changes.is_empty(), "no reward below the goal");
        book = o.quests.iter().try_fold(book, |b, c| b.apply(c)).unwrap();
        assert_eq!(book.count(quest), n);
    }
    let o = kill(&book).unwrap();
    assert!(matches!(o.changes.as_slice(), [P::AwardXp(50)]));
    book = o.quests.iter().try_fold(book, |b, c| b.apply(c)).unwrap();
    assert_eq!((book.count(quest), book.stage(quest)), (3, 2));
    assert!(kill(&book).is_none(), "kills after the goal do nothing");
}

/// A quest with the registry's rules: level 5, an hour's time limit, a day's repeat delay and two
/// solves at most. Its giver branches on InqLevel and InqQuestReady first, as Vault rows do.
fn ruled() -> Tables {
    let reg = HEADERS[0].to_owned() + "\nrule\t0x40000078\t0x25000001\t0x1\tRule\t2\tkiller\t5\t3600\t2\t86400\ttest\n\
        pre\t0x40000079\t0x25000001\t0x1\tPre\t\tkiller\t\t\t\t\ttest\n";
    let stages = HEADERS[1].to_owned() + "\nrule\t1\t0\tKill two.\ttest\nrule\t2\t1\tDone.\ttest\npre\t1\t1\tDone.\ttest\n";
    let sets = HEADERS[2].to_owned() + "\ngiver\t0x47000001\t\tUse\t\trule\t0\t0\t\t\ttest\n\
        level_ok\t\t\tQuestSuccess\t\t\t\t\t\t\ttest\nlow\t\t\tQuestFailure\t\t\t\t\t\t\ttest\n\
        start\t\t\tQuestSuccess\t\t\t\t\t\t\ttest\nwait\t\t\tQuestFailure\t\t\t\t\t\t\ttest\n\
        again\t0x47000001\t\tUse\t\trule\t2\t2\t\t\ttest\n\
        hand_in\t0x47000001\t\tUse\t\trule\t1\t1\t\t\ttest\npaid\t\t\tQuestSuccess\t\t\t\t\t\t\ttest\n\
        kill\t0x47000555\t\tKill\t\trule\t1\t1\t\t\ttest\n\
        gate\t0x47000002\t\tUse\t\t\t\t\t\t\ttest\nopen\t\t\tQuestSuccess\t\t\t\t\t\t\ttest\nshut\t\t\tQuestFailure\t\t\t\t\t\t\ttest\n";
    let acts = HEADERS[3].to_owned() + "\ngiver\t1\tInqLevel\t0\trule\tlevel_ok\tlow\t\t\ttest\n\
        level_ok\t1\tInqQuestReady\t0\trule\tstart\twait\t\t\ttest\n\
        low\t1\tTell\t0\t0x25000001\t0x2\t\t\t\ttest\n\
        start\t1\tSetStage\t0\trule\t1\t\t\t\ttest\n\
        wait\t1\tMessage\t0\tYou must wait.\t\t\t\t\ttest\n\
        again\t1\tInqQuestReady\t0\trule\tstart\twait\t\t\ttest\n\
        hand_in\t1\tInqQuestCount\t0\trule\t2\tpaid\t\t\ttest\n\
        paid\t1\tAwardGold\t0\t25\t\t\t\t\ttest\npaid\t2\tGiveItem\t0\t0x47000ed7\t1\t\t\t\ttest\npaid\t3\tSetStage\t0\trule\t2\t\t\t\ttest\n\
        kill\t1\tIncrementQuest\t0\trule\t\t\t\t\ttest\n\
        gate\t1\tInqSolved\t0\tpre\topen\tshut\t\t\ttest\n\
        open\t1\tMessage\t0\topen\t\t\t\t\ttest\nshut\t1\tMessage\t0\tshut\t\t\t\t\ttest\n";
    Tables::parse([&reg, &stages, &sets, &acts], ["r", "s", "e", "a"]).unwrap()
}
fn at_level(level: u16) -> Progression {
    let mut p = Progression::default();
    while p.level().number < level { p = p.apply(&P::AwardXp(p.level_span().unwrap_or(1).max(1))).unwrap(); }
    p
}
fn play(t: &Tables, trigger: Trigger, entity: u32, book: &Book, bag: &Inventory, p: &Progression, now: u64) -> Option<Outcome> {
    t.run(trigger, entity, 9, None, book, bag, p, first(bag), 0.0, now, &item).unwrap()
}
fn commit(book: &Book, o: &Outcome) -> Book { o.quests.iter().try_fold(book.clone(), |b, c| b.apply(c)).unwrap() }

#[test]
fn the_registry_rules_gate_a_run_and_time_it() {
    let t = ruled();
    let quest = 0x40000078;
    let none = Inventory::default();
    // Below level 5 the giver's refusal line; the engine itself refuses a start the rows let by.
    let o = play(&t, Trigger::Use, 0x47000001, &Book::default(), &none, &at_level(4), 1000).unwrap();
    assert_eq!((o.tells.as_slice(), o.quests.len()), (&[(0x25000001, 2)][..], 0));
    assert!(t.perform_set("start", &Book::default(), &none, &at_level(4), ITEM_NAMESPACE + 1, &item).is_err());
    // At level 5 the run starts at its time, with a fresh counter.
    let p = at_level(5);
    let o = play(&t, Trigger::Use, 0x47000001, &Book::default(), &none, &p, 1000).unwrap();
    assert_eq!(o.quests, [Q::Started { quest, at: 1000 }, Q::Stage { quest, stage: 1 }]);
    let mut book = commit(&Book::default(), &o);
    assert_eq!(book.timer(quest).started, 1000);
    // Kills count while the run lasts; after the hour it counts as stage 0 and kills do nothing.
    book = commit(&book, &play(&t, Trigger::Kill, 0x47000555, &book, &none, &p, 2000).unwrap());
    assert_eq!(book.count(quest), 1);
    assert_eq!(t.stage_now(&book, quest, 4601), 0);
    assert!(play(&t, Trigger::Kill, 0x47000555, &book, &none, &p, 4601).is_none());
    // Started anew after the limit: a new start time and the counter from 0.
    let o = play(&t, Trigger::Use, 0x47000001, &book, &none, &p, 5000).unwrap();
    assert_eq!(o.quests, [Q::Started { quest, at: 5000 }, Q::Stage { quest, stage: 1 }]);
    book = commit(&book, &o);
    assert_eq!(book.count(quest), 0);
    for now in [5100, 5200] { book = commit(&book, &play(&t, Trigger::Kill, 0x47000555, &book, &none, &p, now).unwrap()); }
    // The hand-in pays gold and a generic item (the definition catalog's visual) and solves it.
    let o = play(&t, Trigger::Use, 0x47000001, &book, &none, &p, 5300).unwrap();
    assert!(matches!(o.changes.as_slice(), [P::AwardGold(25)]));
    assert!(matches!(&o.items[..], [I::Grant(i)] if i.definition == 0x47000ed7 && i.visual == TEST_VISUAL));
    assert_eq!(o.quests, [Q::Stage { quest, stage: 2 }, Q::Solved { quest, at: 5300 }]);
    book = commit(&book, &o);
    assert_eq!(book.timer(quest), crate::quests::Timer { started: 5000, completed: 5300, solves: 1 });
    // Not again for a day; then a second run; after two solves never again.
    assert_eq!(play(&t, Trigger::Use, 0x47000001, &book, &none, &p, 5300 + 86399).unwrap().lines, ["You must wait."]);
    let o = play(&t, Trigger::Use, 0x47000001, &book, &none, &p, 5300 + 86400).unwrap();
    assert_eq!(o.quests[0], Q::Started { quest, at: 5300 + 86400 });
    book = commit(&book, &o);
    book = book.apply(&Q::Stage { quest, stage: 2 }).unwrap().apply(&Q::Solved { quest, at: 100_000 }).unwrap();
    assert!(!t.ready(&book, quest, u64::MAX / 2));
    assert_eq!(play(&t, Trigger::Use, 0x47000001, &book, &none, &p, u64::MAX / 2).unwrap().lines, ["You must wait."]);
}

#[test]
fn a_prerequisite_is_a_solved_quest() {
    let t = ruled();
    let none = Inventory::default();
    let p = Progression::default();
    assert_eq!(play(&t, Trigger::Use, 0x47000002, &Book::default(), &none, &p, 1).unwrap().lines, ["shut"]);
    // A completed stage recorded before solves were counts, as does a recorded solve.
    let old = Book::default().apply(&Q::Stage { quest: 0x40000079, stage: 1 }).unwrap();
    assert_eq!(play(&t, Trigger::Use, 0x47000002, &old, &none, &p, 1).unwrap().lines, ["open"]);
}

#[test]
fn the_new_quest_records_round_trip_and_old_records_replay_without_times() {
    for c in [Q::Started { quest: 0x40000078, at: 1_793_000_000 }, Q::Solved { quest: 0x40000078, at: 7 }] {
        assert_eq!(Q::decode(&c.encode()).unwrap(), c);
    }
    assert!(Q::decode("started,40000078").is_err() && Q::decode("solved,4000007,1").is_err());
    // A run recorded before the change (stage and count only) has no start time: it never
    // times out, and its completed stage still counts as solved and ready to repeat.
    let t = ruled();
    let quest = 0x40000078;
    let old = ["stage,40000078,1", "count,40000078,2"].iter().try_fold(Book::default(), |b, r| b.apply(&Q::decode(r).unwrap())).unwrap();
    assert_eq!((t.stage_now(&old, quest, u64::MAX), old.timer(quest)), (1, crate::quests::Timer::default()));
    let done = old.apply(&Q::decode("stage,40000078,2").unwrap()).unwrap();
    assert!(t.solved(&done, quest) && t.ready(&done, quest, 0));
}

#[test]
fn a_level_proportional_award_takes_a_decimal_percent_and_its_cap() {
    let reg = HEADERS[0].to_owned() + "\n";
    let sets = HEADERS[2].to_owned() + "\npay\t0x47000001\t\tUse\t\t\t\t\t\t\ttest\n";
    let parse = |p: &str| Tables::parse([&reg, &(HEADERS[1].to_owned() + "\n"), &sets,
        &(HEADERS[3].to_owned() + &format!("\npay\t1\tAwardLevelProportionalXP\t0\t{p}\t\t\ttest\n"))], ["r", "s", "e", "a"]);
    let p = at_level(10);
    let span = p.level_span().unwrap();
    let award = |t: Tables| match play(&t, Trigger::Use, 0x47000001, &Book::default(), &Inventory::default(), &p, 1).unwrap().changes.as_slice() {
        [P::AwardXp(xp)] => *xp, other => panic!("{other:?}") };
    assert_eq!(award(parse("2.5\t\t").unwrap()), span * 250 / 10_000);
    assert_eq!(award(parse("2.5\t\t7").unwrap()), 7.min(span * 250 / 10_000));
    for bad in ["0\t\t", "2.555\t\t", "101\t\t", "2\t9\t5"] { assert!(parse(bad).is_err(), "{bad}"); }
}

#[test]
fn a_registry_rule_must_be_a_positive_number() {
    let stage = HEADERS[1].to_owned() + "\n";
    for (cols, why) in [("x\t\t\t", "min_level"), ("\t0\t\t", "time_limit_seconds"), ("\t\t70000\t", "max_solves"), ("\t\t\t-1", "repeat_seconds")] {
        let reg = HEADERS[0].to_owned() + &format!("\nq\t0x40000001\t0x25000001\t0x1\tQ\t\tkiller\t{cols}\tsrc\n");
        let err = Tables::parse([&reg, &stage, &(HEADERS[2].to_owned() + "\n"), &(HEADERS[3].to_owned() + "\n")], ["r", "s", "e", "a"]).unwrap_err();
        assert!(err.to_string().contains(why), "{why}: {err}");
    }
}

/// Each shipped hunt from the AC2 Vault library (`*-vault.tsv`), played from the offer to the
/// reward and the repeat delay: Exterminate the Green Phyntos Wasps (Krestovmik, 5 kills, 120 XP)
/// and Destroy the Cahi Takeru (Onakhe, 20 kills, 2% of the level's span).
#[test]
fn the_shipped_vault_hunts_play_through() {
    let t = shipped();
    let none = Inventory::default();
    for (key, npc, iid, target, kills, level) in [
        ("v181", 0x470042f0u32, 0x8140f2e375f7cf3bu64, 0x47000f9cu32, 5u8, 3u16),
        ("v283", 0x47003fe8, 0x8140f95e44094a91, 0x47003b2a, 20, 65),
    ] {
        let q = t.quest(key).unwrap_or_else(|| panic!("{key} is not shipped"));
        let quest = q.journal_id;
        assert_eq!((q.min_level, q.time_limit, q.repeat), (level, Some(604_800), Some(604_800)), "{key}");
        let use_npc = |book: &Book, p: &Progression, now: u64| t.run(Trigger::Use, npc, iid, None, book, &none, p, first(&none), 0.0, now, &item).unwrap()
            .unwrap_or_else(|| panic!("{key}: the NPC does not answer"));
        // Below the level: a refusal, nothing starts.
        let low = use_npc(&Book::default(), &at_level(level - 1), 1_000);
        assert!(low.quests.is_empty() && (low.tells.len() + low.lines.len()) == 1, "{key}: {low:?}");
        // The offer starts the run; each kill advances one phase; the NPC pays on return.
        let p = at_level(level);
        let o = use_npc(&Book::default(), &p, 1_000);
        assert_eq!(o.quests, [Q::Started { quest, at: 1_000 }, Q::Stage { quest, stage: 1 }], "{key}");
        assert_eq!(o.tells.len(), 1);
        let mut book = commit(&Book::default(), &o);
        assert!(use_npc(&book, &p, 1_100).quests.is_empty(), "{key}: talking mid-hunt changes nothing");
        for k in 1..=kills {
            let o = t.run(Trigger::Kill, target, 0, None, &book, &none, &p, first(&none), 0.0, 1_000 + u64::from(k), &item).unwrap()
                .unwrap_or_else(|| panic!("{key}: kill {k} does not count"));
            book = commit(&book, &o);
            assert_eq!(book.stage(quest), k + 1);
        }
        let o = use_npc(&book, &p, 2_000);
        assert!(matches!(o.changes.as_slice(), [P::AwardXp(xp)] if *xp > 0), "{key}: {:?}", o.changes);
        assert_eq!(o.quests.last(), Some(&Q::Solved { quest, at: 2_000 }));
        assert!(t.stage(quest, kills + 2).unwrap().0, "the last stage is completed");
        book = commit(&book, &o);
        // Not again within seven days; again after.
        assert!(use_npc(&book, &p, 2_000 + 604_799).quests.is_empty());
        assert_eq!(use_npc(&book, &p, 2_000 + 604_800).quests[0], Q::Started { quest, at: 2_000 + 604_800 });
    }
}

/// A retired hunt's state moves onto its Vault quest: a run in progress keeps its start and the
/// stage of its kills; a solved one keeps its solve time; the hunting book drops both.
#[test]
fn a_retired_hunts_state_moves_onto_its_vault_quest() {
    let t = shipped();
    let marsh = 0x4000fc20; // Thin the Marsh Armoredillos, now v209
    assert_eq!(t.quest("v209").map(|q| q.journal_id), Some(marsh));
    let mut book = Book::default();
    book.hunting.0.insert(marsh, crate::hunting_quests::State { count: 4, started: 1_000, completed: 0, rewarded_xp: 0 });
    book.hunting.0.insert(0x4000fc1e, crate::hunting_quests::State { count: 10, started: 50, completed: 900, rewarded_xp: 22_000 });
    let records = ["started,4000fc20,1000", "stage,4000fc20,5", "retire,4000fc20",
        "started,4000fc1e,50", "stage,4000fc1e,12", "solved,4000fc1e,900", "retire,4000fc1e"];
    let moved = records.iter().try_fold(book, |b, r| b.apply(&Q::decode(r).unwrap())).unwrap();
    assert!(moved.hunting.0.is_empty());
    assert_eq!((moved.stage(marsh), moved.timer(marsh).started), (5, 1_000));
    assert_eq!(t.stage_now(&moved, marsh, 1_000 + 604_799), 5, "the Vault's 7 days run from the hunt's start");
    assert!(t.solved(&moved, 0x4000fc1e) && t.stage(0x4000fc1e, 12).unwrap().0);
    assert!(Q::decode("retire,4000fc23").is_err(), "only a retired hunt can be retired");
}

/// Sister Nell Palnos' chain (the retired catalog hunts Ulu then Faisi Sclavus, v207 and v208):
/// the second link is offered only once the first is solved, and a link not ready falls back.
#[test]
fn a_vault_chain_offers_its_links_in_order() {
    let t = shipped();
    let (npc, iid) = (0x47004308u32, 0x8140d5d4c268a38bu64);
    let (ulu, faisi) = (t.quest("v207").unwrap().journal_id, t.quest("v208").unwrap().journal_id);
    assert_eq!((ulu, faisi), (0x4000fc1e, 0x4000fc1f), "the catalog ids are kept");
    let none = Inventory::default();
    let p = at_level(30);
    let talk = |book: &Book, now: u64| t.run(Trigger::Use, npc, iid, None, book, &none, &p, first(&none), 0.0, now, &item).unwrap().unwrap();
    // First visit: Ulu is offered, not Faisi.
    let o = talk(&Book::default(), 100);
    assert_eq!(o.quests[1], Q::Stage { quest: ulu, stage: 1 });
    // Ulu solved: Faisi is offered next.
    let solved = Book::default().apply(&Q::Stage { quest: ulu, stage: 12 }).unwrap().apply(&Q::Solved { quest: ulu, at: 200 }).unwrap();
    let o = talk(&solved, 300);
    assert_eq!(o.quests, [Q::Started { quest: faisi, at: 300 }, Q::Stage { quest: faisi, stage: 1 }]);
    // Both solved and waiting: no task; Ulu's delay over first: Ulu again.
    let both = solved.apply(&Q::Stage { quest: faisi, stage: 12 }).unwrap().apply(&Q::Solved { quest: faisi, at: 400 }).unwrap();
    let o = talk(&both, 500);
    assert!(o.quests.is_empty() && o.tells == [(0x25008b21, 0x09200efe)], "{o:?}");
    let o = talk(&both, 200 + 604_800);
    assert_eq!(o.quests[1], Q::Stage { quest: ulu, stage: 1 });
}

/// Journey to Linvak (Vault 64), a talk chain: Akkilea, Pelloran, Kanten, Akkilea.
#[test]
fn the_vault_talk_chain_journey_to_linvak_plays_through() {
    let t = shipped();
    let quest = t.quest("v64").unwrap().journal_id;
    let none = Inventory::default();
    let p = at_level(26);
    let akkilea = (0x47001a98u32, 0x8140f3984b7f353eu64);
    let pelloran = (0x47001c29u32, 0x814089d05d3b1b1eu64);
    let kanten = (0x47001a3cu32, 0x8140ab125e91fb5bu64);
    let talk = |who: (u32, u64), book: &Book, now: u64| t.run(Trigger::Use, who.0, who.1, None, book, &none, &p, first(&none), 0.0, now, &item).unwrap();
    assert!(talk(pelloran, &Book::default(), 1).is_none(), "Pelloran has nothing before the offer");
    let mut book = commit(&Book::default(), &talk(akkilea, &Book::default(), 10).unwrap());
    assert!(talk(kanten, &book, 20).is_none(), "Kanten waits for Pelloran");
    for (who, line, stage) in [(pelloran, (0x2500707b, 0x8f8dca2a), 2), (kanten, (0x25006f84, 0x8f8dca2a), 3)] {
        let o = talk(who, &book, 30).unwrap();
        assert_eq!((o.tells.as_slice(), o.quests.as_slice()), (&[line][..], &[Q::Stage { quest, stage }][..]));
        book = commit(&book, &o);
    }
    let o = talk(akkilea, &book, 40).unwrap();
    assert!(matches!(o.changes.as_slice(), [P::AwardXp(10_000)]));
    assert_eq!(o.quests.last(), Some(&Q::Solved { quest, at: 40 }));
    // The two-hour time limit: a run left at Pelloran's stage expires.
    let stale = commit(&Book::default(), &talk(akkilea, &Book::default(), 10).unwrap());
    assert_eq!(t.stage_now(&stale, quest, 10 + 7201), 0);
}

/// An Enter set names the world object it is around; its range is the radius.
#[test]
fn an_enter_set_is_a_point_and_a_radius() {
    let reg = HEADERS[0].to_owned() + "\nx\t0x40000001\t0x25000001\t0x1\tX\t\tkiller\t\t\t\t\tsrc\n";
    let stages = HEADERS[1].to_owned() + "\nx\t1\t1\tArrived.\tsrc\n";
    let acts = HEADERS[3].to_owned() + "\narrive\t1\tSetStage\t0\tx\t1\t\t\t\tsrc\n";
    let parse = |set: &str| Tables::parse([&reg, &stages, &(HEADERS[2].to_owned() + "\n" + set), &acts], ["r", "s", "e", "a"]);
    assert!(parse("arrive\t0x47000001\t\tEnter\t\tx\t0\t0\t20\t\tsrc\n").unwrap_err().to_string().contains("owner_iid"));
    let t = parse("arrive\t0x47000001\t0x8140000000000007\tEnter\t\tx\t0\t0\t20\t\tsrc\n").unwrap();
    assert_eq!(t.enter_points().collect::<Vec<_>>(), [(0x8140000000000007, 20.0)]);
    let none = Inventory::default();
    let o = t.run(Trigger::Enter, 0x47000001, 0x8140000000000007, None, &Book::default(), &none, &Progression::default(), first(&none), 0.0, 5, &item)
        .unwrap().unwrap_or_else(|| panic!("no Enter set"));
    assert_eq!(o.quests, [Q::Stage { quest: 0x40000001, stage: 1 }, Q::Solved { quest: 0x40000001, at: 5 }]);
    assert!(!t.answers(Trigger::Enter, 0x47000001, 0x8140000000000007, None, &commit(&Book::default(), &o), 6), "once");
}

/// ApplyEffect and RemoveEffect: a quest's curse is on for its seconds, runs out, and a reward
/// lifts it; the records round-trip.
#[test]
fn quest_effects_are_applied_expire_and_are_removed() {
    let reg = HEADERS[0].to_owned() + "\nx\t0x40000001\t0x25000001\t0x1\tX\t\tkiller\t\t\t\t\tsrc\n";
    let stages = HEADERS[1].to_owned() + "\n";
    let sets = HEADERS[2].to_owned() + "\ncurse\t0x47000001\t\tUse\t\t\t\t\t\t\tsrc\nlift\t0x47000002\t\tUse\t\t\t\t\t\t\tsrc\n";
    let acts = HEADERS[3].to_owned() + "\ncurse\t1\tApplyEffect\t0\t0x6f00020f\t86400\t\t\t\tsrc\ncurse\t2\tApplyEffect\t0\t0x6f000212\t86400\t\t\t\tsrc\n\
        lift\t1\tRemoveEffect\t0\t0x6f00020f\t\t\t\t\tsrc\nlift\t2\tRemoveEffect\t0\t0x6f000212\t\t\t\t\tsrc\n";
    let t = Tables::parse([&reg, &stages, &sets, &acts], ["r", "s", "e", "a"]).unwrap();
    let none = Inventory::default();
    let run = |npc: u32, book: &Book, now: u64| t.run(Trigger::Use, npc, 1, None, book, &none, &Progression::default(), first(&none), 0.0, now, &item).unwrap().unwrap();
    let o = run(0x47000001, &Book::default(), 1000);
    assert_eq!(o.quests, [Q::EffectOn { effect: 0x6f00020f, at: 1000, until: 87_400 }, Q::EffectOn { effect: 0x6f000212, at: 1000, until: 87_400 }]);
    let book = commit(&Book::default(), &o);
    assert_eq!(book.quest_effects(87_399).len(), 2);
    assert!(book.quest_effects(87_400).is_empty(), "it runs out at its time");
    let o = run(0x47000002, &book, 2000);
    assert_eq!(o.quests, [Q::EffectOff { effect: 0x6f00020f }, Q::EffectOff { effect: 0x6f000212 }]);
    assert!(commit(&book, &o).quest_effects(2000).is_empty());
    assert!(run(0x47000002, &Book::default(), 2000).quests.is_empty(), "removing an absent effect records nothing");
    for c in [Q::EffectOn { effect: 0x6f00020f, at: 5, until: 9 }, Q::EffectOff { effect: 0x6f000212 }] {
        assert_eq!(Q::decode(&c.encode()).unwrap(), c);
    }
    assert!(Q::decode("effect,4700020f,1,2").is_err() && Q::decode("effect,6f00020f,5,5").is_err());
    let bad = HEADERS[3].to_owned() + "\ncurse\t1\tApplyEffect\t0\t0x4700020f\t60\t\t\t\tsrc\n";
    assert!(Tables::parse([&reg, &stages, &sets, &bad], ["r", "s", "e", "a"]).is_err());
}

/// RandomGive draws by weight from the run's roll; IncrementCounter / InqCounter keep several
/// counters in one quest (and a new run clears them); a Crafted set names the crafted item;
/// a `fellowship` credit lets `fellow_kill` find the quest's Kill set.
#[test]
fn random_gives_counters_crafts_and_fellowship_credit() {
    let reg = HEADERS[0].to_owned() + "\nx\t0x40000001\t0x25000001\t0x1\tX\t\tfellowship\t\t\t\t\tsrc\n";
    let stages = HEADERS[1].to_owned() + "\nx\t1\t0\tGo.\tsrc\nx\t2\t1\tDone.\tsrc\n";
    let sets = HEADERS[2].to_owned() + "\nprize\t0x47000001\t\tUse\t\t\t\t\t\t\tsrc\n\
        raver\t0x47000002\t\tKill\t\tx\t1\t1\t\t\tsrc\nthrasher\t0x47000003\t\tKill\t\tx\t1\t1\t\t\tsrc\n\
        both\t\t\tQuestSuccess\t\t\t\t\t\t\tsrc\nhalf\t\t\tQuestSuccess\t\t\t\t\t\t\tsrc\n\
        made\t0x47000004\t\tCrafted\t\tx\t1\t1\t\t\tsrc\n";
    let acts = HEADERS[3].to_owned() + "\nprize\t1\tRandomGive\t0\t0x47000ed7:1,0x47000ed8:3\t\t\t\t\tsrc\n\
        raver\t1\tIncrementCounter\t0\tx\t1\t2\t\t\tsrc\nraver\t2\tInqCounter\t0\tx\t1\t2\thalf\t\tsrc\n\
        thrasher\t1\tIncrementCounter\t0\tx\t2\t2\t\t\tsrc\nthrasher\t2\tInqCounter\t0\tx\t1\t2\thalf\t\tsrc\n\
        half\t1\tInqCounter\t0\tx\t2\t2\tboth\t\tsrc\nboth\t1\tSetStage\t0\tx\t2\t\t\t\tsrc\n\
        made\t1\tSetStage\t0\tx\t2\t\t\t\tsrc\n";
    let t = Tables::parse([&reg, &stages, &sets, &acts], ["r", "s", "e", "a"]).unwrap();
    let none = Inventory::default();
    let p = Progression::default();
    let go = |trigger, entity, book: &Book, roll: f32| t.run(trigger, entity, 0, None, book, &none, &p, first(&none), roll, 10, &item).unwrap();
    // Weights 1 and 3: rolls below a quarter draw the first item.
    assert_eq!(go(Trigger::Use, 0x47000001, &Book::default(), 0.2).unwrap().given, [0x47000ed7]);
    assert_eq!(go(Trigger::Use, 0x47000001, &Book::default(), 0.3).unwrap().given, [0x47000ed8]);
    // Two counters, in any order; the quest finishes when both reach two.
    let mut book = Book::default().apply(&Q::Stage { quest: 0x40000001, stage: 1 }).unwrap();
    for entity in [0x47000002, 0x47000003, 0x47000002] { book = commit(&book, &go(Trigger::Kill, entity, &book, 0.0).unwrap()); }
    assert_eq!((book.subcount(0x40000001, 1), book.subcount(0x40000001, 2), book.stage(0x40000001)), (2, 1, 1));
    book = commit(&book, &go(Trigger::Kill, 0x47000003, &book, 0.0).unwrap());
    assert_eq!(book.stage(0x40000001), 2);
    assert_eq!(Q::decode("subcount,40000001,2,7").unwrap(), Q::SubCount { quest: 0x40000001, counter: 2, count: 7 });
    let restarted = book.apply(&Q::Started { quest: 0x40000001, at: 20 }).unwrap();
    assert_eq!(restarted.subcount(0x40000001, 1), 0, "a new run counts again");
    // A craft of the named item advances the quest.
    let at1 = Book::default().apply(&Q::Stage { quest: 0x40000001, stage: 1 }).unwrap();
    assert_eq!(go(Trigger::Crafted, 0x47000004, &at1, 0.0).unwrap().quests, [Q::Stage { quest: 0x40000001, stage: 2 }, Q::Solved { quest: 0x40000001, at: 10 }]);
    let bad = HEADERS[2].to_owned() + "\nmade\t0x47000004\t0x8140000000000001\tCrafted\t\t\t\t\t\t\tsrc\n";
    assert!(Tables::parse([&reg, &stages, &bad, &(HEADERS[3].to_owned() + "\n")], ["r", "s", "e", "a"]).is_err());
}

/// `fellow_kill` runs only the Kill sets of `fellowship` quests (the installed tables' Shelter is a
/// `killer` quest, so a fellow gets nothing from a Digger).
#[test]
fn a_killer_quest_gives_fellows_nothing() {
    let _ = shipped();
    let book = Book::default().apply(&Q::Stage { quest: SHELTER, stage: 3 }).unwrap();
    assert!(fellow_kill(&book, &Inventory::default(), &Progression::default(), 0x4700158b, 0).is_none());
    assert!(kill(&book, &Inventory::default(), &Progression::default(), 0x4700158b, 0, 0).is_some());
}

/// A quest the step generator writes from the Vault's walkthrough (Destroy the Toxic Grievvers,
/// Vault 227: Brela Shon, 20 kills, 2% of the level's span capped): offered at level 70, each kill
/// a stage, the reward on return; its Pyrealium title (no DAT string names it) is the Vault's.
#[test]
fn a_step_generated_quest_plays_through() {
    let t = shipped();
    let q = t.quest("v227").unwrap();
    assert_eq!((q.title_text.as_str(), q.min_level, q.journal_id), ("Destroy the Toxic Grievvers", 70, 0x400200e3));
    let quest = q.journal_id;
    let none = Inventory::default();
    let (npc, iid, grievver) = (0x47003fc3u32, 0x81408f7959013befu64, 0x47003a94u32);
    let talk = |book: &Book, p: &Progression, now| t.run(Trigger::Use, npc, iid, None, book, &none, p, first(&none), 0.0, now, &item).unwrap().unwrap();
    assert!(talk(&Book::default(), &at_level(69), 1).quests.is_empty(), "level 70");
    let p = at_level(70);
    let mut book = commit(&Book::default(), &talk(&Book::default(), &p, 10));
    for k in 1..=20u8 {
        book = commit(&book, &t.run(Trigger::Kill, grievver, 0, None, &book, &none, &p, first(&none), 0.0, 10 + u64::from(k), &item).unwrap().unwrap());
    }
    assert_eq!(book.stage(quest), 21);
    let o = talk(&book, &p, 100);
    assert!(matches!(o.changes.as_slice(), [P::AwardXp(xp)] if *xp > 0));
    assert_eq!(o.quests.last(), Some(&Q::Solved { quest, at: 100 }));
}

/// The kingdom state: SetKingdom, InqKingdom (one kingdom or any), AwardKingdomPoints with the
/// Vault's caps ('+100, to a maximum of 1200'), InqKingdomPoints; the records round-trip.
#[test]
fn kingdom_membership_and_points() {
    let reg = HEADERS[0].to_owned() + "\n";
    let sets = HEADERS[2].to_owned() + "\njoin\t0x47000001\t\tUse\t\t\t\t\t\t\tsrc\npay\t0x47000002\t\tUse\t\t\t\t\t\t\tsrc\n\
        ask\t0x47000003\t\tUse\t\t\t\t\t\t\tsrc\nyes\t\t\tQuestSuccess\t\t\t\t\t\t\tsrc\nno\t\t\tQuestFailure\t\t\t\t\t\t\tsrc\n";
    let acts = HEADERS[3].to_owned() + "\njoin\t1\tSetKingdom\t0\t3\t\t\t\t\tsrc\npay\t1\tAwardKingdomPoints\t0\t100\t1200\t\t\t\tsrc\n\
        ask\t1\tInqKingdom\t0\t3\tyes\tno\t\t\tsrc\nyes\t1\tInqKingdomPoints\t0\t1000\tno\t\t\t\tsrc\nyes\t2\tMessage\t0\tshadow\t\t\t\t\tsrc\nno\t1\tMessage\t0\tno\t\t\t\t\tsrc\n";
    let t = Tables::parse([&reg, &(HEADERS[1].to_owned() + "\n"), &sets, &acts], ["r", "s", "e", "a"]).unwrap();
    let none = Inventory::default();
    let go = |e: u32, b: &Book| t.run(Trigger::Use, e, 0, None, b, &none, &Progression::default(), first(&none), 0.0, 1, &item).unwrap().unwrap();
    assert_eq!(go(0x47000003, &Book::default()).lines, ["no"]);
    let b = commit(&Book::default(), &go(0x47000001, &Book::default()));
    assert_eq!(b.kingdom(), (3, 0));
    let b = Book::default().apply(&Q::Kingdom(3)).unwrap().apply(&Q::KingdomPoints(1150)).unwrap();
    assert_eq!(commit(&b, &go(0x47000002, &b)).kingdom().1, 1200, "the cap holds");
    let at_cap = b.apply(&Q::KingdomPoints(1200)).unwrap();
    assert!(go(0x47000002, &at_cap).quests.is_empty());
    assert_eq!(go(0x47000003, &b).lines, ["no", "shadow"]);
    for c in [Q::Kingdom(2), Q::KingdomPoints(77)] { assert_eq!(Q::decode(&c.encode()).unwrap(), c); }
    assert!(Q::decode("kingdom,4").is_err());
}

/// StartEscort / EndEscort mark the outcome; an EscortLost set names the escorted entity.
#[test]
fn escort_actions_and_the_escort_lost_trigger() {
    let reg = HEADERS[0].to_owned() + "\nx\t0x40000001\t0x25000001\t0x1\tX\t\tkiller\t\t\t\t\tsrc\n";
    let stages = HEADERS[1].to_owned() + "\nx\t1\t0\tLead.\tsrc\n";
    let sets = HEADERS[2].to_owned() + "\nlead\t0x47000001\t\tUse\t\tx\t0\t0\t\t\tsrc\narrive\t0x47000002\t0x8140000000000002\tEnter\t\tx\t1\t1\t\t\tsrc\n\
        lost\t0x47000009\t\tEscortLost\t\tx\t1\t1\t\t\tsrc\n";
    let acts = HEADERS[3].to_owned() + "\nlead\t1\tStartEscort\t0\t0x47000009\t\t\t\t\tsrc\nlead\t2\tSetStage\t0\tx\t1\t\t\t\tsrc\n\
        arrive\t1\tEndEscort\t0\t\t\t\t\t\tsrc\nlost\t1\tMessage\t0\tYour escort has fallen.\t\t\t\t\tsrc\nlost\t2\tSetStage\t0\tx\t0\t\t\t\tsrc\n";
    let t = Tables::parse([&reg, &stages, &sets, &acts], ["r", "s", "e", "a"]).unwrap();
    let none = Inventory::default();
    let go = |trigger, e: u32, iid: u64, b: &Book| t.run(trigger, e, iid, None, b, &none, &Progression::default(), first(&none), 0.0, 1, &item).unwrap().unwrap();
    let o = go(Trigger::Use, 0x47000001, 0, &Book::default());
    assert_eq!(o.escort, Some(0x47000009));
    let book = commit(&Book::default(), &o);
    assert!(go(Trigger::Enter, 0x47000002, 0x8140000000000002, &book).end_escort);
    let lost = go(Trigger::EscortLost, 0x47000009, 0, &book);
    assert_eq!((lost.lines.as_slice(), lost.quests.last()), (&["Your escort has fallen.".to_owned()][..], Some(&Q::Stage { quest: 0x40000001, stage: 0 })));
}
