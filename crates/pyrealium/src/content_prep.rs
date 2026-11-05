//! Content preparation: builds the server's world placement table from the checked-in content
//! packs (`content/`), keeping only placements the installed DATs can serve, and the
//! quest/emote tables that go with it. A port of `Network/prepare_content.py` with
//! `content_profile.py` and the readers it used (`Tools/AC2/dat_probe.py`,
//! `population_probe.py`); its output is byte-identical to theirs.
//!
//! `run` stores the result in `content_files` / `content_meta` and re-prepares only when an input changes
//! (`input_key`).
use crate::dat::Dat;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

/// Bumped whenever the preparation's output for the same inputs changes.
pub const VERSION: i32 = 2;

/// The scripted-quest content served beside the world table: the quest database's dump
/// (`quest_emotes::db`).
pub const TABLES: [&str; 1] = [crate::quest_emotes::db::DUMP];
/// The table files the quest database replaced, removed from an output directory.
const RETIRED_TABLES: [&str; 4] = ["quest-registry.tsv", "quest-stages.tsv", "emote-sets.tsv", "emote-actions.tsv"];

const QUICK_EXIT_ENTITY: &str = "47001451"; // wsl_genericportal_pink, the Shelter's quick-exit portal
const QUICK_EXIT_NAME: &str = "Quick Exit to the Arwic Outlands";

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Keep `training-world.tsv`'s placements (else mirror the captured 0x0298 Shelter into 0x0299).
    pub training_placements: bool,
    /// Without training placements, add Rescue the Shelter Hold's placements.
    pub shelter_quest: bool,
}

pub struct Prepared {
    /// The world placement table (`pyrealium-world-v3/v4`).
    pub world: String,
    /// Quest and emote tables (file name, text), served beside the world table.
    pub tables: Vec<(String, String)>,
    pub report: Value,
    /// The summary lines the preparation prints.
    pub summary: Vec<String>,
}

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// ------------------------------------------------------------------------------- DAT readers

/// `population_probe.Reader`: a bounds-checked little-endian cursor. Any overrun is an error.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
#[derive(Debug)]
struct Malformed;
type R<T> = Result<T, Malformed>;
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if n > self.data.len() - self.pos {
            return Err(Malformed);
        }
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }
    fn word(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn half(&mut self) -> R<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn align(&mut self) -> R<()> {
        self.take((4 - self.pos % 4) % 4).map(|_| ())
    }
    /// A PStringBase; only its extent matters here.
    fn string(&mut self, width: usize) -> R<()> {
        let mut n = usize::from(self.half()?);
        if n == 65535 {
            n = self.word()? as usize;
        }
        self.take(n.checked_mul(width).ok_or(Malformed)?)?;
        self.align()
    }
}

/// The one property value the visual resolution reads.
#[derive(Default)]
struct EntityDesc {
    data: u32,
    phys_obj: Option<u32>,
}

/// `population_probe.properties`; keeps the last word of group 0x40000017 key 0x4100020B.
fn properties(r: &mut Reader, types: &HashMap<u32, u32>, out: &mut EntityDesc) -> R<()> {
    for _ in 0..r.word()? {
        let group = r.word()?;
        for _ in 0..r.word()? {
            let key = r.word()?;
            let t = types.get(&key).ok_or(Malformed)? & 65535;
            let word = match t {
                1 | 2 | 3 | 5 | 7 | 8 | 11 => Some(r.word()?),
                6 => {
                    r.string(1)?;
                    None
                }
                4 => r.take(12).map(|_| None)?,
                9 => r.take(44).map(|_| None)?,
                13 => r.take(8).map(|_| None)?,
                14 => r.take(32).map(|_| None)?,
                10 => {
                    r.word()?;
                    r.word()?;
                    let n = r.word()?;
                    if n >> 16 != 0 {
                        r.string(2)?;
                    }
                    if n & 65535 != 0 {
                        return Err(Malformed);
                    }
                    None
                }
                _ => return Err(Malformed),
            };
            if group == 0x4000_0017 && key == 0x4100_020B {
                // A non-word PhysObj would stop the Python preparation; treat it as unreadable.
                out.phys_obj = Some(word.ok_or(Malformed)?);
            }
        }
    }
    Ok(())
}

/// `population_probe.entity` (an EntityDesc).
fn entity(r: &mut Reader, types: &HashMap<u32, u32>) -> R<EntityDesc> {
    let bits = r.word()?;
    if bits & !511 != 0 {
        return Err(Malformed);
    }
    let mut e = EntityDesc::default();
    if bits & 1 != 0 {
        r.word()?;
    }
    if bits & 2 != 0 {
        r.word()?;
    }
    if bits & 4 != 0 {
        r.take(8)?;
    }
    if bits & 8 != 0 {
        e.data = r.word()?;
    }
    if bits & 16 != 0 {
        r.take(12 + 16 + 36)?;
    }
    if bits & 32 != 0 {
        r.take(12)?;
    }
    if bits & 128 != 0 {
        properties(r, types, &mut e)?;
    }
    if bits & 64 != 0 {
        r.word()?;
    }
    if bits & 256 != 0 {
        r.string(1)?;
    }
    Ok(e)
}

/// `content_profile.door_physical`: the PhysObj DID quality (key 1) of a CBaseQualities record.
fn door_physical(data: &[u8]) -> R<Option<u32>> {
    let mut r = Reader::new(data);
    let (mask, did, desc) = (r.word()?, r.word()?, r.word()?);
    if mask & 1 == 0 || did >> 24 != 0x81 || desc & 0x8000_0000 != 0 {
        return Err(Malformed);
    }
    fn string_info(r: &mut Reader, depth: u32) -> R<()> {
        if depth >= 8 {
            return Err(Malformed);
        }
        r.take(8)?;
        let flags = r.word()?;
        let count = flags & 65535;
        if count > 256 {
            return Err(Malformed);
        }
        if flags >> 16 != 0 {
            r.string(2)?;
        }
        for _ in 0..count {
            r.word()?;
            let kind = r.half()?;
            r.align()?;
            match kind {
                9 => string_info(r, depth + 1)?,
                2 | 6 => r.take(4).map(|_| ())?,
                3 | 4 | 7 => r.take(10).map(|_| ())?,
                5 => r.take(16).map(|_| ())?,
                10 | 12 => r.take(8).map(|_| ())?,
                11 | 13 => r.take(14).map(|_| ())?,
                0 | 1 | 8 => {}
                _ => return Err(Malformed),
            }
            r.align()?;
        }
        Ok(())
    }
    for bit in [1, 0x0200_0000, 2] {
        if desc & bit != 0 {
            r.word()?;
        }
    }
    for bit in [4, 8] {
        if desc & bit != 0 {
            string_info(&mut r, 0)?;
        }
    }
    for bit in [
        0x10u32, 0x20, 0x40, 0x100, 0x200, 0x2000_0000, 0x4000_0000, 0x0400_0000, 0x80, 0x800, 0x1000, 0x2000,
        0x20_0000, 0x40_0000, 0x8000, 0x1_0000, 0x2_0000, 0x4_0000, 0x8_0000, 0x10_0000, 0x0800_0000, 0x1000_0000,
    ] {
        if desc & bit != 0 {
            let wide = matches!(bit, 0x20 | 0x40 | 0x100 | 0x200 | 0x2000_0000 | 0x4000_0000 | 0x0400_0000);
            r.take(if wide { 8 } else { 4 })?;
        }
    }
    for (bit, width) in [(2u32, 8usize), (4, 8), (8, 8), (0x10, 12)] {
        if mask & bit != 0 {
            let count = (r.word()? & 0xff_ffff) as usize;
            if count > 512 {
                return Err(Malformed);
            }
            r.take(count * width)?;
        }
    }
    if mask & 0x20 != 0 {
        let count = r.word()? & 0xff_ffff;
        if count > 512 {
            return Err(Malformed);
        }
        for _ in 0..count {
            let (key, value) = (r.word()?, r.word()?);
            if key == 1 {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

/// `content_profile.ContentProfile`: the visual an entity template resolves to in these DATs.
struct ContentProfile {
    portal: Dat,
    types: HashMap<u32, u32>,
    cache: HashMap<u32, Option<u32>>,
}
impl ContentProfile {
    fn open(directory: &Path) -> io::Result<Self> {
        let mut portal = Dat::open(directory.join("portal.dat"))?;
        let bytes = portal.read(0x3400_0000)?;
        let types = (|| -> R<HashMap<u32, u32>> {
            let mut r = Reader::new(&bytes);
            if r.word()? != 0x3400_0000 {
                return Err(Malformed);
            }
            r.take(8)?;
            for _ in 0..r.word()? {
                r.word()?;
                r.string(1)?;
            }
            let mut types = HashMap::new();
            for _ in 0..r.word()? & 65535 {
                let key = r.word()?;
                types.insert(key, r.word()?);
                r.take(16)?;
            }
            Ok(types)
        })()
        .map_err(|_| bad("unreadable property table 0x34000000"))?;
        Ok(Self { portal, types, cache: HashMap::new() })
    }
    fn visual(&mut self, did: u32) -> Option<u32> {
        self.visual_at(did, 0)
    }
    fn visual_at(&mut self, did: u32, depth: u32) -> Option<u32> {
        if let Some(v) = self.cache.get(&did) {
            return *v;
        }
        let v = if depth > 32 { None } else { self.resolve(did, depth) };
        self.cache.insert(did, v);
        v
    }
    fn resolve(&mut self, mut did: u32, depth: u32) -> Option<u32> {
        let original = did;
        let mut seen = HashSet::new();
        for _ in 0..32 {
            if did >> 24 == 0x1f {
                return Some(did);
            }
            if did >> 24 != 0x47 && did != 0 {
                let data = self.portal.read(0x8100_0000 | (original & 0xff_ffff)).ok()?;
                let physical = door_physical(&data).ok()?;
                return match physical {
                    Some(p) if p != 0 && p != original => self.visual_at(p, depth + 1),
                    _ => None,
                };
            }
            if did >> 24 != 0x47 || !seen.insert(did) {
                return None;
            }
            let data = self.portal.read(did).ok()?;
            let mut r = Reader::new(&data);
            let desc = entity(&mut r, &self.types).ok()?;
            if r.pos != data.len() {
                return None;
            }
            // EntityDesc::GetTotalProperties: an explicit PhysObj overrides the
            // parent. Otherwise follow the template chain, never a script class.
            did = desc.phys_obj.filter(|p| *p != 0).unwrap_or(desc.data);
        }
        None
    }
}

/// Every record id in portal.dat and cell_1.dat.
struct AssetIndex {
    portal: HashSet<u32>,
    cell: HashSet<u32>,
}
fn asset_index(directory: &Path) -> io::Result<AssetIndex> {
    let dataset = ac2_dat::DatDataset::open(ac2_dat::DatOpenOptions::new(directory)).map_err(io::Error::other)?;
    Ok(AssetIndex {
        portal: dataset.iter_ids(ac2_dat::DatFileKind::Portal).collect(),
        cell: dataset.iter_ids(ac2_dat::DatFileKind::Cell).collect(),
    })
}

fn hex(v: &str) -> io::Result<u64> {
    let t = v.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u64::from_str_radix(t, 16).map_err(|_| bad(format!("invalid hexadecimal field {v:?}")))
}

fn missing_records(fields: &[&str], index: &AssetIndex) -> io::Result<Vec<Value>> {
    let entity = hex(fields[1])? as u32;
    let visual = hex(fields[2])? as u32;
    let cell = hex(fields[3])? as u32;
    let record = if cell & 65535 >= 256 { cell } else { cell | 65535 };
    Ok([("entity", "portal.dat", entity), ("visual", "portal.dat", visual), ("cell", "cell_1.dat", record)]
        .into_iter()
        .filter(|(_, archive, did)| !if *archive == "portal.dat" { &index.portal } else { &index.cell }.contains(did))
        .map(|(kind, archive, did)| json!({"kind": kind, "archive": archive, "record": format!("{did:08x}")}))
        .collect())
}

// ----------------------------------------------------------------------------- the packs

fn read(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}
fn read_json(path: &Path) -> io::Result<Value> {
    serde_json::from_str(&read(path)?).map_err(|e| bad(format!("{}: {e}", path.display())))
}
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
/// A checked-in file's digest with LF or CRLF endings (git `* text=auto`).
fn sha256_any_eol(path: &Path) -> io::Result<[String; 2]> {
    let raw = std::fs::read(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    let lf = String::from_utf8_lossy(&raw).replace("\r\n", "\n");
    Ok([sha256_hex(lf.as_bytes()), sha256_hex(lf.replace('\n', "\r\n").as_bytes())])
}
fn digest_in(value: Option<&Value>, path: &Path) -> io::Result<bool> {
    let digests = sha256_any_eol(path)?;
    Ok(value.and_then(Value::as_str).is_some_and(|v| digests.iter().any(|d| d == v)))
}
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
/// The first tab-separated field.
fn first(line: &str) -> &str {
    line.split('\t').next().unwrap_or("")
}
/// Python's `str.splitlines()` for these files: LF or CRLF line ends.
fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// Where the prepare step reads the world from: the content directory's pack files and evidence
/// (`Dir`), or the content database's `world_*` rows (`Db`, with the directory for the files not
/// in the database yet: the quest dump, the packs' source catalogs).
pub enum Source<'a> {
    Dir(&'a Path),
    Db { db: rusqlite::Connection, dir: &'a Path },
}
impl Source<'_> {
    fn dir(&self) -> &Path { match self { Self::Dir(d) | Self::Db { dir: d, .. } => d } }
    /// A pack, evidence or other content file of this name exists.
    fn exists(&self, name: &str) -> io::Result<bool> {
        match self {
            Self::Dir(d) => Ok(d.join(name).exists()),
            Self::Db { db, dir } => Ok(if let Some(pack) = name.strip_suffix(".tsv") {
                crate::content_db::world::pack_text(db, pack)?.is_some()
            } else if name.ends_with(".json") && (crate::content_db::world::evidence(db, name)?.is_some() || document(db, name)?.is_some()) {
                true
            } else {
                dir.join(name).exists()
            }),
        }
    }
    /// A pack's text (or another content file's).
    fn text(&self, name: &str) -> io::Result<String> {
        if let (Self::Db { db, .. }, Some(pack)) = (self, name.strip_suffix(".tsv"))
            && let Some(text) = crate::content_db::world::pack_text(db, pack)? {
            return Ok(text);
        }
        read(&self.dir().join(name))
    }
    /// An evidence (or catalog) JSON.
    fn json(&self, name: &str) -> io::Result<Value> {
        if let Self::Db { db, .. } = self
            && let Some(value) = crate::content_db::world::evidence(db, name)? {
            return Ok(value);
        }
        if let Self::Db { db, .. } = self
            && let Some(text) = document(db, name)? {
            return serde_json::from_str(&text).map_err(|e| bad(format!("{name}: {e}")));
        }
        read_json(&self.dir().join(name))
    }
    /// Whether a digest names this pack's (or file's) current content.
    fn digest_in(&self, value: Option<&Value>, name: &str) -> io::Result<bool> {
        if let (Self::Db { db, .. }, Some(pack)) = (self, name.strip_suffix(".tsv"))
            && let Some(text) = crate::content_db::world::pack_text(db, pack)? {
            return Ok(value.and_then(Value::as_str) == Some(sha256_hex(text.as_bytes()).as_str()));
        }
        if let Self::Db { db, .. } = self
            && let Some(text) = document(db, name)? {
            return Ok(value.and_then(Value::as_str) == Some(sha256_hex(text.as_bytes()).as_str()));
        }
        digest_in(value, &self.dir().join(name))
    }
}

/// A content document's text (a pack's source catalog), `None` when the database has none.
fn document(db: &rusqlite::Connection, name: &str) -> io::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    db.query_row("SELECT body FROM content_document WHERE name = ?1", [name], |r| r.get(0)).optional().map_err(|e| io::Error::other(e.to_string()))
}

/// `apply_placement_replacements`: retire audited observations only after their verified
/// replacement passes.
fn apply_placement_replacements(
    content: &Source,
    lines: Vec<String>,
    input_rows: &HashMap<(String, String), String>,
    admitted: &HashMap<String, String>,
) -> io::Result<(Vec<String>, Vec<Value>, Vec<Value>)> {
    if !content.exists("placement-replacements.json")? {
        return Ok((lines, Vec::new(), Vec::new()));
    }
    let policy = content.json("placement-replacements.json")?;
    if policy.get("schema") != Some(&json!(1)) {
        return Err(bad("Unsupported placement replacement schema"));
    }
    let rules = policy["replacements"].as_array().ok_or_else(|| bad("placement replacements"))?;
    let id = |v: &Value| v["id"].as_str().unwrap_or_default().to_owned();
    let retired: HashSet<String> = rules.iter().map(|r| id(&r["superseded"])).collect();
    let retained: HashSet<String> = rules.iter().map(|r| id(&r["retained"])).collect();
    if retired.len() != rules.len() || !retired.is_disjoint(&retained) {
        return Err(bad("Duplicate or chained placement replacements"));
    }
    let (mut removed, mut pending) = (Vec::new(), Vec::new());
    for rule in rules {
        let (old, new) = (&rule["superseded"], &rule["retained"]);
        if new["source"] != json!("recovered-population.tsv") {
            return Err(bad("Placement replacement requires verified recovered population"));
        }
        let mut rows = Vec::new();
        for endpoint in [old, new] {
            let key = (endpoint["source"].as_str().unwrap_or_default().to_owned(), id(endpoint));
            match input_rows.get(&key) {
                Some(row) if Some(sha256_hex(row.as_bytes()).as_str()) == endpoint["row_sha256"].as_str() => {
                    rows.push(row.split('\t').map(str::to_owned).collect::<Vec<_>>())
                }
                _ => return Err(bad(format!("Stale placement replacement evidence: {}", id(endpoint)))),
            }
        }
        if rows[0].get(1..3) != rows[1].get(1..3) {
            return Err(bad("Placement replacement must preserve entity and visual identity"));
        }
        if admitted.get(&id(old)).map(String::as_str) != old["source"].as_str() {
            continue;
        }
        let mut entry = rule.clone();
        if admitted.get(&id(new)).map(String::as_str) != new["source"].as_str() {
            entry["reason"] = json!("Replacement was not admitted; original observation retained");
            pending.push(entry);
        } else {
            removed.push(entry);
        }
    }
    let removed_ids: HashSet<String> = removed.iter().map(|e| id(&e["superseded"])).collect();
    Ok((lines.into_iter().filter(|l| !removed_ids.contains(first(l))).collect(), removed, pending))
}

/// `without_training_placements`: the human start instance without the training kit's authored
/// placements; the captured 0x0298 Shelter is copied into 0x0299 (see the Python's docstring).
fn without_training_placements(content: &Source, lines: Vec<String>, shelter_quest: bool) -> io::Result<(Vec<String>, Value)> {
    let kit_text = content.text("training-world.tsv")?;
    // The kit's placement ids (its header line is no id).
    let kit: HashSet<&str> = self::lines(&kit_text).into_iter().skip(1).filter(|l| !l.starts_with('#')).map(first).collect();
    let before = lines.len();
    let mut lines: Vec<String> = lines.into_iter().filter(|l| !kit.contains(first(l))).collect();
    let removed = before - lines.len();
    let mut taken: HashSet<String> = lines.iter().map(|l| first(l).to_lowercase()).collect();
    let mut mirrored = Vec::new();
    for line in &lines {
        let mut f: Vec<String> = line.split('\t').map(str::to_owned).collect();
        if f.len() > 4 && !line.starts_with('#') && f[3].to_lowercase().starts_with("0298") {
            let mut value = 0x8140_0299_0000_0000u64 | (hex(&f[0])? & 0xFFFF_FFFF);
            while taken.contains(&format!("{value:016x}")) {
                value += 1;
            }
            taken.insert(format!("{value:016x}"));
            f[0] = format!("{value:016x}");
            // The captured pink portal is the Shelter's quick exit (Python docstring).
            if f[1].to_lowercase() == QUICK_EXIT_ENTITY && f.len() > 12 && (f[12].is_empty() || f[12] == "-") {
                f[12] = QUICK_EXIT_NAME.into();
            }
            f[3] = format!("0299{}", f[3].get(4..).unwrap_or(""));
            mirrored.push(f.join("\t"));
        }
    }
    let mirrored_count = mirrored.len();
    lines.extend(mirrored);
    let mut added = 0;
    if shelter_quest && content.exists("human-shelter-world.tsv")? {
        let text = content.text("human-shelter-world.tsv")?;
        let extra: Vec<String> = self::lines(&text).into_iter().filter(|l| !l.starts_with('#')).skip(1).map(str::to_owned).collect();
        added = extra.len();
        lines.extend(extra);
    }
    Ok((lines, json!({"training_placements_removed": removed, "shelter_mirrored": mirrored_count, "shelter_quest_placements": added})))
}

/// Nearby lifestones share one outdoor space per landblock grid; sealed rooms do not.
fn landmark_position(cell: u32, p: [f64; 3]) -> (u32, [f64; 3]) {
    if (1..=64).contains(&(cell & 65535)) {
        (0, [p[0] + f64::from(cell >> 24) * 160.0, p[1] + f64::from((cell >> 16) & 255) * 160.0, p[2]])
    } else {
        (cell, p)
    }
}
fn nearby_landmark(cell: u32, p: [f64; 3], other_cell: u32, other: [f64; 3]) -> bool {
    let (a, pa) = landmark_position(cell, p);
    let (b, pb) = landmark_position(other_cell, other);
    a == b && pa.iter().zip(pb).map(|(x, y)| (x - y).powi(2)).sum::<f64>() <= 144.0
}

/// Builds the world table and its quest tables. `content` is the `content/` directory.
pub fn prepare(directory: &Path, content: &Path, options: Options) -> io::Result<Prepared> {
    let index = asset_index(directory)?;
    let mut profile = ContentProfile::open(directory)?;
    let dat_directory = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    let source = source_for(content)?;
    prepare_with(&index, &mut |did| profile.visual(did), &dat_directory.display().to_string(), &source, options)
}

/// The world's source: the content database when its dump holds the world, else the files.
pub fn source_for(content: &Path) -> io::Result<Source<'_>> {
    let dump = crate::content_db::dump_in(content);
    if dump.exists() {
        let db = crate::content_db::open_dump(&read(&dump)?)?;
        if crate::content_db::world::has_world(&db)? {
            return Ok(Source::Db { db, dir: content });
        }
    }
    Ok(Source::Dir(content))
}

/// `prepare` with the DATs' record index and visual resolution supplied (tests).
fn prepare_with(
    index: &AssetIndex,
    visual_of: &mut dyn FnMut(u32) -> Option<u32>,
    dat_directory: &str,
    content: &Source,
    options: Options,
) -> io::Result<Prepared> {
    let (newer, audited) = (content.exists("capture-spawns-upstream.tsv")?, content.exists("capture-spawns-audited.tsv")?);
    let mut version = if newer || audited { 4 } else { 3 };
    let mut lines = vec![format!("pyrealium-world-v{version}")];
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    let mut lifestones: Vec<(u64, u32, [f64; 3])> = Vec::new();
    let mut duplicates = Vec::new();
    let mut incompatible = Vec::new();
    let mut curated_pairs = HashSet::new();
    let mut sources: Vec<String> = vec!["world-observed-expanded.tsv".into(), "training-world.tsv".into()];
    let mut recovered_cells = HashSet::new();
    if content.exists("recovered-population.tsv")? {
        let evidence = content.json("recovered-population.report.json")?;
        if !truthy(evidence.get("native_ground_verified")) || !content.digest_in(evidence.get("manifest_sha256"), "recovered-population.tsv")? {
            return Err(bad("Recovered population requires current native ground verification"));
        }
        sources.push("recovered-population.tsv".into());
        version = 4;
        lines[0] = "pyrealium-world-v4".into();
    }
    sources.push("capture-spawns.tsv".into());
    if newer {
        sources.push("capture-spawns-upstream.tsv".into());
    }
    if audited {
        sources.push("capture-spawns-audited.tsv".into());
    }
    for (pack, catalog) in crate::content_db::world::CATALOGS {
        let pack = &format!("{pack}.tsv");
        if !content.exists(pack)? {
            continue;
        }
        let evidence = content.json(&pack.replace(".tsv", ".report.json"))?;
        let manifest = evidence.get("manifest_sha256").ok_or_else(|| bad(format!("{pack}: report lacks manifest_sha256")))?;
        let source = evidence.get("source_sha256").ok_or_else(|| bad(format!("{pack}: report lacks source_sha256")))?;
        if !truthy(evidence.get("native_ground_verified"))
            || !content.digest_in(Some(manifest), pack)?
            || !content.digest_in(Some(source), catalog)?
        {
            return Err(bad("Inferred content requires fresh native ground validation"));
        }
        sources.push(pack.into());
        version = 4;
        lines[0] = "pyrealium-world-v4".into();
    }
    // Inferred packs whose visuals are checked like captures (the Arwic mines' are authored).
    let checked_packs = [
        "recovered-population.tsv", "inferred-world-v1.tsv", "regional-world-v1.tsv", "wilderness-world-v1.tsv",
        "exploration-world-v1.tsv", "hunting-world-v1.tsv", "hunting-world-v2.tsv", "workshops-world-v1.tsv",
        "npc-world-v1.tsv", "gauntlet-world-v1.tsv",
    ];
    let mut state_exclusions: HashMap<String, Value> = HashMap::new();
    if audited && content.exists("capture-spawns-audited.report.json")? {
        let evidence = content.json("capture-spawns-audited.report.json")?;
        for e in evidence["excluded_existing"].as_array().ok_or_else(|| bad("excluded_existing"))? {
            state_exclusions.insert(e["id"].as_str().unwrap_or_default().to_owned(), e.clone());
        }
    }
    let mut transient = Vec::new();
    let mut input_seen = HashSet::new();
    let mut input_counts: Vec<(String, u64)> = Vec::new();
    let mut input_rows: HashMap<(String, String), String> = HashMap::new();
    let mut admitted: HashMap<String, String> = HashMap::new();
    let mut earlier_capture_cells = HashSet::new();
    let mut overlapping = Vec::new();
    for name in &sources {
        let mut source_cells = HashSet::new();
        let text = content.text(name)?;
        for line in self::lines(&text).into_iter().skip(1) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields: Vec<&str> = line.split('\t').collect();
            if !matches!(fields.len(), 12 | 13) {
                return Err(bad(format!("Unsupported placement in {name}")));
            }
            let iid = hex(fields[0])?;
            let entity = hex(fields[1])? as u32;
            let visual = hex(fields[2])? as u32;
            let cell = hex(fields[3])? as u32;
            input_rows.insert((name.clone(), fields[0].to_owned()), line.to_owned());
            // A later capture cannot replace an earlier placement identity.
            if !input_seen.insert(iid) {
                continue;
            }
            match input_counts.iter_mut().find(|(n, _)| n == name) {
                Some((_, count)) => *count += 1,
                None => input_counts.push((name.clone(), 1)),
            }
            if name.starts_with("capture-spawns") && recovered_cells.contains(&cell) {
                overlapping.push(json!({"source": name, "id": fields[0], "cell": fields[3],
                    "reason": "verified translated snapshot owns this cell"}));
                continue;
            }
            if matches!(name.as_str(), "capture-spawns-upstream.tsv" | "capture-spawns-audited.tsv")
                && earlier_capture_cells.contains(&cell)
            {
                overlapping.push(json!({"source": name, "id": fields[0], "cell": fields[3],
                    "reason": "earlier capture already owns this cell"}));
                continue;
            }
            if name == "world-observed-expanded.tsv" || name.starts_with("capture-spawns") {
                source_cells.insert(cell);
            }
            if let Some(exclusion) = state_exclusions.get(fields[0])
                && exclusion["source"].as_str() == Some(name.as_str())
                && exclusion["row_sha256"].as_str() == Some(sha256_hex(line.as_bytes()).as_str())
            {
                transient.push(exclusion.clone());
                continue;
            }
            // Outdoor cells use the landblock record; interior cells have individual records.
            let missing = missing_records(&fields, index)?;
            if !missing.is_empty() {
                skipped.push(json!({"source": name, "id": fields[0], "entity": fields[1], "visual": fields[2],
                    "cell": fields[3], "missing_records": missing}));
                continue;
            }
            if seen.contains(&iid) {
                continue;
            }
            if name.starts_with("capture-spawns") || checked_packs.contains(&name.as_str()) {
                let expected = visual_of(entity);
                if !curated_pairs.contains(&(entity, visual)) && expected != Some(visual) {
                    let known = expected.filter(|v| *v != 0);
                    incompatible.push(json!({"source": name, "id": fields[0], "entity": fields[1],
                        "visual": fields[2], "cell": fields[3],
                        "dat_visual": known.map(|v| format!("{v:08x}")),
                        "reason": if known.is_some() { "visual differs from installed entity template" }
                                  else { "capture visual cannot be verified against installed templates" }}));
                    continue;
                }
            } else {
                // The curated observation and authored training packs supply
                // known profiles for script-only templates (e.g. lifestones).
                curated_pairs.insert((entity, visual));
            }
            if entity == 0x4700_07de {
                let parse = |v: &str| v.trim().parse::<f64>().map_err(|_| bad(format!("invalid coordinate {v:?}")));
                let position = [parse(fields[4])?, parse(fields[5])?, parse(fields[6])?];
                if let Some((old, ..)) = lifestones.iter().find(|(_, c, p)| nearby_landmark(cell, position, *c, *p)) {
                    duplicates.push(json!({"source": name, "id": fields[0], "retained": format!("{old:016x}")}));
                    continue;
                }
                lifestones.push((iid, cell, position));
            }
            if version == 4 && fields.len() == 12 {
                fields.push("-");
            }
            lines.push(fields.join("\t"));
            seen.insert(iid);
            admitted.insert(fields[0].to_owned(), name.clone());
            if name == "recovered-population.tsv" {
                recovered_cells.insert(cell);
            }
        }
        earlier_capture_cells.extend(source_cells);
    }
    let (mut lines, superseded, pending) = apply_placement_replacements(content, lines, &input_rows, &admitted)?;
    for entry in &superseded {
        if let Some(id) = entry["superseded"]["id"].as_str() {
            seen.remove(&hex(id)?);
        }
    }
    let mut start_instance = None;
    if !options.training_placements {
        let (next, summary) = without_training_placements(content, lines, options.shelter_quest)?;
        lines = next;
        start_instance = Some(summary);
    }
    let mut world = lines.join("\n");
    world.push('\n');
    let mut tables = Vec::new();
    for name in TABLES {
        tables.push((name.to_owned(), read(&crate::content_db::dump_in(content.dir()))?));
    }
    let mut counts = BTreeMap::new();
    for kind in ["entity", "visual", "cell"] {
        let unique: BTreeSet<&str> = skipped
            .iter()
            .flat_map(|e| e["missing_records"].as_array().into_iter().flatten())
            .filter(|r| r["kind"] == kind)
            .filter_map(|r| r["record"].as_str())
            .collect();
        counts.insert(kind, unique.len());
    }
    let excluded = skipped.len() + incompatible.len() + duplicates.len() + overlapping.len() + transient.len() + superseded.len();
    let mut source_counts = Map::new();
    for (name, count) in &input_counts {
        source_counts.insert(name.clone(), json!(count));
    }
    let mut report = json!({
        "included": seen.len(), "excluded": excluded, "missing_assets": skipped,
        "superseded_placements": superseded, "unapplied_placement_replacements": pending,
        "transient_capture_placements": transient,
        "source_unique_placements": source_counts,
        "overlapping_capture_cells": overlapping,
        "inactive_training_services": [], "duplicate_landmarks": duplicates,
        "incompatible_capture_placements": incompatible,
        "dat_directory": dat_directory,
        "missing_record_counts": counts,
    });
    if let Some(summary) = &start_instance {
        report["start_instance"] = summary.clone();
    }
    let mut summary = vec![format!(
        "Compatible server placements: {}; placements missing DAT records: {}; duplicate landmarks: {}; inactive training service props: 0",
        seen.len(),
        skipped.len(),
        duplicates.len()
    )];
    if let Some(s) = &start_instance {
        summary.push(format!(
            "Training placements removed: {}; Shelter mirrored into 0x0299: {}; Shelter Hold quest placements: {}",
            s["training_placements_removed"], s["shelter_mirrored"], s["shelter_quest_placements"]
        ));
    }
    summary.push(format!("Unverified or incompatible capture placements withheld: {}.", incompatible.len()));
    if !overlapping.is_empty() {
        summary.push(format!("Overlapping capture observations withheld: {}; one verified snapshot retained per cell.", overlapping.len()));
    }
    if !transient.is_empty() {
        summary.push(format!("Captured looted/dead objects withheld: {}; exact original observation rows retained in source files.", transient.len()));
    }
    if !superseded.is_empty() {
        summary.push(format!("Superseded duplicate placements withheld: {}; verified replacements retained.", superseded.len()));
    }
    if !pending.is_empty() {
        summary.push(format!("Placement replacements unavailable: {}; original observations retained.", pending.len()));
    }
    if !skipped.is_empty() {
        summary.push(format!("Missing unique records: entity {}, visual {}, cell {}.", counts["entity"], counts["visual"], counts["cell"]));
    }
    Ok(Prepared { world, tables, report, summary })
}

/// Writes a preparation as files: the world table at `output`, its report beside it as
/// `NAME.report.json`, and the quest tables in the same directory (stale families removed).
pub fn write_files(prepared: &Prepared, output: &Path) -> io::Result<()> {
    let directory = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)?;
    std::fs::write(output, &prepared.world)?;
    for name in RETIRED_TABLES {
        let _ = std::fs::remove_file(directory.join(name));
        let prefix = format!("{}-", name.trim_end_matches(".tsv"));
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".tsv")) {
                std::fs::remove_file(path)?;
            }
        }
    }
    for (name, text) in &prepared.tables {
        std::fs::write(directory.join(name), text)?;
    }
    let report = serde_json::to_string_pretty(&prepared.report).map_err(io::Error::other)?;
    std::fs::write(output.with_extension("report.json"), report)
}

/// What a preparation depends on: the preparation version and options, every file in the
/// content directory, and the DAT files' identity (size and modification time). `run` prepares
/// again only when this changes.
pub fn input_key(directory: &Path, content: &Path, options: Options) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(format!("v{VERSION} training={} shelter={}\n", options.training_placements, options.shelter_quest));
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(content)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
    files.sort();
    for path in files {
        let bytes = std::fs::read(&path)?;
        hash.update(path.file_name().unwrap().to_string_lossy().as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    for name in ["portal.dat", "cell_1.dat"] {
        let meta = std::fs::metadata(directory.join(name))
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", directory.join(name).display())))?;
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        hash.update(format!("{name} {} {modified}\n", meta.len()));
    }
    Ok(Sha256::digest(hash.finalize()).iter().map(|b| format!("{b:02x}")).collect())
}

/// The world file name under which the placement table is stored in `content_files`.
pub const WORLD_FILE: &str = "world.tsv";

/// Replaces the stored prepared content in one transaction.
pub fn store(connection: &mut crate::db::Connection, key: &str, prepared: &Prepared) -> io::Result<()> {
    use crate::db::error;
    let tx = connection.transaction().map_err(error)?;
    tx.execute("DELETE FROM content_files", []).map_err(error)?;
    tx.execute("DELETE FROM content_meta", []).map_err(error)?;
    tx.execute("INSERT INTO content_files (name, body) VALUES (?1, ?2)", [WORLD_FILE, prepared.world.as_str()])
        .map_err(error)?;
    for (name, text) in &prepared.tables {
        tx.execute("INSERT INTO content_files (name, body) VALUES (?1, ?2)", [name, text]).map_err(error)?;
    }
    tx.execute(
        "INSERT INTO content_meta (id, input_hash, version, report) VALUES (1, ?1, ?2, ?3)",
        rusqlite::params![key, VERSION, crate::db::json_text(&prepared.report)?],
    )
    .map_err(error)?;
    tx.commit().map_err(error)
}

/// The stored preparation's input key, if any.
pub fn stored_key(connection: &mut crate::db::Connection) -> io::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    connection
        .query_row("SELECT input_hash FROM content_meta WHERE version = ?1", [VERSION], |r| r.get(0))
        .optional()
        .map_err(crate::db::error)
}

/// Prepared content as the server loads it: the world table and the quest tables.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub world: String,
    pub tables: Vec<(String, String)>,
}

impl World {
    /// The content database's dump (`quest_emotes::Tables::from_sql`), if prepared (stored as
    /// `content.sql`, or `quests.sql` by an older prepare).
    pub fn quest_sql(&self) -> Option<&str> {
        self.tables.iter().find(|(n, _)| n == crate::content_db::DUMP || n == crate::content_db::OLD_DUMP).map(|(_, t)| t.as_str())
    }
}

pub fn load(client: &mut crate::db::Connection) -> io::Result<Option<World>> {
    let mut out = World::default();
    let mut found = false;
    let mut statement = client.prepare("SELECT name, body FROM content_files ORDER BY name").map_err(crate::db::error)?;
    let files = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(crate::db::error)?;
    for file in files {
        let (name, body) = file.map_err(crate::db::error)?;
        if name == WORLD_FILE {
            out.world = body;
            found = true;
        } else {
            out.tables.push((name, body));
        }
    }
    Ok(found.then_some(out))
}

#[cfg(test)]
mod tests;
