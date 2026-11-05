use std::path::Path;

use super::*;

fn content() -> &'static Path { Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content")) }
fn rows(text: &str) -> Vec<&str> { text.lines().skip(1).filter(|l| !l.is_empty() && !l.starts_with('#')).collect() }

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

/// Every content file the shipped dump holds reads as the file in `content/` did: a table file
/// byte for byte, a pack-backed file row for row under its own header, a document as the same JSON.
/// (The check retires with the files.)
#[test]
fn the_shipped_dump_holds_every_content_file() {
    let shipped = std::fs::read_to_string(content().join(crate::content_db::DUMP)).unwrap();
    let db = crate::content_db::open_dump(&shipped).unwrap();
    for (name, rows_in) in FILES {
        let Ok(file) = std::fs::read_to_string(content().join(name)) else { continue };
        let file = file.replace("\r\n", "\n");
        let rebuilt = rebuild(&db, name).unwrap().unwrap_or_else(|| panic!("the dump lacks {name}"));
        if rows_in.starts_with("pack:") {
            assert_eq!(rebuilt.lines().next(), file.lines().next(), "{name}");
            assert_eq!(rows(&rebuilt), rows(&file), "{name}");
        } else {
            let file = if file.ends_with('\n') { file } else { format!("{file}\n") };
            assert!(rebuilt == file, "{name} differs from its rows");
        }
    }
    for name in DOCUMENTS.iter().chain(&REPORTS) {
        let Ok(file) = std::fs::read_to_string(content().join(name)) else { continue };
        let rebuilt: serde_json::Value = serde_json::from_str(&rebuild(&db, name).unwrap().unwrap_or_else(|| panic!("the dump lacks {name}"))).unwrap();
        let mut file: serde_json::Value = serde_json::from_str(&file).unwrap();
        // A pack report's digests are re-made over its rows (`world::import_packs`).
        for digest in ["manifest_sha256", "source_sha256"] {
            if let (Some(have), Some(was)) = (rebuilt.get(digest), rebuilt.get(format!("original_{digest}"))) {
                file[format!("original_{digest}")] = was.clone();
                file[digest] = have.clone();
            }
        }
        assert!(rebuilt == file, "{name} differs from its document");
    }
}

#[test]
fn text_serves_the_repository_dump() {
    assert!(text("skills.tsv").starts_with("pyrealium-skills-v1\n"));
    assert!(text("professions-v1.json").starts_with('{'));
}
