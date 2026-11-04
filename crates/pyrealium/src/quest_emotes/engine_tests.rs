//! The engine's generic actions and triggers (delays, Goto, EraseQuest, counters, stat and
//! solve checks), on small tables of their own.
use super::*;
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::Progression;

const Q1: u32 = 0x40000071;
const NPC: u32 = 0x47000001;

/// Tables from rows: `sets` and `acts` are TSV lines (no header) of emote-sets and emote-actions.
fn tables(sets: &str, acts: &str) -> io::Result<Tables> {
    let reg = HEADERS[0].to_owned() + "\nq\t0x40000071\t0x25000001\t0x1\tQ\t\tkiller\t\t\t\t\tt\n";
    let stages = HEADERS[1].to_owned() + "\nq\t1\t0\tUnderway.\tt\nq\t2\t1\tDone.\tt\n";
    Tables::parse([&reg, &stages, &(HEADERS[2].to_owned() + "\n" + sets), &(HEADERS[3].to_owned() + "\n" + acts)], ["r", "s", "e", "a"])
}
fn use_npc(t: &Tables, book: &Book) -> Outcome {
    t.run(Trigger::Use, NPC, 9, None, book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 100, &|_, _, _| None)
        .unwrap().unwrap()
}
fn after(book: Book, o: &Outcome) -> Book { o.quests.iter().try_fold(book, |b, c| b.apply(c)).unwrap() }

#[test]
fn a_delay_pauses_the_set_and_resume_carries_on_through_branches() {
    let t = tables("talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\ninner\t\t\tGoto\t\t\t\t\t\t\tt\n",
        "talk\t1\tTell\t0\t0x25000001\t0x1\t\t\t\tt\n\
         talk\t2\tTell\t1500\t0x25000001\t0x2\t\t\t\tt\n\
         talk\t3\tGoto\t0\tinner\t\t\t\t\tt\n\
         talk\t4\tTell\t0\t0x25000001\t0x5\t\t\t\tt\n\
         inner\t1\tTell\t0\t0x25000001\t0x3\t\t\t\tt\n\
         inner\t2\tTell\t250\t0x25000001\t0x4\t\t\t\tt\n").unwrap();
    let book = Book::default();
    let o = use_npc(&t, &book);
    assert_eq!(o.tells, [(0x25000001, 1)]);
    let r = o.resume.clone().unwrap();
    assert_eq!((r.frames.as_slice(), r.after_ms), (&[("talk".to_owned(), 1)][..], 1500));
    let resume = |r: &Resume| t.resume(r, &book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 101, &|_, _, _| None).unwrap();
    // After the wait: the delayed line, then into `inner` until its own delay.
    let o = resume(&r);
    assert_eq!(o.tells, [(0x25000001, 2), (0x25000001, 3)]);
    let r = o.resume.clone().unwrap();
    assert_eq!((r.frames.as_slice(), r.after_ms), (&[("inner".to_owned(), 1), ("talk".to_owned(), 3)][..], 250));
    // Then the rest of `inner`, and back in `talk` after its Goto.
    let o = resume(&r);
    assert_eq!((o.tells.as_slice(), o.resume.is_none()), (&[(0x25000001, 4), (0x25000001, 5)][..], true));
    // A live edit that removed the set ends a pending resume quietly.
    let other = tables("talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\n", "talk\t1\tTell\t0\t0x25000001\t0x1\t\t\t\tt\n").unwrap();
    let o = other.resume(&r, &book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 101, &|_, _, _| None).unwrap();
    assert!(o.tells.is_empty() && o.resume.is_none());
}

#[test]
fn goto_erase_and_counters_change_the_quest_book() {
    let t = tables("talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\nstart\t\t\tGoto\t\t\t\t\t\t\tt\n",
        "talk\t1\tGoto\t0\tstart\t\t\t\t\tt\n\
         talk\t2\tSetCounter\t0\tq\t3\t7\t\t\tt\n\
         talk\t3\tDecrementCounter\t0\tq\t3\t\t\t\tt\n\
         start\t1\tSetStage\t0\tq\t1\t\t\t\tt\n").unwrap();
    let book = after(Book::default(), &use_npc(&t, &Book::default()));
    assert_eq!((book.stage(Q1), book.subcount(Q1, 3), book.timer(Q1).started), (1, 6, 100));
    let erase = tables("talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\n", "talk\t1\tEraseQuest\t0\tq\t\t\t\t\tt\n").unwrap();
    let solved = book.apply(&Q::Solved { quest: Q1, at: 50 }).unwrap();
    let o = use_npc(&erase, &solved);
    assert_eq!(o.quests, [Q::Erase { quest: Q1 }]);
    let erased = after(solved, &o);
    assert_eq!((erased.stage(Q1), erased.subcount(Q1, 3), erased.timer(Q1).started, erased.timer(Q1).solves), (0, 0, 0, 1));
    // The record survives the journal.
    assert_eq!(Q::decode(&Q::Erase { quest: Q1 }.encode()).unwrap(), Q::Erase { quest: Q1 });
    // Nothing to erase: no record.
    assert!(use_npc(&erase, &Book::default()).quests.is_empty());
}

#[test]
fn stat_solve_and_item_checks_branch() {
    let t = tables("talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\nyes\t\t\tQuestSuccess\t\t\t\t\t\t\tt\nno\t\t\tQuestFailure\t\t\t\t\t\t\tt\n",
        "talk\t1\tInqStat\t0\tlevel\t2\t\tyes\tno\tt\n\
         talk\t2\tInqQuestSolves\t0\tq\t1\t2\tyes\tno\tt\n\
         talk\t3\tInqOwnsItems\t0\t0x47000099\t2\tyes\tno\t\tt\n\
         yes\t1\tMessage\t0\tyes\t\t\t\t\tt\n\
         no\t1\tMessage\t0\tno\t\t\t\t\tt\n").unwrap();
    assert_eq!(use_npc(&t, &Book::default()).lines, ["no", "no", "no"]);
    let solved = Book::default().apply(&Q::Solved { quest: Q1, at: 1 }).unwrap();
    assert_eq!(use_npc(&t, &solved).lines, ["no", "yes", "no"]);
    let thrice = [1, 2, 3].iter().fold(Book::default(), |b, at| b.apply(&Q::Solved { quest: Q1, at: *at }).unwrap());
    assert_eq!(use_npc(&t, &thrice).lines, ["no", "no", "no"], "above max");
}

#[test]
fn rows_are_checked_against_the_action_list() {
    let set = "talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\n";
    let err = |acts: &str| tables(set, acts).unwrap_err().to_string();
    assert!(err("talk\t1\tWave\t0\t\t\t\t\t\tt\n").contains("unknown action `Wave`"));
    assert!(err("talk\t1\tGiveItem\t0\t0x47000001\t\t\t\t\tt\n").contains("GiveItem needs its count (p2)"));
    assert!(err("talk\t1\tGiveItem\t0\t0x47000001\t17\t\t\t\tt\n").contains("p2 (count) must be a whole number 1 to 16"));
    assert!(err("talk\t1\tEndEscort\t0\tx\t\t\t\t\tt\n").contains("EndEscort takes 0 parameters"));
    assert!(err("talk\t1\tInqStat\t0\tstrength\t1\t\t\t\tt\n").contains("p1 (stat) must be one of level"));
    assert!(err("talk\t1\tGoto\t0\ttalk\t\t\t\t\tt\n").contains("cannot Goto itself"));
    assert!(err("talk\t1\tTell\t3600001\t0x1\t0x2\t\t\t\tt\n").contains("delay_ms"));
    // A personal trigger names its quest and no owner.
    let e = tables("died\t\t\tPlayerDeath\t\t\t\t\t\t\tt\n", "").unwrap_err().to_string();
    assert!(e.contains("names its quest"), "{e}");
    assert!(tables("died\t\t\tPlayerDeath\t\tq\t1\t1\t\t\tt\n", "died\t1\tEraseQuest\t0\tq\t\t\t\t\tt\n").is_ok());
}

#[test]
fn the_action_reference_is_written_from_the_list() {
    let page = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/quest-actions.md")).unwrap();
    assert_eq!(page, spec::reference(), "run `cargo run -p pyrealium --bin pyrealium-content -- actions --markdown > docs/quest-actions.md`");
}

#[test]
fn a_death_logout_or_expiry_runs_each_quests_set_on_quest_state_only() {
    let reg = HEADERS[0].to_owned() + "\nq\t0x40000071\t0x25000001\t0x1\tQ\t\tkiller\t\t60\t\t\tt\n";
    let stages = HEADERS[1].to_owned() + "\nq\t1\t0\tUnderway.\tt\nq\t2\t1\tDone.\tt\n";
    let sets = HEADERS[2].to_owned() + "\ndied\t\t\tPlayerDeath\t\tq\t1\t1\t\t\tt\nlate\t\t\tQuestExpired\t\tq\t1\t1\t\t\tt\n";
    let acts = HEADERS[3].to_owned() + "\ndied\t1\tMessage\t0\tYou fell; the run is over.\t\t\t\t\tt\ndied\t2\tEraseQuest\t0\tq\t\t\t\t\tt\n\
        late\t1\tEraseQuest\t0\tq\t\t\t\t\tt\n";
    let t = Tables::parse([&reg, &stages, &sets, &acts], ["r", "s", "e", "a"]).unwrap();
    let book = Book::default().apply(&Q::Started { quest: Q1, at: 100 }).unwrap().apply(&Q::Stage { quest: Q1, stage: 1 }).unwrap();
    let run = |trigger, now| t.run_personal(trigger, &book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, now, &|_, _, _| None).unwrap();
    let died = run(Trigger::PlayerDeath, 120);
    assert_eq!((died.len(), died[0].lines.as_slice(), died[0].quests.as_slice()), (1, &["You fell; the run is over.".to_owned()][..], &[Q::Erase { quest: Q1 }][..]));
    assert!(run(Trigger::QuestExpired, 120).is_empty(), "within the time limit");
    assert_eq!(run(Trigger::QuestExpired, 200)[0].quests, [Q::Erase { quest: Q1 }]);
    assert!(run(Trigger::PlayerDeath, 200).is_empty(), "an expired run is no longer underway");
    assert_eq!(t.timed_quests().collect::<Vec<_>>(), [Q1]);
    // Such a set may not reach a reward, even through a branch.
    let greedy = HEADERS[3].to_owned() + "\ndied\t1\tGoto\t0\tpay\t\t\t\t\tt\npay\t1\tAwardGold\t0\t5\t\t\t\t\tt\n";
    let sets = HEADERS[2].to_owned() + "\ndied\t\t\tPlayerDeath\t\tq\t1\t1\t\t\tt\npay\t\t\tGoto\t\t\t\t\t\t\tt\n";
    let e = Tables::parse([&reg, &stages, &sets, &greedy], ["r", "s", "e", "a"]).unwrap_err().to_string();
    assert!(e.contains("changes quests and talks only, but reaches AwardGold(5) in set pay"), "{e}");
}

#[test]
fn list_owners_stacks_lines_scenes_and_recipes() {
    let d = db::empty().unwrap();
    d.execute_batch("INSERT INTO quest (name, journal_id, title_table, title_key, title, goal, source) VALUES ('q', '0x40000071', '0x25000001', '0x1', 'Q', 3, 't');
        INSERT INTO quest_stage (quest, stage, completed, journal_text, source) VALUES ('q', 1, 0, 'Progress {count} / {goal}.', 't'), ('q', 2, 1, 'Done.', 't');
        INSERT INTO alias (name, kind, value, source) VALUES ('bugs', 'entities', '0x47000011,0x47000012', 't'), ('stones', 'objects', '0x8140000000030002,0x8140000000030004', 't');
        INSERT INTO quest_item (entity, stack_limit, source) VALUES ('0x470010f3', 65535, 't');
        INSERT INTO emote_set (name, priority, owner_entity, owner_iid, trigger, quest, stage_min, stage_max, source) VALUES
            ('kill', 1, 'bugs', NULL, 'Kill', 'q', '1', '1', 't'),
            ('talk', 2, '0x47000001', NULL, 'Use', NULL, NULL, NULL, 't'),
            ('gate', 3, NULL, 'stones', 'Travel', 'q', '1', '1', 't');
        INSERT INTO emote_action (emote_set, ord, action, p1, p2, source) VALUES
            ('kill', 1, 'IncrementQuest', 'q', '', 't'),
            ('talk', 1, 'Say', 'Hammer this iron.', '', 't'), ('talk', 2, 'GiveStack', '0x470010f3', '20', 't'),
            ('talk', 3, 'TakeQuantity', '0x470010f3', '15', 't'), ('talk', 4, 'GrantScene', '0x40000021', '', 't'),
            ('talk', 5, 'LearnRecipe', '0x6f001f0e', '', 't'),
            ('gate', 1, 'SetStage', 'q', '2', 't');").unwrap();
    let t = Tables::from_db(&d).unwrap();
    let book = Book::default().apply(&Q::Stage { quest: Q1, stage: 1 }).unwrap();
    let run = |trigger, entity, iid, book: &Book| t.run(trigger, entity, iid, None, book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 100, &item).unwrap();
    // Either creature of the list counts; the portal of the objects list moves the quest on.
    assert!(run(Trigger::Kill, 0x47000012, 5, &book).is_some() && run(Trigger::Kill, 0x47000013, 5, &book).is_none());
    assert_eq!(run(Trigger::Travel, 0x47000099, 0x8140000000030004, &book).unwrap().quests, [Q::Stage { quest: Q1, stage: 2 }, Q::Solved { quest: Q1, at: 100 }]);
    // Say, a stack given and part of it taken back, a scene and a recipe.
    let o = run(Trigger::Use, 0x47000001, 9, &Book::default()).unwrap();
    assert_eq!(o.says, [(None, "Hammer this iron.".to_owned())]);
    let bag = o.items.iter().try_fold(Inventory::default(), |b, c| b.apply(c)).unwrap();
    let iron: Vec<u32> = bag.items().filter(|i| i.definition == 0x470010f3).map(|i| i.quantity).collect();
    assert_eq!(iron, [5]);
    let after = o.quests.iter().try_fold(Book::default(), |b, c| b.apply(c)).unwrap();
    assert!(after.scenes().any(|s| s == 0x40000021) && after.knows(0x6f001f0e));
    assert_eq!(Q::decode(&Q::Recipe(0x6f001f0e).encode()).unwrap(), Q::Recipe(0x6f001f0e));
    // Short of the quantity: the whole set fails.
    d.execute("UPDATE emote_action SET p2 = '25' WHERE emote_set = 'talk' AND ord = 3", []).unwrap();
    let t = Tables::from_db(&d).unwrap();
    assert!(t.run(Trigger::Use, 0x47000001, 9, None, &Book::default(), &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 100, &item).is_err());
    // A Travel set changes quests and talks only.
    d.execute("INSERT INTO emote_action (emote_set, ord, action, p1, source) VALUES ('gate', 2, 'AwardGold', '5', 't')", []).unwrap();
    assert!(Tables::from_db(&d).unwrap_err().to_string().contains("changes quests and talks only"));
}

#[test]
fn a_capped_level_reward_pays_its_cap_at_the_last_level() {
    let t = tables("talk\t0x47000001\t\tUse\t\t\t\t\t\t\tt\n", "talk\t1\tAwardLevelProportionalXP\t0\t3\t\t500\t\t\tt\n").unwrap();
    let mut top = Progression::default();
    while top.level_span().is_some() { top = top.apply(&crate::progression::Command::AwardXp(top.level_span().unwrap())).unwrap(); }
    let o = t.run(Trigger::Use, NPC, 9, None, &Book::default(), &Inventory::default(), &top, ITEM_NAMESPACE + 1, 0.0, 100, &|_, _, _| None).unwrap().unwrap();
    assert_eq!(format!("{:?}", o.changes), "[AwardXp(500)]");
}
