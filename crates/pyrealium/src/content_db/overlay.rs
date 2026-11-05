//! A world builder's world as content-database rows: its own placements (the `builder` pack's
//! `world_object` rows: new objects, and base objects moved or changed, by placement id), the base
//! placements it removes (`world_tombstone`), and its creature rules, patrols and spawn groups
//! (`creature_stats`, `creature_ai`, `creature_loot`, `patrol`, `spawn_group`,
//! `spawn_group_member`).
//!
//! The server's world and rules readers take text (`Content::parse_text`, `world_rules::Rules`), so
//! these rows are read out as that text (`builder_text`, `rules_text`). `import` takes a world
//! builder's older files (`NAME.tsv`, `NAME.rules.tsv`) in as rows.
use std::collections::BTreeSet;
use std::io;

use rusqlite::Connection;
use rusqlite::types::ValueRef;

/// The pack a world builder's own placements are in.
pub const BUILDER: &str = "builder";
/// Its place in pack order: after every shipped pack (a world builder's database holds none of
/// those, so a place computed there would collide with one when merged over the shipped dump).
pub const BUILDER_ORD: i64 = 1000;

fn sql(e: rusqlite::Error) -> io::Error { io::Error::other(e.to_string()) }
fn bad(message: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message.into()) }

/// A column's value as rule text: `-` for NULL (keep the inherited value).
fn field(v: ValueRef) -> String {
    match v {
        ValueRef::Null => "-".to_owned(),
        ValueRef::Integer(n) => n.to_string(),
        ValueRef::Real(x) => format!("{x}"),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(_) => String::new(),
    }
}
/// A hex identifier without its `0x` (the text formats' form).
fn bare(v: &str) -> &str { v.strip_prefix("0x").unwrap_or(v) }

/// The world builder's placements as world text (`pyrealium-world-v4`).
pub fn builder_text(db: &Connection) -> io::Result<String> {
    let mut text = String::from("pyrealium-world-v4\n");
    let mut statement = db.prepare("SELECT o.iid, o.entity, o.visual, o.cell, o.x, o.y, o.z, o.qw, o.qx, o.qy, o.qz, o.scale, o.name \
        FROM world_object o JOIN world_pack p ON p.name = o.pack WHERE p.kind = 'builder' ORDER BY o.pack, o.ord").map_err(sql)?;
    let mut rows = statement.query([]).map_err(sql)?;
    while let Some(row) = rows.next().map_err(sql)? {
        let mut f: Vec<String> = (0..4).map(|i| row.get::<_, String>(i).map(|v| bare(&v).to_owned())).collect::<Result<_, _>>().map_err(sql)?;
        for i in 4..11 { f.push(row.get(i).map_err(sql)?); }
        f.push(row.get::<_, Option<String>>(11).map_err(sql)?.unwrap_or_else(|| "-".into()));
        f.push(row.get::<_, Option<String>>(12).map_err(sql)?.unwrap_or_else(|| "-".into()));
        text.push_str(&f.join("\t"));
        text.push('\n');
    }
    Ok(text)
}

/// The base placements the world builder's world removes.
pub fn tombstones(db: &Connection) -> io::Result<BTreeSet<u64>> {
    let mut statement = db.prepare("SELECT iid FROM world_tombstone").map_err(sql)?;
    let ids = statement.query_map([], |r| r.get::<_, String>(0)).map_err(sql)?.collect::<Result<Vec<_>, _>>().map_err(sql)?;
    ids.iter().map(|id| u64::from_str_radix(bare(id), 16).map_err(|_| bad(format!("world_tombstone {id}")))).collect()
}

/// The world builder's rules as rules text (`pyrealium-world-rules-v1`).
pub fn rules_text(db: &Connection) -> io::Result<String> {
    let mut text = format!("{}\n", crate::world_rules::HEADER);
    let mut rows = |query: &str, kind: &str, columns: usize| -> io::Result<()> {
        let mut statement = db.prepare(query).map_err(sql)?;
        let mut found = statement.query([]).map_err(sql)?;
        while let Some(row) = found.next().map_err(sql)? {
            let mut f = vec![kind.to_owned()];
            for i in 0..columns {
                let v = field(row.get_ref(i).map_err(sql)?);
                f.push(if i == 0 { bare(&v).to_owned() } else { v });
            }
            text.push_str(&f.join("\t"));
            text.push('\n');
        }
        Ok(())
    };
    rows("SELECT target, health, damage, armor, level, xp, gold, interval_ms, range, leash, respawn_ms FROM creature_stats ORDER BY target", "stats", 11)?;
    rows("SELECT target, perception, homesick, wandering, wander_range, wander_probability, aggressive, idle_only, loot_proof \
        FROM creature_ai ORDER BY target", "ai", 9)?;
    rows("SELECT target, weight, items FROM creature_loot ORDER BY target, ord", "loot", 3)?;
    rows("SELECT actor, pause_ms, points FROM patrol ORDER BY actor", "patrol", 3)?;
    let mut statement = db.prepare("SELECT g.id, g.capacity, g.regen_ms, group_concat(substr(m.placement, 3) || coalesce(':' || m.chance, ''), ',') FROM spawn_group g \
        JOIN spawn_group_member m ON m.spawn_group = g.id GROUP BY g.id ORDER BY g.id").map_err(sql)?;
    let mut found = statement.query([]).map_err(sql)?;
    while let Some(row) = found.next().map_err(sql)? {
        let id: String = row.get(0).map_err(sql)?;
        text.push_str(&format!("group\t{}\t{}\t{}\t{}\n", bare(&id), field(row.get_ref(1).map_err(sql)?),
            field(row.get_ref(2).map_err(sql)?), field(row.get_ref(3).map_err(sql)?)));
    }
    Ok(text)
}

/// Replaces the world builder's placements and rules with those of its older files: the world
/// text of `NAME.tsv` (any `pyrealium-world-vN`) and the rules text of `NAME.rules.tsv`. Both are
/// checked by the readers the server uses first.
pub fn import(db: &mut Connection, world: &str, rules: &str, game_data: Option<&std::path::Path>) -> io::Result<(usize, usize)> {
    if let Some(game_data) = game_data { crate::content::Content::parse_text(world, game_data)?; }
    if !rules.trim().is_empty() { crate::world_rules::Rules::parse(rules)?; }
    let tx = db.transaction().map_err(sql)?;
    tx.execute_batch("DELETE FROM world_object WHERE pack = 'builder'; DELETE FROM spawn_group_member; DELETE FROM spawn_group;
        DELETE FROM patrol; DELETE FROM creature_loot; DELETE FROM creature_ai; DELETE FROM creature_stats;").map_err(sql)?;
    tx.execute("INSERT OR IGNORE INTO world_pack (name, ord, kind, source) VALUES ('builder', ?1, 'builder', \
        'the world builder''s placements')", [BUILDER_ORD]).map_err(sql)?;
    let mut lines = world.lines();
    let version = lines.next().and_then(|h| h.strip_prefix("pyrealium-world-v")).and_then(|v| v.parse::<u8>().ok())
        .ok_or_else(|| bad("the world text has no pyrealium-world-vN header"))?;
    let mut placed = 0;
    for (n, line) in lines.filter(|l| !l.is_empty() && !l.starts_with('#')).enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        // v1 has a heading instead of the quaternion: the server's own conversion of it.
        let q: Vec<String> = if version == 1 {
            let heading = f.get(7).and_then(|h| h.parse::<u8>().ok()).ok_or_else(|| bad(format!("world row {}: heading", n + 1)))?;
            let placement = crate::movement::Placement { cell: 0, coordinates: [0.0; 3], heading };
            placement.orientation().iter().map(|v| format!("{v}")).collect()
        } else {
            f.get(7..11).ok_or_else(|| bad(format!("world row {}: too few fields", n + 1)))?.iter().map(|v| v.to_string()).collect()
        };
        let (scale, name) = match version { 1 | 2 => (None, None), 3 => (f.get(11).copied(), None), _ => (f.get(11).copied(), f.get(12).copied()) };
        let hex = |v: &str| format!("0x{v}");
        tx.execute("INSERT INTO world_object (pack, iid, ord, entity, visual, cell, x, y, z, qw, qx, qy, qz, scale, name, source) \
            VALUES ('builder', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'the world builder')",
            rusqlite::params![hex(f[0]), n as i64, hex(f[1]), hex(f[2]), hex(f[3]), f[4], f[5], f[6], q[0], q[1], q[2], q[3],
                scale.filter(|s| *s != "-"), name.filter(|s| *s != "-")])
            .map_err(|e| bad(format!("world row {} ({}): {e}", n + 1, f[0])))?;
        placed += 1;
    }
    let mut ruled = 0;
    let mut loot_ord = std::collections::BTreeMap::<String, i64>::new();
    for line in rules.lines().skip(1).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let v = |i: usize| -> Option<&str> { f.get(i).copied().filter(|s| *s != "-" && !s.is_empty()) };
        let hex = |s: &str| format!("0x{s}");
        let src = "the world builder";
        match f[0] {
            "stats" => tx.execute("INSERT INTO creature_stats (target, health, damage, armor, level, xp, gold, interval_ms, range, leash, respawn_ms, source) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![hex(f[1]), v(2), v(3), v(4), v(5), v(6), v(7), v(8), v(9), v(10), v(11), src]),
            "ai" => tx.execute("INSERT INTO creature_ai (target, perception, homesick, wandering, wander_range, wander_probability, aggressive, idle_only, loot_proof, source) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![hex(f[1]), v(2), v(3), v(4), v(5), v(6), v(7), v(8), v(9), src]),
            "loot" => {
                let ord = loot_ord.entry(f[1].to_owned()).or_insert(0);
                *ord += 1;
                tx.execute("INSERT INTO creature_loot (target, ord, weight, items, source) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![hex(f[1]), *ord, f[2], f[3], src])
            }
            "patrol" => tx.execute("INSERT INTO patrol (actor, pause_ms, points, source) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![hex(f[1]), f[2], f[3], src]),
            "group" => {
                tx.execute("INSERT INTO spawn_group (id, capacity, regen_ms, source) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![hex(f[1]), f[2], v(3), src]).map_err(sql)?;
                // A member is `placement` or `placement:chance` (`world_rules::Group::chances`).
                for member in f[4].split(',') {
                    let (member, chance) = member.split_once(':').map_or((member, None), |(m, c)| (m, Some(c)));
                    tx.execute("INSERT INTO spawn_group_member (spawn_group, placement, chance, source) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![hex(f[1]), hex(member), chance, src]).map_err(sql)?;
                }
                Ok(1)
            }
            other => return Err(bad(format!("unrecognized rule row `{other}`"))),
        }.map_err(|e| bad(format!("rule `{line}`: {e}")))?;
        ruled += 1;
    }
    tx.commit().map_err(sql)?;
    Ok((placed, ruled))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "pyrealium-world-rules-v1\n\
        stats\t47001234\t500\t40\t-\t12\t-\t-\t1500\t2.5\t30\t60000\n\
        ai\t8140000000030001\t20\t-\t1\t8\t0.25\t0\t-\t1\n\
        loot\t47001234\t3\t47000f2d,47000f2e\nloot\t47001234\t1\t-\n\
        patrol\t8140000000030001\t2000\t1,0;0,1;-1,0\n\
        group\t1a\t2\t30000\t8140000000030001,8140000000030002:0.25\n";

    #[test]
    fn a_world_builders_files_come_in_as_rows_and_back_out_as_the_same_world_and_rules() {
        let mut db = crate::content_db::empty().unwrap();
        let world = "pyrealium-world-v3\n\
            8140000000030001\t47001454\t1f000d09\t74ba0007\t8\t12\t2\t1\t0\t0\t0\t-\n\
            8140000000030002\t47001454\t1f000d09\t02990109\t50\t220.5\t-44\t0.7071\t0\t0\t0.7071\t1.5\n";
        assert_eq!(import(&mut db, world, RULES, None).unwrap(), (2, 6));
        assert_eq!(builder_text(&db).unwrap(), "pyrealium-world-v4\n\
            8140000000030001\t47001454\t1f000d09\t74ba0007\t8\t12\t2\t1\t0\t0\t0\t-\t-\n\
            8140000000030002\t47001454\t1f000d09\t02990109\t50\t220.5\t-44\t0.7071\t0\t0\t0.7071\t1.5\t-\n");
        let back = crate::world_rules::Rules::parse(&rules_text(&db).unwrap()).unwrap();
        assert!(back == crate::world_rules::Rules::parse(RULES).unwrap(), "{}", rules_text(&db).unwrap());
        // A v1 row's heading becomes the server's own quaternion for it.
        import(&mut db, "pyrealium-world-v1\n8140000000030003\t47001454\t1f000d09\t74ba0007\t8\t12\t2\t64\n", "", None).unwrap();
        let q = crate::movement::Placement { cell: 0, coordinates: [0.0; 3], heading: 64 }.orientation();
        assert!(builder_text(&db).unwrap().contains(&format!("\t{}\t0\t0\t{}\t-\t-", q[0], q[3])));
        // Tombstones, and the rows surviving the world-less dump the editor is handed.
        db.execute("INSERT INTO world_tombstone (iid, source) VALUES ('0x8140000000000042', 'test')", []).unwrap();
        assert_eq!(tombstones(&db).unwrap(), BTreeSet::from([0x8140_0000_0000_0042]));
        let kept = crate::content_db::open_dump(&crate::content_db::without_world(&crate::content_db::export(&db).unwrap()).unwrap()).unwrap();
        assert_eq!(builder_text(&kept).unwrap(), builder_text(&db).unwrap());
        assert_eq!(tombstones(&kept).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    /// A world builder's placements merge over the shipped content (which holds every pack):
    /// its pack's place never collides with a shipped pack's.
    #[test]
    fn a_world_builders_placements_merge_over_the_shipped_content() {
        let shipped = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/content.sql")).unwrap();
        let base = crate::content_db::without_world(&shipped).unwrap();
        let mut db = crate::content_db::open_dump(&base).unwrap();
        import(&mut db, "pyrealium-world-v4\n8140000000abcdef\t4700157e\t1f000d75\t02990108\t-13.5\t239.3\t-43.9\t1\t0\t0\t0\t-\t-\n", "", None).unwrap();
        db.execute("INSERT INTO world_tombstone (iid, source) VALUES ('0x8140000000030041', 'test')", []).unwrap();
        let overlay = crate::content_db::export(&db).unwrap();
        let merged = crate::content_db::merge(&base, &shipped, &overlay).unwrap();
        let merged = crate::content_db::open_dump(&merged.text).unwrap();
        assert!(builder_text(&merged).unwrap().contains("8140000000abcdef"));
        assert_eq!(tombstones(&merged).unwrap().len(), 1);
        let packs: i64 = merged.query_row("SELECT COUNT(*) FROM world_pack", [], |r| r.get(0)).unwrap();
        assert_eq!(packs, crate::content_db::world::PACKS.len() as i64 + 2, "every shipped pack, the bundled world's and the builder's");
        // The shipped content files come through the merge too.
        let files: i64 = merged.query_row("SELECT COUNT(*) FROM content_file", [], |r| r.get(0)).unwrap();
        assert_eq!(files, crate::content_db::files::FILES.len() as i64);
    }
}
