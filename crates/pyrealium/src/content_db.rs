//! The content database: Pyrealium's content as SQLite rows, edited as data (the world builder,
//! or any SQLite tool) rather than code or loose files. It holds the scripted quests (their
//! rules: `quest_emotes`), the NPC dialogue rows and the world builder's placement hints; the
//! world itself joins it next (the plan: world placements, rules and routes).
//!
//! The checked-in form is `content/content.sql`, a dump this module writes deterministically
//! (`export`: the schema, then one `INSERT` per row in key order), so a change reads as a diff and
//! a conflict merges as text. `build` turns the dump into an editable `content.sqlite`; `export`
//! writes the edited database back. The server reads the dump through the same checks the rows
//! always had; the schema's own constraints refuse the plainest mistakes as they are typed.
//!
//! Every row carries a `family`: `hand` for hand-written content, or the name of the tool that
//! generates it (`vault`, `tools/import_vault_quests.py`), whose re-import replaces only its own
//! rows. A world builder's edits are a copy of the dump laid over the shipped one row by row
//! (`merge`).
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite::types::{Value, ValueRef};

use crate::quest_emotes::Tables;

pub mod overlay;
pub mod world;

/// Bumped when the schema changes; the dump sets it as `PRAGMA user_version`.
pub const SCHEMA_VERSION: i32 = 14;

/// The tables, in dump order (`content_db/schema.sql`).
pub const SCHEMA: &str = include_str!("content_db/schema.sql");

/// The dump's file name in the content directory.
pub const DUMP: &str = "content.sql";
/// The editable database `build` makes beside the dump (not checked in).
pub const DATABASE: &str = "content.sqlite";
/// The dump's name before the content database held more than quests: still read where no
/// `content.sql` is.
pub const OLD_DUMP: &str = "quests.sql";

/// The content dump of a directory: `content.sql`, else an older `quests.sql`.
pub fn dump_in(directory: &Path) -> PathBuf {
    let path = directory.join(DUMP);
    if !path.exists() && directory.join(OLD_DUMP).exists() { directory.join(OLD_DUMP) } else { path }
}

/// Each table with its columns (the four TSV tables first, in TSV column order; `emote_set`'s
/// `priority` and every table's `family` are the database's own), its dump order, and how many
/// leading columns are its key.
pub struct Shape { pub table: &'static str, pub columns: &'static [&'static str], pub order: &'static str, pub key: usize }
/// Optional columns added after a table's first schema, which an older dump lacks.
const OPTIONAL_SINCE: [(&str, &str); 5] = [("spawn_slot", "arrival"), ("spawn_slot", "step"), ("emote_set", "owner_slots"), ("emote_set", "phrase"),
    ("quest_item", "physical")];
/// `emote_set`'s columns beyond the TSV's (the database's own), in dump order after its name.
const SET_EXTRA: [&str; 3] = ["priority", "owner_slots", "phrase"];
/// A table's columns as the database holds them: `emote_set`'s extras after its name, then
/// `family`.
pub fn db_columns(shape: &Shape) -> Vec<&'static str> {
    let mut columns: Vec<&str> = shape.columns.to_vec();
    if shape.table == "emote_set" { for (i, c) in SET_EXTRA.iter().enumerate() { columns.insert(1 + i, c); } }
    columns.push("family");
    columns
}
/// The TSV tables (`FILES` order) are SHAPES[..TSV].
pub const TSV: usize = 4;
pub const SHAPES: [Shape; 10] = [
    Shape { table: "quest", order: "name", key: 1,
        columns: &["name", "journal_id", "title_table", "title_key", "title", "goal", "credit", "min_level",
            "time_limit_seconds", "max_solves", "repeat_seconds", "source"] },
    Shape { table: "quest_stage", order: "quest, stage", key: 2, columns: &["quest", "stage", "completed", "journal_text", "source"] },
    Shape { table: "emote_set", order: "priority, name", key: 1,
        columns: &["name", "owner_entity", "owner_iid", "trigger", "item", "quest", "stage_min", "stage_max", "range",
            "probability", "source"] },
    Shape { table: "emote_action", order: "emote_set, ord", key: 2,
        columns: &["emote_set", "ord", "action", "delay_ms", "p1", "p2", "p3", "p4", "p5", "source"] },
    Shape { table: "alias", order: "name", key: 1, columns: &["name", "kind", "value", "source"] },
    Shape { table: "quest_counter", order: "quest, idx", key: 2, columns: &["quest", "name", "idx", "source"] },
    Shape { table: "template", order: "name", key: 1, columns: &["name", "doc", "source"] },
    Shape { table: "template_param", order: "template, ord, name", key: 2, columns: &["template", "name", "ord", "doc", "source"] },
    Shape { table: "template_set", order: "template, priority, name", key: 2,
        columns: &["template", "name", "priority", "owner_entity", "owner_iid", "trigger", "item", "quest", "stage_min", "stage_max",
            "range", "probability", "source"] },
    Shape { table: "template_action", order: "template, emote_set, ord", key: 3,
        columns: &["template", "emote_set", "ord", "action", "delay_ms", "p1", "p2", "p3", "p4", "p5", "source"] },
];
const SHAPES_TAIL: [Shape; 19] = [
    Shape { table: "quest_recipe", order: "recipe", key: 1, columns: &["recipe", "ingredient", "quantity", "output", "source"] },
    Shape { table: "quest_item", order: "entity", key: 1, columns: &["entity", "visual", "stack_limit", "equipment_mask", "physical", "source"] },
    Shape { table: "template_use", order: "name", key: 1, columns: &["name", "template", "priority", "source"] },
    Shape { table: "template_arg", order: "use_name, param", key: 2, columns: &["use_name", "param", "value", "source"] },
    Shape { table: "instance_group", order: "name", key: 1, columns: &["name", "doc", "source"] },
    Shape { table: "instance_lane", order: "instance_group, lane", key: 2, columns: &["instance_group", "lane", "entry", "source"] },
    Shape { table: "spawn_slot", order: "instance_group, name, lane", key: 2, columns: &["name", "lane", "instance_group", "iid", "arrival", "step", "source"] },
    // NPCs and creatures quest rows name that the world does not place yet, with a guide's
    // position: the world builder's To place list.
    Shape { table: "placement_hint", order: "entity", key: 1,
        columns: &["entity", "what", "name", "north", "east", "cell", "x", "y", "quests", "source"] },
    // The world: its packs (with the evidence the prepare step checks), its placed objects, the
    // world builder's creature rules and spawn groups, and the portal routes.
    Shape { table: "world_pack", order: "ord", key: 1, columns: &["name", "ord", "kind", "source"] },
    Shape { table: "world_evidence", order: "name", key: 1, columns: &["name", "body", "source"] },
    Shape { table: "world_object", order: "pack, ord", key: 2,
        columns: &["pack", "iid", "ord", "entity", "visual", "cell", "x", "y", "z", "qw", "qx", "qy", "qz", "scale", "name", "source"] },
    // Base placements a world builder's world removes.
    Shape { table: "world_tombstone", order: "iid", key: 1, columns: &["iid", "source"] },
    Shape { table: "creature_stats", order: "target", key: 1,
        columns: &["target", "health", "damage", "armor", "level", "xp", "gold", "interval_ms", "range", "leash", "respawn_ms", "source"] },
    Shape { table: "creature_ai", order: "target", key: 1,
        columns: &["target", "perception", "homesick", "wandering", "wander_range", "wander_probability", "aggressive", "idle_only",
            "loot_proof", "source"] },
    Shape { table: "creature_loot", order: "target, ord", key: 2, columns: &["target", "ord", "weight", "items", "source"] },
    Shape { table: "patrol", order: "actor", key: 1, columns: &["actor", "pause_ms", "points", "source"] },
    Shape { table: "spawn_group", order: "id", key: 1, columns: &["id", "capacity", "regen_ms", "source"] },
    Shape { table: "spawn_group_member", order: "spawn_group, placement", key: 2, columns: &["spawn_group", "placement", "source"] },
    Shape { table: "portal_route", order: "portal", key: 1,
        columns: &["portal", "entity", "cell", "x", "y", "z", "dest_cell", "dest_x", "dest_y", "dest_z", "heading", "level", "source"] },
];
/// Every table, in dump order.
pub fn shapes() -> impl Iterator<Item = &'static Shape> { SHAPES.iter().chain(SHAPES_TAIL.iter()) }
pub fn shape(table: &str) -> &'static Shape { shapes().find(|s| s.table == table).expect("a known table") }

pub fn sql(e: rusqlite::Error) -> io::Error { io::Error::other(e.to_string()) }

/// An empty in-memory database with the schema.
pub fn empty() -> io::Result<Connection> {
    let db = Connection::open_in_memory().map_err(sql)?;
    db.execute_batch(&format!("PRAGMA foreign_keys = ON;\n{SCHEMA}")).map_err(sql)?;
    Ok(db)
}

/// A database holding the dump `text`, in memory: `content/content.sql`, or any SQL dump of a
/// content database (`sqlite3 .dump`, Python's `iterdump`), whatever order its rows come in. Its
/// tables must be this schema's (a missing one is created empty); a row naming a quest or set
/// that is not there is refused once every row is in.
pub fn open_dump(text: &str) -> io::Result<Connection> {
    let db = Connection::open_in_memory().map_err(sql)?;
    // References are checked once every row is in (the bundled SQLite turns them on by default).
    db.execute_batch("PRAGMA foreign_keys = OFF;").map_err(sql)?;
    execute_dump(&db, text).map_err(|e| io::Error::other(format!("content.sql: {e}")))?;
    db.execute_batch(SCHEMA).map_err(sql)?;
    for shape in shapes() {
        let mut statement = db.prepare(&format!("PRAGMA table_info({})", shape.table)).map_err(sql)?;
        let mut have: Vec<String> = statement.query_map([], |r| r.get::<_, String>(1)).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
        let mut want: Vec<String> = db_columns(shape).iter().map(|c| c.to_string()).collect();
        // An older dump's table lacks the optional columns added since: they are added empty
        // (export writes the table anew).
        for column in want.iter().filter(|c| !have.contains(c)).cloned().collect::<Vec<_>>() {
            if !OPTIONAL_SINCE.contains(&(shape.table, column.as_str())) { continue; }
            db.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {column} TEXT", shape.table)).map_err(sql)?;
            have.push(column);
        }
        have.sort();
        want.sort();
        if have != want {
            // An empty table of an earlier shape (a table added and reshaped before it held rows)
            // is made anew; a table with rows must match.
            let rows: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {}", shape.table), [], |r| r.get(0)).map_err(sql)?;
            if rows > 0 || !RESHAPED_EMPTY.contains(&shape.table) {
                return Err(io::Error::other(format!("content.sql: table {} has columns {}; this server reads {}", shape.table, have.join(", "), want.join(", "))));
            }
            db.execute_batch(&format!("DROP TABLE {}", shape.table)).map_err(sql)?;
            db.execute_batch(SCHEMA).map_err(sql)?;
        }
    }
    let broken: Option<(String, i64, String)> = db.query_row("PRAGMA foreign_key_check", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(Some).or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) }).map_err(sql)?;
    if let Some((table, row, parent)) = broken {
        return Err(io::Error::other(format!("content.sql: row {row} of {table} names a {parent} that does not exist")));
    }
    db.execute_batch("PRAGMA foreign_keys = ON;").map_err(sql)?;
    check_version(&db)?;
    Ok(db)
}

/// Tables whose shape changed before they ever held rows: a dump's empty one of the earlier shape
/// is made anew (any other table must match).
const RESHAPED_EMPTY: [&str; 1] = ["world_pack"];

/// The world's tables (`world::*`), which the quest-editing paths leave out of their dumps.
pub const WORLD_TABLES: [&str; 3] = ["world_pack", "world_evidence", "world_object"];

/// The dump `text` without the shipped world's rows (`WORLD_TABLES`, but a world builder's
/// `builder` pack), in export form: what the world
/// builder's quest editing exchanges (the shipped world is tens of megabytes and is not edited
/// there; a merge over the shipped dump keeps it).
pub fn without_world(text: &str) -> io::Result<String> {
    let db = open_dump(text)?;
    // The world builder's own placements (a `builder` pack) stay: they are its edits.
    db.execute_batch("DELETE FROM world_evidence;
        DELETE FROM world_object WHERE pack IN (SELECT name FROM world_pack WHERE kind <> 'builder');
        DELETE FROM world_pack WHERE kind <> 'builder';").map_err(sql)?;
    export(&db)
}

/// Runs a SQL dump statement by statement. SQLite copies the text it is given to prepare when that
/// text is not NUL-terminated, so handing it the whole rest of a large dump for each of its
/// statements (as `execute_batch` does) copies the dump once per row: tens of thousands of rows
/// made a load take most of a minute. A statement ends at a line ending in `;` once its single
/// quotes balance (SQL doubles a quote inside a string); comment lines do not count.
pub fn execute_dump(db: &Connection, text: &str) -> rusqlite::Result<()> {
    let (mut statement, mut quotes) = (String::new(), 0usize);
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if statement.is_empty() && (trimmed.is_empty() || trimmed.starts_with("--")) { continue; }
        statement.push_str(line);
        quotes += line.bytes().filter(|b| *b == b'\'').count();
        if quotes % 2 == 0 && trimmed.ends_with(';') {
            db.execute_batch(&statement)?;
            statement.clear();
            quotes = 0;
        }
    }
    if !statement.trim().is_empty() { db.execute_batch(&statement)?; }
    Ok(())
}

/// The dump `text` as `export` writes it (checked as the server checks it), so any dump of the
/// same rows becomes the same text.
pub fn canonical(text: &str) -> io::Result<String> {
    let db = open_dump(text)?;
    Tables::from_db(&db)?;
    export(&db)
}

pub fn check_version(db: &Connection) -> io::Result<()> {
    let version: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(sql)?;
    // An older dump reads as this schema (export writes it anew); a newer one is refused.
    if version > SCHEMA_VERSION {
        return Err(io::Error::other(format!("the content database is schema {version}; this server reads {SCHEMA_VERSION}")));
    }
    Ok(())
}

/// A table's rows as text (NULL as empty), in its columns' order and its dump order.
pub fn table(db: &Connection, shape: &Shape, extra: &[&str]) -> io::Result<Vec<Vec<String>>> {
    let columns: Vec<&str> = shape.columns.iter().chain(extra).copied().collect();
    let query = format!("SELECT {} FROM {} ORDER BY {}", columns.join(", "), shape.table, shape.order);
    let mut statement = db.prepare(&query).map_err(sql)?;
    let mut found = statement.query([]).map_err(sql)?;
    let mut out = Vec::new();
    while let Some(row) = found.next().map_err(sql)? {
        let mut fields = Vec::with_capacity(columns.len());
        for (i, column) in columns.iter().enumerate() {
            fields.push(match row.get_ref(i).map_err(sql)? {
                ValueRef::Null => String::new(),
                ValueRef::Integer(n) => n.to_string(),
                ValueRef::Real(x) => x.to_string(),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                ValueRef::Blob(_) => return Err(io::Error::other(format!("{}: a blob in column {column}", shape.table))),
            });
        }
        out.push(fields);
    }
    Ok(out)
}

/// A TSV field as the column's SQL value: blank optional fields are NULL, numbers are numbers.
pub fn typed(table: &str, column: &str, v: &str) -> Value {
    let text_column = matches!(column, "p1" | "p2" | "p3" | "p4" | "p5" | "action" | "source" | "journal_text" | "title" | "credit"
        | "trigger" | "name" | "quest" | "emote_set") || v.starts_with("0x") || v.starts_with("0X");
    if v.is_empty() {
        return if matches!(column, "p1" | "p2" | "p3" | "p4" | "p5") || (table == "emote_action" && column == "delay_ms") {
            if column == "delay_ms" { Value::Integer(0) } else { Value::Text(String::new()) }
        } else { Value::Null };
    }
    if !text_column {
        if let Ok(n) = v.parse::<i64>() { return Value::Integer(n); }
        if matches!(column, "range" | "probability" | "north" | "east" | "x" | "y" | "z" | "qw" | "qx" | "qy" | "qz" | "scale"
            | "leash" | "perception" | "homesick" | "wander_range" | "wander_probability" | "dest_x" | "dest_y" | "dest_z") {
            if let Ok(x) = v.parse::<f64>() { return Value::Real(x); }
        }
    }
    Value::Text(v.to_owned())
}

/// Replaces `family`'s placement hints with the TSV `text` (a header of `placement_hint`'s
/// columns, then one hint per line); an entity another family hints at is left as it is.
pub fn import_hints(db: &mut Connection, text: &str, family: &str) -> io::Result<usize> {
    let shape = shape("placement_hint");
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap_or("").split('\t').collect();
    if header != shape.columns {
        return Err(io::Error::other(format!("placement hints: the header must be {}", shape.columns.join(" TAB "))));
    }
    let tx = db.transaction().map_err(sql)?;
    tx.execute("DELETE FROM placement_hint WHERE family = ?1", [family]).map_err(sql)?;
    let marks: Vec<String> = (1..=shape.columns.len() + 1).map(|i| format!("?{i}")).collect();
    // An entity another family already hints at (hand rows, edited in the database) keeps its row.
    let query = format!("INSERT OR IGNORE INTO placement_hint ({}, family) VALUES ({})", shape.columns.join(", "), marks.join(", "));
    let mut count = 0;
    for (n, line) in lines.enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != shape.columns.len() { return Err(io::Error::other(format!("placement hints line {}: {} columns", n + 2, fields.len()))); }
        let mut values: Vec<Value> = fields.iter().zip(shape.columns).map(|(v, c)| typed("placement_hint", c, v)).collect();
        values.push(Value::Text(family.to_owned()));
        tx.execute(&query, rusqlite::params_from_iter(values)).map_err(|e| io::Error::other(format!("placement hints line {}: {e}", n + 2)))?;
        count += 1;
    }
    tx.commit().map_err(sql)?;
    Ok(count)
}

/// The database as its dump: the schema version and schema, then every row as one `INSERT`,
/// in key order. The same rows always give the same text.
pub fn export(db: &Connection) -> io::Result<String> {
    let mut out = format!("-- Pyrealium content (schema {SCHEMA_VERSION}). Written by `pyrealium-content export`;\n\
        -- edit content.sqlite (`pyrealium-content build`) or these rows, then export again.\n\
        PRAGMA user_version = {SCHEMA_VERSION};\n{SCHEMA}");
    for shape in shapes() {
        let columns = db_columns(shape);
        if shape.table != "emote_action" && !SHAPES[..TSV].iter().any(|s| s.table == shape.table) {
            // The later tables are written only when they have rows.
            let count: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {}", shape.table), [], |r| r.get(0)).map_err(sql)?;
            if count == 0 { continue; }
        }
        out.push('\n');
        let query = format!("SELECT {} FROM {} ORDER BY {}", columns.join(", "), shape.table, shape.order);
        let mut statement = db.prepare(&query).map_err(sql)?;
        let mut found = statement.query([]).map_err(sql)?;
        while let Some(row) = found.next().map_err(sql)? {
            let values: Vec<String> = (0..columns.len()).map(|i| row.get_ref(i).map(literal)).collect::<Result<_, _>>().map_err(sql)?;
            out.push_str(&format!("INSERT INTO {} ({}) VALUES ({});\n", shape.table, columns.join(", "), values.join(", ")));
        }
    }
    Ok(out)
}

fn literal(v: ValueRef) -> String {
    match v {
        ValueRef::Null => "NULL".to_owned(),
        ValueRef::Integer(n) => n.to_string(),
        ValueRef::Real(x) => format!("{x:?}"),
        // A line break is written as char(10) (char(13) for a carriage return), so every row stays
        // on one line of the dump.
        ValueRef::Text(t) => {
            let quoted = format!("'{}'", String::from_utf8_lossy(t).replace('\'', "''"));
            quoted.replace('\r', "' || char(13) || '").replace('\n', "' || char(10) || '")
        }
        ValueRef::Blob(b) => format!("X'{}'", b.iter().map(|c| format!("{c:02x}")).collect::<String>()),
    }
}

/// Replaces family `family`'s rows in every table with the rows the SQL `text` inserts (a
/// generator's output, each row naming `family`), checked as the server checks them.
pub fn import_sql(db: &mut Connection, text: &str, family: &str) -> io::Result<()> {
    let tx = db.transaction().map_err(sql)?;
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;").map_err(sql)?;
    let all: Vec<&Shape> = shapes().collect();
    for shape in all.iter().rev() {
        tx.execute(&format!("DELETE FROM {} WHERE family = ?1", shape.table), [family]).map_err(sql)?;
    }
    let count = |tx: &rusqlite::Transaction, table: &str| -> io::Result<i64> {
        tx.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE family <> ?1"), [family], |r| r.get(0)).map_err(sql)
    };
    let before: Vec<i64> = all.iter().map(|s| count(&tx, s.table)).collect::<io::Result<_>>()?;
    execute_dump(&tx, text).map_err(|e| io::Error::other(format!("the {family} rows: {e}")))?;
    // Every row the text adds is the family's (a row left at the default `hand` would not be
    // replaced by the next import).
    for (shape, was) in all.iter().zip(before) {
        if count(&tx, shape.table)? != was {
            return Err(io::Error::other(format!("the {family} rows add {} rows of another family (each row names family '{family}')", shape.table)));
        }
    }
    Tables::from_db(&tx)?;
    tx.commit().map_err(sql)
}

/// Writes the dump `text` into a new database file at `path` (replacing it).
pub fn build(text: &str, path: &Path) -> io::Result<()> {
    let db = open_dump(text)?;
    Tables::from_db(&db)?;
    if path.exists() { std::fs::remove_file(path)?; }
    let file = path.display().to_string();
    db.execute("VACUUM INTO ?1", [file]).map_err(sql)?;
    Ok(())
}

/// Opens a database file (`build`'s), checking its schema version.
pub fn open(path: &Path) -> io::Result<Connection> {
    let db = Connection::open(path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
    db.execute_batch("PRAGMA foreign_keys = ON;").map_err(sql)?;
    check_version(&db)?;
    // A database built by an older schema gains the tables added since (empty).
    db.execute_batch(SCHEMA).map_err(sql)?;
    Ok(db)
}

/// What a three-way merge (`merge`) kept: the merged dump and the rows both sides changed
/// (the overlay's version won), each named by table and key.
pub struct Merged { pub text: String, pub conflicts: Vec<String> }

/// Every row of a table by its key, with every column (`family` and `priority` included).
fn keyed(db: &Connection, shape: &Shape) -> io::Result<std::collections::BTreeMap<String, Vec<Value>>> {
    let columns = db_columns(shape);
    let mut statement = db.prepare(&format!("SELECT {} FROM {}", columns.join(", "), shape.table)).map_err(sql)?;
    let mut found = statement.query([]).map_err(sql)?;
    let mut out = std::collections::BTreeMap::new();
    while let Some(row) = found.next().map_err(sql)? {
        let values: Vec<Value> = (0..columns.len()).map(|i| row.get::<_, Value>(i)).collect::<Result<_, _>>().map_err(sql)?;
        let key = values[..shape.key].iter().map(text_of).collect::<Vec<_>>().join("/");
        out.insert(key, values);
    }
    Ok(out)
}
fn text_of(v: &Value) -> String {
    match v { Value::Text(t) => t.clone(), Value::Integer(n) => n.to_string(), Value::Real(x) => x.to_string(), _ => String::new() }
}

/// The world builder's quests (`overlay`, edited from the shipped dump `base`) laid over the
/// shipped dump as it is now (`shipped`), row by row on each table's key: a row only the
/// overlay changed (added, edited or deleted) is the overlay's, a row only the shipped dump
/// changed is the shipped one's, and a row both changed differently is the overlay's (listed in
/// `conflicts`). The result is checked as the server checks any dump.
pub fn merge(base: &str, shipped: &str, overlay: &str) -> io::Result<Merged> {
    let (base, shipped, overlay) = (open_dump(base)?, open_dump(shipped)?, open_dump(overlay)?);
    let out = Connection::open_in_memory().map_err(sql)?;
    out.execute_batch(&format!("PRAGMA foreign_keys = OFF;\nPRAGMA user_version = {SCHEMA_VERSION};\n{SCHEMA}")).map_err(sql)?;
    let mut conflicts = Vec::new();
    for shape in shapes() {
        let (b, s, o) = (keyed(&base, shape)?, keyed(&shipped, shape)?, keyed(&overlay, shape)?);
        let keys: std::collections::BTreeSet<&String> = b.keys().chain(s.keys()).chain(o.keys()).collect();
        let columns = db_columns(shape);
        let marks: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let mut insert = out.prepare(&format!("INSERT INTO {} ({}) VALUES ({})", shape.table, columns.join(", "), marks.join(", "))).map_err(sql)?;
        for key in keys {
            let (was, now, mine) = (b.get(key), s.get(key), o.get(key));
            let pick = if mine == was { now } else if now == was || now == mine { mine } else {
                conflicts.push(format!("{} {key}", shape.table));
                mine
            };
            if let Some(values) = pick { insert.execute(rusqlite::params_from_iter(values)).map_err(sql)?; }
        }
    }
    let text = export(&out)?;
    // Checked whole: a shipped quest deleted under an overlay row that names it is refused here.
    let text = canonical(&text)?;
    Ok(Merged { text, conflicts })
}
