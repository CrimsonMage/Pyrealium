//! The quest rules' side of the content database (`crate::content_db`): reading the quest tables
//! from it (`rows`, `Tables::from_db`) and the old TSV tables' import (`import_tsv`). Everything of
//! the database itself (schema, dump, build, merge) is `content_db`'s, re-exported here.
use std::io;
use std::path::Path;

use rusqlite::Connection;
use rusqlite::types::Value;

pub use crate::content_db::*;
use super::{FILES, HEADERS, Row, Tables};

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
    pub groups: Vec<Row>, pub lanes: Vec<Row>, pub slots: Vec<Row>, pub items: Vec<Row>, pub recipes: Vec<Row>,
}

pub fn rows(db: &Connection) -> io::Result<QuestRows> {
    let expanded = super::templates::expand(db)?;
    // A set's owner_slots and phrase ride after its TSV columns (`Tables::parse_all` reads them there).
    let mut sets: Vec<(i64, Row)> = table(db, &SHAPES[2], &["owner_slots", "phrase", "priority"])?.into_iter().map(|mut f| {
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
        items: named(db, shape("quest_item"))?,
        recipes: named(db, shape("quest_recipe"))?,
    })
}

impl Tables {
    /// The tables of a database.
    pub fn from_db(db: &Connection) -> io::Result<Self> {
        check_version(db)?;
        Self::parse_all(rows(db)?)
    }
    /// The tables of the dump `text` (`content/content.sql`).
    pub fn from_sql(text: &str) -> io::Result<Self> { Self::from_db(&open_dump(text)?) }
    /// The tables of the directory's content dump (`content.sql`, else an older `quests.sql`).
    pub fn load(directory: &Path) -> io::Result<Self> {
        let path = dump_in(directory);
        let text = std::fs::read_to_string(&path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
        Self::from_sql(&text)
    }
}

/// Adds TSV tables (each `(file, text)` list in `FILES` order, the old table files) to `db` as
/// family `family`, first removing that family's rows; new sets go after every existing one, in
/// file order. The rows are checked as the server checks them.
pub fn import_tsv(db: &mut Connection, files: [Vec<(String, String)>; 4], family: &str) -> io::Result<()> {
    let tables = [
        super::load::all_rows(&files[0], HEADERS[0], 12)?, super::load::all_rows(&files[1], HEADERS[1], 5)?,
        super::load::all_rows(&files[2], HEADERS[2], 11)?, super::load::all_rows(&files[3], HEADERS[3], 10)?,
    ];
    let tx = db.transaction().map_err(sql)?;
    // A family imported again keeps its place among the others (its sets numbered in file order
    // from where its block began), so a re-import never moves it behind rows added since; a new
    // family goes after every set.
    let was: Option<i64> = tx.query_row("SELECT MIN(priority) FROM emote_set WHERE family = ?1", [family], |r| r.get(0)).map_err(sql)?;
    for shape in SHAPES[..TSV].iter().rev() {
        tx.execute(&format!("DELETE FROM {} WHERE family = ?1", shape.table), [family]).map_err(sql)?;
    }
    let mut priority: i64 = match was {
        Some(first) => first - 10,
        None => tx.query_row("SELECT COALESCE(MAX(priority), 0) FROM emote_set", [], |r| r.get(0)).map_err(sql)?,
    };
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
