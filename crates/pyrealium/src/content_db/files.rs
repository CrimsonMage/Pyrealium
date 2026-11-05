//! Content files as database rows: the creature, loot, skill, crafting, route and appearance
//! tables the server's subsystems read, and the JSON documents (professions, trophies, loot
//! limits). Each subsystem keeps its own parser and asks for its file's text (`text`); the text is
//! rebuilt from the rows, so what the parser reads is what the file held.
//!
//! A table file is a `content_file` row (its header: the format line and the comments before the
//! first row, kept as written, since some readers check them; its footer) and its rows in a table
//! named for what they hold, each field exactly as the file wrote it (NULL where a row has fewer
//! fields), with the comment lines written before a row as its `note`. A world file the server reads
//! whole is its world pack (`PACK_FILES`). A document is a `content_document` row; the packs'
//! reports are the world's evidence (`world_evidence`).
//!
//! The server installs its content database at start (`install`); anything that reads a file
//! before that, and tests and tools, read the repository's `content/content.sql`.
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rusqlite::{Connection, OptionalExtension};

use super::{Shape, sql};

/// Each row table with its fields in file column order (after its `file` and `ord`).
pub const TABLES: [(&str, &[&str]); 16] = [
    ("creature_combat", &["entity", "health", "damage", "interval_ms", "armor", "xp", "level", "variance", "attack", "defense",
        "perception", "homesick", "damage_type", "max_vigor"]),
    ("creature_ai_profile", &["entity", "perception", "homesick", "wandering", "wander_range", "wander_probability", "friend_table",
        "free_attacking", "idle_only", "loot_profile", "loot_proof"]),
    ("creature_skill", &["entity", "skills"]),
    ("definition", &["entity", "visual", "label"]),
    ("loot_item", &["entity", "visual", "name"]),
    ("observed_loot", &["monster", "profile", "observed_count", "items"]),
    ("encounter_rule", &["kind", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8"]),
    ("protected_entity", &["entity"]),
    ("crafting_level", &["skill", "level", "curve_xp", "learning_limit", "crafting_xp"]),
    ("skill", &["id", "record", "digest", "fingerprint", "race", "level", "credits", "cap", "curve", "kind", "required", "forbidden"]),
    ("skill_advancement", &["curve", "fingerprint", "digest", "xp"]),
    ("skill_effect", &["skill", "record", "kind", "reset_s", "vigor", "range", "targets", "right_hand", "left_hand", "story", "fx",
        "damage_table", "effects"]),
    ("skill_missile", &["skill", "record", "hook", "entity"]),
    ("skill_travel", &["skill", "class", "destinations"]),
    ("item_appearance", &["entity", "visual", "instances_pre2012", "instances_2012_2017", "mods"]),
    ("portal_route", &["portal", "entity", "cell", "x", "y", "z", "dest_cell", "dest_x", "dest_y", "dest_z", "heading", "level"]),
];

/// Every table file the server reads, by its path under `content/`, with the table its rows live in.
pub const FILES: [(&str, &str); 30] = [
    ("creature-combat.tsv", "creature_combat"), ("inferred-creature-combat-v1.tsv", "creature_combat"),
    ("regional-creature-combat-v1.tsv", "creature_combat"), ("wilderness-creature-combat-v1.tsv", "creature_combat"),
    ("arwic-mines-creature-combat-v1.tsv", "creature_combat"),
    ("creature-ai.tsv", "creature_ai_profile"), ("inferred-creature-ai-v1.tsv", "creature_ai_profile"),
    ("regional-creature-ai-v1.tsv", "creature_ai_profile"), ("wilderness-creature-ai-v1.tsv", "creature_ai_profile"),
    ("arwic-mines-creature-ai-v1.tsv", "creature_ai_profile"),
    ("creature-skills.tsv", "creature_skill"), ("7230/creature-skills.tsv", "creature_skill"),
    ("definitions.tsv", "definition"), ("loot-items.tsv", "loot_item"), ("observed-loot.tsv", "observed_loot"),
    ("development-encounters.tsv", "encounter_rule"), ("population-protected.txt", "protected_entity"),
    ("crafting-levels.tsv", "crafting_level"),
    ("skills.tsv", "skill"), ("7230/skills.tsv", "skill"),
    ("skill-advancement.tsv", "skill_advancement"), ("7230/skill-advancement.tsv", "skill_advancement"),
    ("skill-effects.tsv", "skill_effect"), ("7230/skill-effects.tsv", "skill_effect"),
    ("skill-missiles.tsv", "skill_missile"), ("7230/skill-missiles.tsv", "skill_missile"),
    ("skill-travel.tsv", "skill_travel"), ("7230/skill-travel.tsv", "skill_travel"),
    ("7230/item-appearance-observed.tsv", "item_appearance"), ("portal-routes.tsv", "portal_route"),
];
/// The world files the server reads whole, with the world pack each is (its canonical text,
/// `world::pack_text`): `world.tsv` is the bundled pack, the others are prepare-step packs.
pub const PACK_FILES: [(&str, &str); 4] = [
    ("world.tsv", "bundled-world"), ("world-observed-expanded.tsv", "world-observed-expanded"),
    ("training-world.tsv", "training-world"), ("arwic-mines-world-v1.tsv", "arwic-mines-world-v1"),
];
/// The JSON documents the server reads, then the world packs' source catalogs (which the prepare
/// step checks the packs against).
pub const DOCUMENTS: [&str; 17] = ["professions-v1.json", "native-trophies.json", "native-loot-limits.json", "treasure-weapons.json",
    "arwic-mines-v1.json", "skill-effects.report.json", "7230/skill-effects.report.json",
    "inferred-content-v1.json", "regional-content-v1.json", "wilderness-content-v1.json", "exploration-content-v1.json",
    "hunting-content-v1.json", "hunting-content-v2.json", "workshops-content-v1.json", "arwic-mines-world-v1.json",
    "npc-dialogue-v1.json", "gauntlet-content-v1.json"];
/// The packs' reports the server reads (their spawn rosters), from the world's evidence.
pub const REPORTS: [&str; 5] = ["inferred-world-v1.report.json", "regional-world-v1.report.json", "wilderness-world-v1.report.json",
    "hunting-world-v2.report.json", "arwic-mines-world-v1.report.json"];
/// The world pack `world.tsv` becomes (the bundled stationary placements; not a prepare-step pack).
pub const BUNDLED_PACK: (&str, i64) = ("bundled-world", 100);

/// The shapes of `content_file`, `content_document` and the row tables, in dump order.
pub fn shapes() -> &'static [Shape] {
    static SHAPES: OnceLock<Vec<Shape>> = OnceLock::new();
    SHAPES.get_or_init(|| {
        let mut out = vec![
            Shape { table: "content_file", order: "name", key: 1, columns: &["name", "rows_in", "header", "footer", "source"] },
            Shape { table: "content_document", order: "name", key: 1, columns: &["name", "body", "source"] },
        ];
        for (table, fields) in TABLES {
            let mut columns = vec!["file", "ord"];
            columns.extend_from_slice(fields);
            columns.push("note");
            out.push(Shape { table, order: "file, ord", key: 2, columns: Box::leak(columns.into_boxed_slice()) });
        }
        out
    })
}

fn bad(message: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message.into()) }
fn fields_of(table: &str) -> &'static [&'static str] {
    TABLES.iter().find(|(t, _)| *t == table).map(|(_, f)| *f).expect("a row table")
}

/// A file split as it is stored: the header lines, each row's fields with the comment lines
/// before it, and the footer lines.
struct Split<'a> { header: Vec<&'a str>, rows: Vec<(Vec<&'a str>, Vec<&'a str>)>, footer: Vec<&'a str> }
fn split(text: &str) -> Split<'_> {
    let mut out = Split { header: Vec::new(), rows: Vec::new(), footer: Vec::new() };
    let mut pending: Vec<&str> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let comment = line.is_empty() || line.starts_with('#') || (n == 0 && line.starts_with("pyrealium-"));
        if !comment {
            let note = std::mem::take(&mut pending);
            if out.rows.is_empty() { out.header = note; out.rows.push((Vec::new(), line.split('\t').collect())); }
            else { out.rows.push((note, line.split('\t').collect())); }
        } else {
            pending.push(line);
        }
    }
    if out.rows.is_empty() { out.header = pending; } else { out.footer = pending; }
    out
}
fn joined(lines: &[&str]) -> String { lines.iter().map(|l| format!("{l}\n")).collect() }

/// What `import` took in.
pub struct Imported { pub files: usize, pub rows: usize, pub documents: usize }

/// Moves the content files and documents of the content directory `content` into the database,
/// replacing those it holds (a file not there is left as the database has it). A pack-backed
/// file's rows are its pack's (`world::import_packs`); `world.tsv` becomes the bundled pack.
pub fn import(db: &mut Connection, content: &Path) -> io::Result<Imported> {
    let tx = db.transaction().map_err(sql)?;
    let mut imported = Imported { files: 0, rows: 0, documents: 0 };
    // Schema 15's first dumps kept pack-backed files here too.
    tx.execute("DELETE FROM content_file WHERE rows_in LIKE 'pack:%'", []).map_err(sql)?;
    let bundled = content.join(PACK_FILES[0].0);
    if bundled.exists() {
        let text = std::fs::read_to_string(&bundled)?.replace("\r\n", "\n");
        imported.rows += super::world::put_pack(&tx, BUNDLED_PACK.0, BUNDLED_PACK.1, "observed", &text,
            &format!("content/{}, imported by pyrealium-content import-files", PACK_FILES[0].0))?;
        imported.files += 1;
    }
    for (name, rows_in) in FILES {
        let path = content.join(name);
        if !path.exists() { continue; }
        let text = std::fs::read_to_string(&path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        let text = text.replace("\r\n", "\n");
        let parts = split(&text);
        let source = format!("content/{name}, imported by pyrealium-content import-files");
        for (table, _) in TABLES { tx.execute(&format!("DELETE FROM {table} WHERE file = ?1"), [name]).map_err(sql)?; }
        tx.execute("INSERT OR REPLACE INTO content_file (name, rows_in, header, footer, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![name, rows_in, joined(&parts.header), joined(&parts.footer), source]).map_err(sql)?;
        imported.files += 1;
        let fields = fields_of(rows_in);
        let mut columns = vec!["file", "ord"];
        columns.extend_from_slice(fields);
        columns.push("note");
        let marks: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let mut insert = tx.prepare(&format!("INSERT INTO {rows_in} ({}) VALUES ({})", columns.join(", "), marks.join(", "))).map_err(sql)?;
        for (ord, (note, row)) in parts.rows.iter().enumerate() {
            if row.len() > fields.len() {
                return Err(bad(format!("{name} row {}: {} fields ({rows_in} has {})", ord + 1, row.len(), fields.len())));
            }
            let mut values: Vec<rusqlite::types::Value> = vec![name.to_owned().into(), (ord as i64).into()];
            values.extend((0..fields.len()).map(|i| row.get(i).map_or(rusqlite::types::Value::Null, |v| v.to_string().into())));
            values.push(if note.is_empty() { rusqlite::types::Value::Null } else { note.join("\n").into() });
            insert.execute(rusqlite::params_from_iter(values)).map_err(|e| bad(format!("{name} row {}: {e}", ord + 1)))?;
            imported.rows += 1;
        }
    }
    for name in DOCUMENTS {
        let path = content.join(name);
        if !path.exists() { continue; }
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(|e| bad(format!("{name}: {e}")))?;
        tx.execute("INSERT OR REPLACE INTO content_document (name, body, source) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, super::world::document_text(&value)?,
                format!("content/{name}, imported by pyrealium-content import-files")]).map_err(sql)?;
        imported.documents += 1;
    }
    tx.commit().map_err(sql)?;
    Ok(imported)
}

/// A content file's text rebuilt from the database (`None` when it holds no such file): its
/// header, its rows (each after its note), its footer. A document's text is its JSON; so is a
/// world evidence file's.
pub fn rebuild(db: &Connection, name: &str) -> io::Result<Option<String>> {
    if let Some((_, pack)) = PACK_FILES.iter().find(|(n, _)| *n == name) { return super::world::pack_text(db, pack); }
    let file: Option<(String, String, String)> = db.query_row("SELECT rows_in, header, footer FROM content_file WHERE name = ?1", [name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(sql)?;
    let Some((rows_in, header, footer)) = file else {
        let body: Option<String> = db.query_row("SELECT body FROM content_document WHERE name = ?1", [name], |r| r.get(0)).optional().map_err(sql)?;
        if body.is_some() { return Ok(body); }
        return db.query_row("SELECT body FROM world_evidence WHERE name = ?1", [name], |r| r.get(0)).optional().map_err(sql);
    };
    let mut text = header;
    {
        let fields = fields_of(&rows_in);
        let mut statement = db.prepare(&format!("SELECT note, {} FROM {rows_in} WHERE file = ?1 ORDER BY ord", fields.join(", "))).map_err(sql)?;
        let mut rows = statement.query([name]).map_err(sql)?;
        while let Some(row) = rows.next().map_err(sql)? {
            if let Some(note) = row.get::<_, Option<String>>(0).map_err(sql)? { text.push_str(&note); text.push('\n'); }
            let values: Vec<String> = (1..=fields.len()).map_while(|i| row.get::<_, Option<String>>(i).transpose()).collect::<Result<_, _>>().map_err(sql)?;
            text.push_str(&values.join("\t"));
            text.push('\n');
        }
    }
    text.push_str(&footer);
    Ok(Some(text))
}

/// The installed content: every file and document the server reads, as text.
static INSTALLED: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
static INSTALLING: Mutex<()> = Mutex::new(());

fn render(db: &Connection) -> io::Result<HashMap<String, &'static str>> {
    let mut out = HashMap::new();
    let names: Vec<String> = FILES.iter().chain(&PACK_FILES).map(|(n, _)| n.to_string()).chain(DOCUMENTS.iter().map(|n| n.to_string()))
        .chain(REPORTS.iter().map(|n| n.to_string())).collect();
    for name in names {
        if let Some(text) = rebuild(db, &name)? { out.insert(name, &*Box::leak(text.into_boxed_str())); }
    }
    Ok(out)
}

/// Installs the content database the server runs on (its dump `text`), before anything reads a
/// file. Installing again is refused when it would change what was read.
pub fn install(text: &str) -> io::Result<()> {
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    let rendered = render(&super::open_dump(text)?)?;
    // A database from before the files were rows (an older prepared copy) is refused whole.
    let missing: Vec<&str> = FILES.iter().chain(&PACK_FILES).map(|(n, _)| *n).chain(DOCUMENTS).filter(|n| !rendered.contains_key(*n)).collect();
    if !missing.is_empty() {
        return Err(bad(format!("the content database lacks the content files {} (import them: pyrealium-content import-files)",
            missing.join(", "))));
    }
    match INSTALLED.get() {
        None => { let _ = INSTALLED.set(rendered); Ok(()) }
        Some(have) if *have == rendered => Ok(()),
        Some(_) => Err(io::Error::other("the content files were read before the server's content database was installed")),
    }
}

/// The repository's dump, for a process that installs none (tests, tools).
fn repository() -> &'static HashMap<String, &'static str> {
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(have) = INSTALLED.get() { return have; }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content").join(super::DUMP);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rendered = render(&super::open_dump(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let _ = INSTALLED.set(rendered);
    INSTALLED.get().expect("installed")
}

/// A content file's text (a table file under `content/`, a document, or a pack report), from the
/// installed content database. Panics on a name the database does not hold: every name is one of
/// `FILES`, `DOCUMENTS` or the read evidence, fixed at build time.
pub fn text(name: &str) -> &'static str {
    let files = INSTALLED.get().unwrap_or_else(repository);
    files.get(name).copied().unwrap_or_else(|| panic!("the content database holds no file {name}"))
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
