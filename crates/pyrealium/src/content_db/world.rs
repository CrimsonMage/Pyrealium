//! The world as content-database rows: the packs the prepare step assembles the world from
//! (`world_pack`, `world_object`), and the evidence it checks them with (`world_evidence`: the
//! packs' ground-verification reports, the audited capture exclusions, the placement
//! replacements).
//!
//! A row keeps its fields exactly as the pack file wrote them (hex identifiers with `0x`, numbers
//! as their decimal text, `-` as written), so a pack's canonical text (`pack_text`) reproduces each
//! row byte for byte. The evidence's per-row digests (`row_sha256`) stay valid. A pack's
//! whole-file digest (`manifest_sha256`) is re-made over its canonical text at import, after the
//! original file's digest was checked; the original is kept as `original_manifest_sha256`.
use std::io;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Every pack, in the prepare step's source order, with its kind.
pub const PACKS: [(&str, &str); 17] = [
    ("world-observed-expanded", "observed"), ("training-world", "authored"), ("human-shelter-world", "authored"),
    ("recovered-population", "recovered"), ("capture-spawns", "capture"), ("capture-spawns-upstream", "capture"),
    ("capture-spawns-audited", "capture"), ("inferred-world-v1", "inferred"), ("regional-world-v1", "inferred"),
    ("wilderness-world-v1", "inferred"), ("exploration-world-v1", "inferred"), ("hunting-world-v1", "inferred"),
    ("hunting-world-v2", "inferred"), ("workshops-world-v1", "inferred"), ("arwic-mines-world-v1", "authored"),
    ("npc-world-v1", "inferred"), ("gauntlet-world-v1", "authored"),
];
/// Evidence files the prepare step reads beside the packs' own reports.
pub const EVIDENCE: [&str; 2] = ["placement-replacements.json", "capture-spawns-audited.report.json"];
/// The packs made from a source catalog (a `content_document`), which the prepare step checks a
/// pack's report against (`source_sha256`).
pub const CATALOGS: [(&str, &str); 10] = [
    ("inferred-world-v1", "inferred-content-v1.json"), ("regional-world-v1", "regional-content-v1.json"),
    ("wilderness-world-v1", "wilderness-content-v1.json"), ("exploration-world-v1", "exploration-content-v1.json"),
    ("hunting-world-v1", "hunting-content-v1.json"), ("hunting-world-v2", "hunting-content-v2.json"),
    ("workshops-world-v1", "workshops-content-v1.json"), ("arwic-mines-world-v1", "arwic-mines-world-v1.json"),
    ("npc-world-v1", "npc-dialogue-v1.json"), ("gauntlet-world-v1", "gauntlet-content-v1.json"),
];
/// A catalog document's text as the database keeps it (compact JSON), which its digest names.
pub fn document_text(value: &Value) -> io::Result<String> { serde_json::to_string(value).map_err(io::Error::other) }
/// The header of a pack of full-width rows (the prepare step reads rows, not the header).
const HEADER: &str = "pyrealium-world-v4";

fn sql(e: rusqlite::Error) -> io::Error { io::Error::other(e.to_string()) }
fn bad(message: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message.into()) }
pub fn sha256_hex(bytes: &[u8]) -> String { Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect() }

/// A file's digests with LF or CRLF line ends (git `* text=auto`).
fn file_digests(path: &Path) -> io::Result<[String; 2]> {
    let raw = std::fs::read(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    let lf = String::from_utf8_lossy(&raw).replace("\r\n", "\n");
    Ok([sha256_hex(lf.as_bytes()), sha256_hex(lf.replace('\n', "\r\n").as_bytes())])
}

/// The pack's canonical text: the header of its rows' width (`pyrealium-world-v2` for 11 fields,
/// `v3` for 12, `v4` for 13, so the text reads as a world table as it is), then each row's fields
/// as the file wrote them, in row order; `None` when the database has no such pack.
pub fn pack_text(db: &Connection, pack: &str) -> io::Result<Option<String>> {
    let known: Option<String> = db.query_row("SELECT name FROM world_pack WHERE name = ?1", [pack], |r| r.get(0)).optional().map_err(sql)?;
    if known.is_none() { return Ok(None); }
    let mut statement = db.prepare("SELECT iid, entity, visual, cell, x, y, z, qw, qx, qy, qz, scale, name FROM world_object \
        WHERE pack = ?1 ORDER BY ord").map_err(sql)?;
    let mut rows = statement.query([pack]).map_err(sql)?;
    let (mut body, mut width) = (String::new(), 13);
    let mut first = true;
    while let Some(row) = rows.next().map_err(sql)? {
        let mut fields: Vec<String> = Vec::with_capacity(13);
        for i in 0..4 {
            let v: String = row.get(i).map_err(sql)?;
            fields.push(v.strip_prefix("0x").unwrap_or(&v).to_owned());
        }
        for i in 4..11 { fields.push(row.get(i).map_err(sql)?); }
        for i in 11..13 {
            if let Some(v) = row.get::<_, Option<String>>(i).map_err(sql)? { fields.push(v); }
        }
        width = if first { fields.len() } else { width.max(fields.len()) };
        first = false;
        body.push_str(&fields.join("\t"));
        body.push('\n');
    }
    let header = match width { 11 => "pyrealium-world-v2", 12 => "pyrealium-world-v3", _ => HEADER };
    Ok(Some(format!("{header}\n{body}")))
}

/// An evidence file's JSON from the database, `None` when it has none.
pub fn evidence(db: &Connection, name: &str) -> io::Result<Option<Value>> {
    let body: Option<String> = db.query_row("SELECT body FROM world_evidence WHERE name = ?1", [name], |r| r.get(0)).optional().map_err(sql)?;
    body.map(|b| serde_json::from_str(&b).map_err(|e| bad(format!("world_evidence {name}: {e}")))).transpose()
}

/// Whether the database holds the world (any pack).
pub fn has_world(db: &Connection) -> io::Result<bool> {
    db.query_row("SELECT COUNT(*) FROM world_pack", [], |r| r.get::<_, i64>(0)).map(|n| n > 0).map_err(sql)
}

/// What `import_packs` took in.
pub struct Imported { pub packs: usize, pub objects: usize, pub evidence: usize }

/// Moves the world packs of the content directory `content` (their `*.tsv`, their
/// `*.report.json`, and the evidence files) into the database, replacing any it holds. A pack
/// whose report digest does not match its file is refused: stale evidence never comes in.
pub fn import_packs(db: &mut Connection, content: &Path) -> io::Result<Imported> {
    let tx = db.transaction().map_err(sql)?;
    let mut imported = Imported { packs: 0, objects: 0, evidence: 0 };
    for (ord, (pack, kind)) in PACKS.iter().enumerate() {
        let path = content.join(format!("{pack}.tsv"));
        if !path.exists() { continue; }
        let text = std::fs::read_to_string(&path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        imported.objects += put_pack(&tx, pack, ord as i64, kind, &text,
            &format!("content/{pack}.tsv, imported by pyrealium-content import-world-packs"))?;
        imported.packs += 1;
        // Its report: the original file's digest checked, the digest re-made over the rows.
        let report = content.join(format!("{pack}.report.json"));
        if report.exists() {
            let name = format!("{pack}.report.json");
            let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&report)?).map_err(|e| bad(format!("{name}: {e}")))?;
            if let Some(manifest) = value.get("manifest_sha256").and_then(Value::as_str).map(str::to_owned) {
                if !file_digests(&path)?.contains(&manifest) {
                    return Err(bad(format!("{name}: its manifest_sha256 does not match {pack}.tsv (stale evidence)")));
                }
                let canonical = pack_text(&tx, pack)?.unwrap_or_default();
                // A report checked out of the database already names the first original.
                if value.get("original_manifest_sha256").is_none() { value["original_manifest_sha256"] = Value::String(manifest); }
                value["manifest_sha256"] = Value::String(sha256_hex(canonical.as_bytes()));
            }
            // Its source catalog's digest likewise, over the document the database keeps.
            let catalog = CATALOGS.iter().find(|(p, _)| p == pack).map(|(_, c)| content.join(c)).filter(|c| c.exists());
            if let (Some(source), Some(catalog)) = (value.get("source_sha256").and_then(Value::as_str).map(str::to_owned), catalog) {
                if !file_digests(&catalog)?.contains(&source) {
                    return Err(bad(format!("{name}: its source_sha256 does not match {} (stale evidence)", catalog.display())));
                }
                let document: Value = serde_json::from_str(&std::fs::read_to_string(&catalog)?).map_err(|e| bad(format!("{}: {e}", catalog.display())))?;
                if value.get("original_source_sha256").is_none() { value["original_source_sha256"] = Value::String(source); }
                value["source_sha256"] = Value::String(sha256_hex(document_text(&document)?.as_bytes()));
            }
            put_evidence(&tx, &name, &value)?;
            imported.evidence += 1;
        }
    }
    for name in EVIDENCE {
        let path = content.join(name);
        if !path.exists() { continue; }
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(|e| bad(format!("{name}: {e}")))?;
        put_evidence(&tx, name, &value)?;
        imported.evidence += 1;
    }
    tx.commit().map_err(sql)?;
    Ok(imported)
}

/// Replaces the pack `pack` with the rows of the pack file `text` (its first line, the header,
/// is not a row); returns how many it holds.
pub fn put_pack(db: &Connection, pack: &str, ord: i64, kind: &str, text: &str, source: &str) -> io::Result<usize> {
    db.execute("DELETE FROM world_object WHERE pack = ?1", [pack]).map_err(sql)?;
    db.execute("DELETE FROM world_pack WHERE name = ?1", [pack]).map_err(sql)?;
    db.execute("INSERT INTO world_pack (name, ord, kind, source) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![pack, ord, kind, source]).map_err(sql)?;
    let mut insert = db.prepare("INSERT INTO world_object (pack, iid, ord, entity, visual, cell, x, y, z, qw, qx, qy, qz, scale, name) \
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)").map_err(sql)?;
    let mut count = 0;
    for (n, line) in text.lines().skip(1).filter(|l| !l.is_empty() && !l.starts_with('#')).enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        if !matches!(f.len(), 11..=13) {
            return Err(bad(format!("{pack}.tsv row {}: {} fields (a pack row has 11 to 13)", n + 1, f.len())));
        }
        let hex = |v: &str| format!("0x{v}");
        insert.execute(rusqlite::params![pack, hex(f[0]), n as i64, hex(f[1]), hex(f[2]), hex(f[3]), f[4], f[5], f[6], f[7], f[8],
            f[9], f[10], f.get(11).copied(), f.get(12).copied()])
            .map_err(|e| bad(format!("{pack}.tsv row {} ({}): {e}", n + 1, f[0])))?;
        count += 1;
    }
    Ok(count)
}

fn put_evidence(db: &Connection, name: &str, value: &Value) -> io::Result<()> {
    let body = serde_json::to_string(value).map_err(io::Error::other)?;
    db.execute("INSERT OR REPLACE INTO world_evidence (name, body, source) VALUES (?1, ?2, ?3)",
        rusqlite::params![name, body, format!("content/{name}, imported by pyrealium-content import-world-packs")]).map_err(sql)?;
    Ok(())
}
