//! The quest content database: the scripted-quest tables as SQLite, so a quest is edited as rows
//! (the content editor, or any SQLite tool) rather than code.
//!
//! The checked-in form is `content/quests.sql`, a dump this module writes deterministically
//! (`export`: the schema, then one `INSERT` per row in key order), so a change reads as a diff and
//! a conflict merges as text. `build` turns the dump into an editable `quests.sqlite`; `export`
//! writes the edited database back. The server reads the dump (`Tables::from_sql`) through the
//! same checks the rows always had (`Tables::parse_rows`); the schema's own constraints refuse
//! the plainest mistakes as they are typed.
//!
//! Every row carries a `family`: `hand` for hand-written content, or the name of the tool that
//! generates it (`vault`, `tools/import_vault_quests.py`), whose re-import replaces only its own
//! rows (`import_tsv`). Sets are tried in `priority` order (then name), the first match running.
use std::io;
use std::path::Path;

use rusqlite::Connection;
use rusqlite::types::{Value, ValueRef};

use super::{FILES, HEADERS, Row, Tables};

/// Bumped when the schema changes; the dump sets it as `PRAGMA user_version`.
pub const SCHEMA_VERSION: i32 = 5;

/// The tables, in dump order. Hex identifiers are text (`0x4700157d`), as they are written
/// everywhere else; blank optional fields are NULL.
pub const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS quest (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  journal_id TEXT NOT NULL CHECK (journal_id GLOB '0x[0-9a-fA-F]*'),
  title_table TEXT NOT NULL CHECK (title_table GLOB '0x[0-9a-fA-F]*'),
  title_key TEXT NOT NULL CHECK (title_key GLOB '0x[0-9a-fA-F]*'),
  title TEXT NOT NULL CHECK (title <> ''),
  goal INTEGER CHECK (goal >= 0),
  credit TEXT NOT NULL DEFAULT 'killer' CHECK (credit IN ('killer', 'fellowship')),
  min_level INTEGER CHECK (min_level >= 0),
  time_limit_seconds INTEGER CHECK (time_limit_seconds > 0),
  max_solves INTEGER CHECK (max_solves > 0),
  repeat_seconds INTEGER CHECK (repeat_seconds > 0),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS quest_stage (
  quest TEXT NOT NULL REFERENCES quest(name) ON UPDATE CASCADE ON DELETE CASCADE,
  stage INTEGER NOT NULL CHECK (stage BETWEEN 0 AND 255),
  completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
  journal_text TEXT NOT NULL CHECK (journal_text <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (quest, stage)
);
CREATE TABLE IF NOT EXISTS emote_set (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  priority INTEGER NOT NULL,
  owner_entity TEXT CHECK (owner_entity GLOB '0x47[0-9a-fA-F]*' OR owner_entity GLOB '[a-z]*'),
  owner_iid TEXT CHECK (owner_iid GLOB '0x[0-9a-fA-F]*'),
  trigger TEXT NOT NULL CHECK (trigger IN ('Use', 'Give', 'Kill', 'ItemUsedOn', 'Enter', 'Crafted', 'EscortLost',
    'QuestSuccess', 'QuestFailure', 'Goto', 'PlayerDeath', 'QuestExpired', 'Logout', 'EscortAbandoned')),
  item TEXT CHECK (item GLOB '0x[0-9a-fA-F]*' OR item GLOB '[a-z]*'),
  quest TEXT REFERENCES quest(name) ON UPDATE CASCADE,
  stage_min INTEGER CHECK (stage_min BETWEEN 0 AND 255),
  stage_max INTEGER CHECK (stage_max BETWEEN 0 AND 255),
  range REAL CHECK (range > 0),
  probability REAL CHECK (probability > 0 AND probability <= 1),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS emote_action (
  emote_set TEXT NOT NULL REFERENCES emote_set(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  action TEXT NOT NULL CHECK (action <> ''),
  delay_ms INTEGER NOT NULL DEFAULT 0 CHECK (delay_ms BETWEEN 0 AND 3600000),
  p1 TEXT NOT NULL DEFAULT '',
  p2 TEXT NOT NULL DEFAULT '',
  p3 TEXT NOT NULL DEFAULT '',
  p4 TEXT NOT NULL DEFAULT '',
  p5 TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (emote_set, ord)
);
CREATE TABLE IF NOT EXISTS alias (
  name TEXT PRIMARY KEY CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  kind TEXT NOT NULL CHECK (kind IN ('entity', 'effect', 'hex', 'placement')),
  value TEXT NOT NULL CHECK (value <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS quest_counter (
  quest TEXT NOT NULL REFERENCES quest(name) ON UPDATE CASCADE ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  idx INTEGER NOT NULL CHECK (idx BETWEEN 0 AND 255),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (quest, name),
  UNIQUE (quest, idx)
);
CREATE TABLE IF NOT EXISTS template (
  name TEXT PRIMARY KEY CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  doc TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS template_param (
  template TEXT NOT NULL REFERENCES template(name) ON UPDATE CASCADE ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  ord INTEGER NOT NULL DEFAULT 0,
  doc TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (template, name)
);
CREATE TABLE IF NOT EXISTS template_set (
  template TEXT NOT NULL REFERENCES template(name) ON UPDATE CASCADE ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (name <> '' AND name NOT GLOB '*.*'),
  priority INTEGER NOT NULL DEFAULT 0,
  owner_entity TEXT, owner_iid TEXT, trigger TEXT NOT NULL, item TEXT, quest TEXT,
  stage_min TEXT, stage_max TEXT, range TEXT, probability TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (template, name)
);
CREATE TABLE IF NOT EXISTS template_action (
  template TEXT NOT NULL,
  emote_set TEXT NOT NULL,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  action TEXT NOT NULL CHECK (action <> ''),
  delay_ms TEXT NOT NULL DEFAULT '0',
  p1 TEXT NOT NULL DEFAULT '', p2 TEXT NOT NULL DEFAULT '', p3 TEXT NOT NULL DEFAULT '',
  p4 TEXT NOT NULL DEFAULT '', p5 TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (template, emote_set, ord),
  FOREIGN KEY (template, emote_set) REFERENCES template_set(template, name) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS template_use (
  name TEXT PRIMARY KEY CHECK (name <> '' AND name NOT GLOB '*.*'),
  template TEXT NOT NULL REFERENCES template(name) ON UPDATE CASCADE,
  priority INTEGER NOT NULL DEFAULT 0,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS template_arg (
  use_name TEXT NOT NULL REFERENCES template_use(name) ON UPDATE CASCADE ON DELETE CASCADE,
  param TEXT NOT NULL,
  value TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (use_name, param)
);
CREATE TABLE IF NOT EXISTS instance_group (
  name TEXT PRIMARY KEY CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  doc TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS instance_lane (
  instance_group TEXT NOT NULL REFERENCES instance_group(name) ON UPDATE CASCADE ON DELETE CASCADE,
  lane INTEGER NOT NULL CHECK (lane BETWEEN 0 AND 255),
  entry TEXT NOT NULL CHECK (entry <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (instance_group, lane)
);
CREATE TABLE IF NOT EXISTS spawn_slot (
  name TEXT NOT NULL CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  instance_group TEXT NOT NULL,
  lane INTEGER NOT NULL,
  iid TEXT NOT NULL CHECK (iid GLOB '0x[0-9a-fA-F]*'),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (name, lane),
  FOREIGN KEY (instance_group, lane) REFERENCES instance_lane(instance_group, lane) ON UPDATE CASCADE ON DELETE CASCADE
);
";

/// Each table with its columns (the four TSV tables first, in TSV column order; `emote_set`'s
/// `priority` and every table's `family` are the database's own), its dump order, and how many
/// leading columns are its key.
pub(super) struct Shape { pub(super) table: &'static str, pub(super) columns: &'static [&'static str], order: &'static str, key: usize }
/// The TSV tables (`FILES` order) are SHAPES[..TSV].
const TSV: usize = 4;
pub(super) const SHAPES: [Shape; 10] = [
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
const SHAPES_TAIL: [Shape; 5] = [
    Shape { table: "template_use", order: "name", key: 1, columns: &["name", "template", "priority", "source"] },
    Shape { table: "template_arg", order: "use_name, param", key: 2, columns: &["use_name", "param", "value", "source"] },
    Shape { table: "instance_group", order: "name", key: 1, columns: &["name", "doc", "source"] },
    Shape { table: "instance_lane", order: "instance_group, lane", key: 2, columns: &["instance_group", "lane", "entry", "source"] },
    Shape { table: "spawn_slot", order: "instance_group, name, lane", key: 2, columns: &["name", "instance_group", "lane", "iid", "source"] },
];
/// Every table, in dump order.
pub(super) fn shapes() -> impl Iterator<Item = &'static Shape> { SHAPES.iter().chain(SHAPES_TAIL.iter()) }
pub(super) fn shape(table: &str) -> &'static Shape { shapes().find(|s| s.table == table).expect("a known table") }

fn sql(e: rusqlite::Error) -> io::Error { io::Error::other(e.to_string()) }

/// An empty in-memory database with the schema.
pub fn empty() -> io::Result<Connection> {
    let db = Connection::open_in_memory().map_err(sql)?;
    db.execute_batch(&format!("PRAGMA foreign_keys = ON;\n{SCHEMA}")).map_err(sql)?;
    Ok(db)
}

/// A database holding the dump `text`, in memory: `content/quests.sql`, or any SQL dump of a
/// quest database (`sqlite3 .dump`, Python's `iterdump`), whatever order its rows come in. Its
/// tables must be this schema's (a missing one is created empty); a row naming a quest or set
/// that is not there is refused once every row is in.
pub fn open_dump(text: &str) -> io::Result<Connection> {
    let db = Connection::open_in_memory().map_err(sql)?;
    // References are checked once every row is in (the bundled SQLite turns them on by default).
    db.execute_batch("PRAGMA foreign_keys = OFF;").map_err(sql)?;
    db.execute_batch(text).map_err(|e| io::Error::other(format!("quests.sql: {e}")))?;
    db.execute_batch(SCHEMA).map_err(sql)?;
    for shape in shapes() {
        let mut statement = db.prepare(&format!("PRAGMA table_info({})", shape.table)).map_err(sql)?;
        let mut have: Vec<String> = statement.query_map([], |r| r.get::<_, String>(1)).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
        let mut want: Vec<String> = shape.columns.iter().map(|c| c.to_string()).chain(["family".to_owned()]).collect();
        if shape.table == "emote_set" { want.push("priority".to_owned()); }
        have.sort();
        want.sort();
        if have != want {
            return Err(io::Error::other(format!("quests.sql: table {} has columns {}; this server reads {}", shape.table, have.join(", "), want.join(", "))));
        }
    }
    let broken: Option<(String, i64, String)> = db.query_row("PRAGMA foreign_key_check", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(Some).or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) }).map_err(sql)?;
    if let Some((table, row, parent)) = broken {
        return Err(io::Error::other(format!("quests.sql: row {row} of {table} names a {parent} that does not exist")));
    }
    db.execute_batch("PRAGMA foreign_keys = ON;").map_err(sql)?;
    check_version(&db)?;
    Ok(db)
}

/// The dump `text` as `export` writes it (checked as the server checks it), so any dump of the
/// same rows becomes the same text.
pub fn canonical(text: &str) -> io::Result<String> {
    let db = open_dump(text)?;
    Tables::from_db(&db)?;
    export(&db)
}

fn check_version(db: &Connection) -> io::Result<()> {
    let version: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(sql)?;
    // An older dump reads as this schema (export writes it anew); a newer one is refused.
    if version > SCHEMA_VERSION {
        return Err(io::Error::other(format!("the quest database is schema {version}; this server reads {SCHEMA_VERSION}")));
    }
    Ok(())
}

/// A table's rows as text (NULL as empty), in its columns' order and its dump order.
pub(super) fn table(db: &Connection, shape: &Shape, extra: &[&str]) -> io::Result<Vec<Vec<String>>> {
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

/// A table's rows, each named by its table and key (`quests emote_action guard_start/2`).
fn named(db: &Connection, shape: &Shape) -> io::Result<Vec<Row>> {
    Ok(table(db, shape, &[])?.into_iter().map(|f| (format!("quests {} {}", shape.table, f[..shape.key].join("/")), f)).collect())
}

/// What `Tables::parse_all` reads: the four tables in `FILES` order and TSV column order (the
/// template uses expanded into sets and actions, every set in priority order), and the aliases
/// and named counters.
#[derive(Default)]
pub struct QuestRows {
    pub tables: [Vec<Row>; 4], pub aliases: Vec<Row>, pub counters: Vec<Row>,
    pub groups: Vec<Row>, pub lanes: Vec<Row>, pub slots: Vec<Row>,
}

pub fn rows(db: &Connection) -> io::Result<QuestRows> {
    let expanded = super::templates::expand(db)?;
    let mut sets: Vec<(i64, Row)> = table(db, &SHAPES[2], &["priority"])?.into_iter().map(|mut f| {
        let priority = f.pop().and_then(|p| p.parse().ok()).unwrap_or(0);
        (priority, (format!("quests emote_set {}", f[0]), f))
    }).collect();
    sets.extend(expanded.sets);
    sets.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.1[0].cmp(&b.1.1[0])));
    let mut actions = named(db, &SHAPES[3])?;
    actions.extend(expanded.actions);
    Ok(QuestRows {
        tables: [named(db, &SHAPES[0])?, named(db, &SHAPES[1])?, sets.into_iter().map(|(_, row)| row).collect(), actions],
        aliases: named(db, shape("alias"))?,
        counters: named(db, shape("quest_counter"))?,
        groups: named(db, shape("instance_group"))?,
        lanes: named(db, shape("instance_lane"))?,
        slots: named(db, shape("spawn_slot"))?,
    })
}

impl Tables {
    /// The tables of a database.
    pub fn from_db(db: &Connection) -> io::Result<Self> {
        check_version(db)?;
        Self::parse_all(rows(db)?)
    }
    /// The tables of the dump `text` (`content/quests.sql`).
    pub fn from_sql(text: &str) -> io::Result<Self> { Self::from_db(&open_dump(text)?) }
    /// The tables of `directory/quests.sql`.
    pub fn load(directory: &Path) -> io::Result<Self> {
        let path = directory.join(DUMP);
        let text = std::fs::read_to_string(&path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
        Self::from_sql(&text)
    }
}

/// The dump's file name in the content directory.
pub const DUMP: &str = "quests.sql";
/// The editable database `build` makes beside the dump (not checked in).
pub const DATABASE: &str = "quests.sqlite";

/// Adds TSV tables (each `(file, text)` list in `FILES` order, the old table files) to `db` as
/// family `family`, first removing that family's rows; new sets go after every existing one, in
/// file order. The rows are checked as the server checks them.
pub fn import_tsv(db: &mut Connection, files: [Vec<(String, String)>; 4], family: &str) -> io::Result<()> {
    let tables = [
        super::load::all_rows(&files[0], HEADERS[0], 12)?, super::load::all_rows(&files[1], HEADERS[1], 5)?,
        super::load::all_rows(&files[2], HEADERS[2], 11)?, super::load::all_rows(&files[3], HEADERS[3], 10)?,
    ];
    let tx = db.transaction().map_err(sql)?;
    for shape in SHAPES[..TSV].iter().rev() {
        tx.execute(&format!("DELETE FROM {} WHERE family = ?1", shape.table), [family]).map_err(sql)?;
    }
    let mut priority: i64 = tx.query_row("SELECT COALESCE(MAX(priority), 0) FROM emote_set", [], |r| r.get(0)).map_err(sql)?;
    for (shape, list) in SHAPES[..TSV].iter().zip(&tables) {
        let set = shape.table == "emote_set";
        let mut columns = shape.columns.to_vec();
        columns.push("family");
        if set { columns.push("priority"); }
        let marks: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let query = format!("INSERT INTO {} ({}) VALUES ({})", shape.table, columns.join(", "), marks.join(", "));
        let mut insert = tx.prepare(&query).map_err(sql)?;
        for (at, fields) in list {
            let mut values: Vec<Value> = fields.iter().zip(shape.columns).map(|(v, c)| typed(shape.table, c, v)).collect();
            values.push(Value::Text(family.to_owned()));
            if set { priority += 10; values.push(Value::Integer(priority)); }
            insert.execute(rusqlite::params_from_iter(values)).map_err(|e| io::Error::other(format!("{at}: {e}")))?;
        }
    }
    Tables::from_db(&tx)?;
    tx.commit().map_err(sql)
}

/// A TSV field as the column's SQL value: blank optional fields are NULL, numbers are numbers.
fn typed(table: &str, column: &str, v: &str) -> Value {
    let text_column = matches!(column, "p1" | "p2" | "p3" | "p4" | "p5" | "action" | "source" | "journal_text" | "title" | "credit"
        | "trigger" | "name" | "quest" | "emote_set") || v.starts_with("0x") || v.starts_with("0X");
    if v.is_empty() {
        return if matches!(column, "p1" | "p2" | "p3" | "p4" | "p5") || (table == "emote_action" && column == "delay_ms") {
            if column == "delay_ms" { Value::Integer(0) } else { Value::Text(String::new()) }
        } else { Value::Null };
    }
    if !text_column {
        if let Ok(n) = v.parse::<i64>() { return Value::Integer(n); }
        if matches!(column, "range" | "probability") { if let Ok(x) = v.parse::<f64>() { return Value::Real(x); } }
    }
    Value::Text(v.to_owned())
}

/// The database as its dump: the schema version and schema, then every row as one `INSERT`,
/// in key order. The same rows always give the same text.
pub fn export(db: &Connection) -> io::Result<String> {
    let mut out = format!("-- Pyrealium quest content (schema {SCHEMA_VERSION}). Written by `pyrealium-content export`;\n\
        -- edit quests.sqlite (`pyrealium-content build`) or these rows, then export again.\n\
        PRAGMA user_version = {SCHEMA_VERSION};\n{SCHEMA}");
    for shape in shapes() {
        let mut columns: Vec<&str> = shape.columns.to_vec();
        if shape.table == "emote_set" { columns.insert(1, "priority"); }
        columns.push("family");
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
        ValueRef::Text(t) => format!("'{}'", String::from_utf8_lossy(t).replace('\'', "''")),
        ValueRef::Blob(b) => format!("X'{}'", b.iter().map(|c| format!("{c:02x}")).collect::<String>()),
    }
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
    Ok(db)
}

/// The old TSV files of `directory` (each base table and its families), for `import_tsv`.
pub fn tsv_files(directory: &Path, family: Option<&str>) -> io::Result<[Vec<(String, String)>; 4]> {
    let mut files: [Vec<(String, String)>; 4] = Default::default();
    for (table, list) in FILES.iter().zip(files.iter_mut()) {
        let paths: Vec<std::path::PathBuf> = match family {
            None => std::iter::once(directory.join(table)).chain(super::families(directory, table)?).collect(),
            Some(f) => vec![directory.join(table.replace(".tsv", &format!("-{f}.tsv")))],
        };
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
            list.push((path.display().to_string(), text));
        }
    }
    Ok(files)
}

/// What a three-way merge (`merge`) kept: the merged dump and the rows both sides changed
/// (the overlay's version won), each named by table and key.
pub struct Merged { pub text: String, pub conflicts: Vec<String> }

/// Every row of a table by its key, with every column (`family` and `priority` included).
fn keyed(db: &Connection, shape: &Shape) -> io::Result<std::collections::BTreeMap<String, Vec<Value>>> {
    let mut columns: Vec<&str> = shape.columns.to_vec();
    if shape.table == "emote_set" { columns.push("priority"); }
    columns.push("family");
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
        let mut columns: Vec<&str> = shape.columns.to_vec();
        if shape.table == "emote_set" { columns.push("priority"); }
        columns.push("family");
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
