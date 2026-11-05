use super::*;

#[test]
fn a_file_round_trips_through_rows() {
    let mut db = crate::content_db::empty().unwrap();
    let dir = std::env::temp_dir().join(format!("pyrealium-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = "pyrealium-encounters-v1\n# player: damage\nplayer\t20\t2\t1000\t3\n\n# npc rows\n# two lines\nnpc\t1\t2\t3\t4\t5\t6\t7\t8\n\
        regen\t2\t\t1000\n# the end\n";
    std::fs::write(dir.join("development-encounters.tsv"), text).unwrap();
    let imported = import(&mut db, &dir).unwrap();
    assert_eq!((imported.files, imported.rows, imported.documents), (1, 3, 0));
    assert_eq!(rebuild(&db, "development-encounters.tsv").unwrap().as_deref(), Some(text));
    // An empty field stays empty; fewer fields stay fewer.
    let (v2, v4): (Option<String>, Option<String>) = db.query_row("SELECT v2, v4 FROM encounter_rule WHERE kind = 'regen'", [],
        |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((v2.as_deref(), v4), (Some(""), None));
    // It survives the dump.
    let again = crate::content_db::open_dump(&crate::content_db::export(&db).unwrap()).unwrap();
    assert_eq!(rebuild(&again, "development-encounters.tsv").unwrap().as_deref(), Some(text));
    assert_eq!(rebuild(&again, "skills.tsv").unwrap(), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn text_serves_the_repository_dump() {
    assert!(text("skills.tsv").starts_with("pyrealium-skills-v1\n"));
    assert!(text("professions-v1.json").starts_with('{'));
    // A pack-backed world file reads as a world table of its rows' width.
    assert!(text("world.tsv").starts_with("pyrealium-world-v2\n"));
    assert!(text("world-observed-expanded.tsv").starts_with("pyrealium-world-v3\n"));
}
