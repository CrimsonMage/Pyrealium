//! Aliases, named counters and templates, on small databases of their own.
use super::*;
use crate::inventory::ITEM_NAMESPACE;
use crate::progression::Progression;

fn db_with(sql: &str) -> rusqlite::Connection {
    let d = db::empty().unwrap();
    d.execute_batch("INSERT INTO quest (name, journal_id, title_table, title_key, title, goal, source) VALUES
        ('qa', '0x40000081', '0x25000001', '0x1', 'A', 3, 't'), ('qb', '0x40000082', '0x25000001', '0x1', 'B', 2, 't');
        INSERT INTO quest_stage (quest, stage, completed, journal_text, source) VALUES
        ('qa', 1, 0, 'A underway.', 't'), ('qa', 2, 1, 'A done.', 't'), ('qb', 1, 0, 'B underway.', 't'), ('qb', 2, 1, 'B done.', 't');").unwrap();
    d.execute_batch(sql).unwrap();
    d
}
fn trigger(t: &Tables, trigger: Trigger, entity: u32, book: &Book) -> Option<Outcome> {
    t.run(trigger, entity, 9, None, book, &Inventory::default(), &Progression::default(), ITEM_NAMESPACE + 1, 0.0, 100, &item).unwrap()
}

#[test]
fn aliases_and_counter_names_stand_for_their_values() {
    let d = db_with("INSERT INTO alias (name, kind, value, source) VALUES ('guard', 'entity', '0x47000001', 't'),
        ('potion', 'entity', '0x47000099', 't'), ('gate', 'placement', '0x7d640021 10 12 0 64', 't');
        INSERT INTO quest_counter (quest, name, idx, source) VALUES ('qa', 'kills', 2, 't');
        INSERT INTO emote_set (name, priority, owner_entity, trigger, source) VALUES ('talk', 1, 'guard', 'Use', 't');
        INSERT INTO emote_action (emote_set, ord, action, p1, p2, p3, source) VALUES
        ('talk', 1, 'GiveItem', 'potion', '1', '', 't'), ('talk', 2, 'IncrementCounter', 'qa', 'kills', '5', 't'),
        ('talk', 3, 'MoveTo', 'gate', '', '', 't');");
    let t = Tables::from_db(&d).unwrap();
    let o = trigger(&t, Trigger::Use, 0x47000001, &Book::default()).unwrap();
    assert_eq!(o.given, [0x47000099]);
    assert_eq!(o.quests, [Q::SubCount { quest: 0x40000081, counter: 2, count: 1 }]);
    assert_eq!(o.moveto, Some(crate::movement::Placement { cell: 0x7d640021, coordinates: [10.0, 12.0, 0.0], heading: 64 }));
    // A name that is not the quest's counter, or an alias of another kind, is refused by name.
    d.execute("UPDATE emote_action SET p2 = 'deaths' WHERE ord = 2", []).unwrap();
    let e = Tables::from_db(&d).unwrap_err().to_string();
    assert!(e.contains("emote_action talk/2: p2 (counter) must be 0 to 255 or one of the quest's counter names"), "{e}");
    d.execute("UPDATE emote_action SET p2 = 'kills', p1 = 'qa' WHERE ord = 2", []).unwrap();
    d.execute("UPDATE emote_action SET p1 = 'gate' WHERE ord = 1", []).unwrap();
    assert!(Tables::from_db(&d).unwrap_err().to_string().contains("p1 (item) must be an EntityDesc DID (0x47...) or an entity alias"));
    d.execute("UPDATE emote_action SET p1 = 'potion' WHERE ord = 1", []).unwrap();
    d.execute("UPDATE alias SET value = '0x6f000001' WHERE name = 'potion'", []).unwrap();
    assert!(Tables::from_db(&d).unwrap_err().to_string().contains("quests alias potion: value does not fit its kind"));
}

const HUNT: &str = "INSERT INTO template (name, doc, source) VALUES ('hunt', 'Kill $goal of a creature for an NPC.', 't');
    INSERT INTO template_param (template, name, ord, source) VALUES ('hunt', 'npc', 1, 't'), ('hunt', 'target', 2, 't'),
        ('hunt', 'quest', 3, 't'), ('hunt', 'goal', 4, 't');
    INSERT INTO template_set (template, name, priority, owner_entity, trigger, quest, stage_min, stage_max, source) VALUES
        ('hunt', 'offer', 1, '$npc', 'Use', '$quest', '0', '0', 't'),
        ('hunt', 'kill', 2, '$target', 'Kill', '$quest', '1', '1', 't'),
        ('hunt', 'done', 3, NULL, 'QuestSuccess', NULL, NULL, NULL, 't');
    INSERT INTO template_action (template, emote_set, ord, action, p1, p2, p3, source) VALUES
        ('hunt', 'offer', 1, 'SetStage', '$quest', '1', '', 't'),
        ('hunt', 'kill', 1, 'IncrementQuest', '$quest', '', '', 't'),
        ('hunt', 'kill', 2, 'InqQuestCount', '$quest', '$goal', 'done', 't'),
        ('hunt', 'done', 1, 'SetStage', '$quest', '2', '', 't');
    INSERT INTO template_use (name, template, priority, source) VALUES ('a', 'hunt', 100, 't'), ('b', 'hunt', 200, 't');
    INSERT INTO template_arg (use_name, param, value, source) VALUES
        ('a', 'npc', '0x47000001', 't'), ('a', 'target', '0x47000011', 't'), ('a', 'quest', 'qa', 't'), ('a', 'goal', '3', 't'),
        ('b', 'npc', '0x47000002', 't'), ('b', 'target', '0x47000012', 't'), ('b', 'quest', 'qb', 't'), ('b', 'goal', '2', 't');";

#[test]
fn a_template_written_once_runs_for_each_use() {
    let d = db_with(HUNT);
    let t = Tables::from_db(&d).unwrap();
    let start = trigger(&t, Trigger::Use, 0x47000002, &Book::default()).unwrap();
    assert_eq!((start.set.as_str(), start.quests.as_slice()), ("b.offer", &[Q::Started { quest: 0x40000082, at: 100 }, Q::Stage { quest: 0x40000082, stage: 1 }][..]));
    // Use b's kill counts to its goal of 2 and branches to b's own `done`.
    let mut book = start.quests.iter().try_fold(Book::default(), |b, c| b.apply(c)).unwrap();
    for _ in 0..2 {
        let o = trigger(&t, Trigger::Kill, 0x47000012, &book).unwrap();
        book = o.quests.iter().try_fold(book, |b, c| b.apply(c)).unwrap();
    }
    assert_eq!(book.stage(0x40000082), 2);
    assert_eq!(book.stage(0x40000081), 0, "use a is untouched");
    assert!(trigger(&t, Trigger::Kill, 0x47000011, &book).is_none(), "a's kill waits for a's run");
    // The dump keeps the template rows and reads the same.
    assert_eq!(Tables::from_sql(&db::export(&d).unwrap()).unwrap(), t);
}

#[test]
fn a_template_use_is_checked_by_name() {
    let err = |sql: &str| { let d = db_with(HUNT); d.execute_batch(sql).unwrap(); Tables::from_db(&d).unwrap_err().to_string() };
    assert!(err("DELETE FROM template_arg WHERE use_name = 'a' AND param = 'goal';").contains("quests template_use a: template hunt needs `goal`"));
    assert!(err("INSERT INTO template_arg (use_name, param, value, source) VALUES ('a', 'colour', 'red', 't');")
        .contains("`colour` is not a parameter of template hunt"));
    let e = err("UPDATE template_action SET p2 = '$gaol' WHERE emote_set = 'kill' AND ord = 2;");
    assert!(e.contains("template_use a (template hunt set kill action 2): `$gaol` is not a parameter"), "{e}");
    // A filled-in row is checked as any other, naming its use.
    let e = err("UPDATE template_arg SET value = 'nobody' WHERE use_name = 'b' AND param = 'quest';");
    assert!(e.contains("template_use b (template hunt set"), "{e}");
}

#[test]
fn template_rows_merge_by_their_keys() {
    let base = db::export(&db_with(HUNT)).unwrap();
    let edit = |sql: &str| { let d = db::open_dump(&base).unwrap(); d.execute_batch(sql).unwrap(); db::export(&d).unwrap() };
    let mine = edit("UPDATE template_arg SET value = '4' WHERE use_name = 'a' AND param = 'goal';");
    let shipped = edit("UPDATE template_arg SET value = '5' WHERE use_name = 'b' AND param = 'goal';");
    let merged = db::merge(&base, &shipped, &mine).unwrap();
    assert!(merged.conflicts.is_empty());
    assert!(merged.text.contains("('a', 'goal', '4'") && merged.text.contains("('b', 'goal', '5'"), "{}", merged.text.lines().filter(|l| l.contains("template_arg")).collect::<Vec<_>>().join("\n"));
}
