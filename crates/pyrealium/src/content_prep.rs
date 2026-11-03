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
pub const VERSION: i32 = 1;

/// The four scripted-quest tables served beside the world table, each with its families.
pub const TABLES: [&str; 4] = ["quest-registry.tsv", "quest-stages.tsv", "emote-sets.tsv", "emote-actions.tsv"];

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

/// `apply_placement_replacements`: retire audited observations only after their verified
/// replacement passes.
fn apply_placement_replacements(
    content: &Path,
    lines: Vec<String>,
    input_rows: &HashMap<(String, String), String>,
    admitted: &HashMap<String, String>,
) -> io::Result<(Vec<String>, Vec<Value>, Vec<Value>)> {
    let path = content.join("placement-replacements.json");
    if !path.exists() {
        return Ok((lines, Vec::new(), Vec::new()));
    }
    let policy = read_json(&path)?;
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
fn without_training_placements(content: &Path, lines: Vec<String>, shelter_quest: bool) -> io::Result<(Vec<String>, Value)> {
    let kit_text = read(&content.join("training-world.tsv"))?;
    let kit: HashSet<&str> = self::lines(&kit_text).into_iter().filter(|l| !l.starts_with('#')).map(first).collect();
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
    let shelter_world = content.join("human-shelter-world.tsv");
    if shelter_quest && shelter_world.exists() {
        let text = read(&shelter_world)?;
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
    prepare_with(&index, &mut |did| profile.visual(did), &dat_directory.display().to_string(), content, options)
}

/// `prepare` with the DATs' record index and visual resolution supplied (tests).
fn prepare_with(
    index: &AssetIndex,
    visual_of: &mut dyn FnMut(u32) -> Option<u32>,
    dat_directory: &str,
    content: &Path,
    options: Options,
) -> io::Result<Prepared> {
    let newer = content.join("capture-spawns-upstream.tsv");
    let audited = content.join("capture-spawns-audited.tsv");
    let recovered = content.join("recovered-population.tsv");
    let mut version = if newer.exists() || audited.exists() { 4 } else { 3 };
    let mut lines = vec![format!("pyrealium-world-v{version}")];
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    let mut lifestones: Vec<(u64, u32, [f64; 3])> = Vec::new();
    let mut duplicates = Vec::new();
    let mut incompatible = Vec::new();
    let mut curated_pairs = HashSet::new();
    let mut sources: Vec<String> = vec!["world-observed-expanded.tsv".into(), "training-world.tsv".into()];
    let mut recovered_cells = HashSet::new();
    if recovered.exists() {
        let evidence = read_json(&recovered.with_extension("report.json"))?;
        if !truthy(evidence.get("native_ground_verified")) || !digest_in(evidence.get("manifest_sha256"), &recovered)? {
            return Err(bad("Recovered population requires current native ground verification"));
        }
        sources.push("recovered-population.tsv".into());
        version = 4;
        lines[0] = "pyrealium-world-v4".into();
    }
    sources.push("capture-spawns.tsv".into());
    if newer.exists() {
        sources.push("capture-spawns-upstream.tsv".into());
    }
    if audited.exists() {
        sources.push("capture-spawns-audited.tsv".into());
    }
    let packs = [
        ("inferred-world-v1.tsv", "inferred-content-v1.json"),
        ("regional-world-v1.tsv", "regional-content-v1.json"),
        ("wilderness-world-v1.tsv", "wilderness-content-v1.json"),
        ("exploration-world-v1.tsv", "exploration-content-v1.json"),
        ("hunting-world-v1.tsv", "hunting-content-v1.json"),
        ("hunting-world-v2.tsv", "hunting-content-v2.json"),
        ("workshops-world-v1.tsv", "workshops-content-v1.json"),
        ("arwic-mines-world-v1.tsv", "arwic-mines-world-v1.json"),
        ("npc-world-v1.tsv", "npc-dialogue-v1.json"),
    ];
    for (pack, catalog) in packs {
        let path = content.join(pack);
        if !path.exists() {
            continue;
        }
        let evidence = read_json(&path.with_extension("report.json"))?;
        let manifest = evidence.get("manifest_sha256").ok_or_else(|| bad(format!("{pack}: report lacks manifest_sha256")))?;
        let source = evidence.get("source_sha256").ok_or_else(|| bad(format!("{pack}: report lacks source_sha256")))?;
        if !truthy(evidence.get("native_ground_verified"))
            || !digest_in(Some(manifest), &path)?
            || !digest_in(Some(source), &content.join(catalog))?
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
        "npc-world-v1.tsv",
    ];
    let state_file = content.join("capture-spawns-audited.report.json");
    let mut state_exclusions: HashMap<String, Value> = HashMap::new();
    if audited.exists() && state_file.exists() {
        let evidence = read_json(&state_file)?;
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
        let text = read(&content.join(name))?;
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
        let stem = name.trim_end_matches(".tsv");
        tables.push((name.to_owned(), read(&content.join(name))?));
        let mut family: Vec<PathBuf> = std::fs::read_dir(content)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{stem}-")) && n.ends_with(".tsv"))
            })
            .collect();
        family.sort();
        for path in family {
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            tables.push((file, read(&path)?));
        }
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
    for name in TABLES {
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
pub fn store(client: &mut postgres::Client, key: &str, prepared: &Prepared) -> io::Result<()> {
    let mut tx = client.transaction().map_err(crate::db::error)?;
    tx.execute("DELETE FROM content_files", &[]).map_err(crate::db::error)?;
    tx.execute("DELETE FROM content_meta", &[]).map_err(crate::db::error)?;
    tx.execute("INSERT INTO content_files (name, body) VALUES ($1, $2)", &[&WORLD_FILE, &prepared.world])
        .map_err(crate::db::error)?;
    for (name, text) in &prepared.tables {
        tx.execute("INSERT INTO content_files (name, body) VALUES ($1, $2)", &[name, text]).map_err(crate::db::error)?;
    }
    tx.execute(
        "INSERT INTO content_meta (input_hash, version, report) VALUES ($1, $2, $3)",
        &[&key, &VERSION, &prepared.report],
    )
    .map_err(crate::db::error)?;
    tx.commit().map_err(crate::db::error)
}

/// The stored preparation's input key, if any.
pub fn stored_key(client: &mut postgres::Client) -> io::Result<Option<String>> {
    Ok(client
        .query_opt("SELECT input_hash FROM content_meta WHERE version = $1", &[&VERSION])
        .map_err(crate::db::error)?
        .map(|r| r.get(0)))
}

/// Prepared content as the server loads it: the world table and the quest tables.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub world: String,
    pub tables: Vec<(String, String)>,
}

impl World {
    /// The quest tables grouped as `quest_emotes::Tables::parse_files` takes them: each base
    /// table first, then its families in name order.
    pub fn quest_files(&self) -> [Vec<(String, String)>; 4] {
        let mut files: [Vec<(String, String)>; 4] = Default::default();
        for (base, list) in TABLES.iter().zip(files.iter_mut()) {
            let prefix = format!("{}-", base.trim_end_matches(".tsv"));
            let mut family: Vec<&(String, String)> = self.tables.iter().filter(|(n, _)| n.starts_with(&prefix)).collect();
            family.sort_by(|a, b| a.0.cmp(&b.0));
            list.extend(self.tables.iter().filter(|(n, _)| n == base).cloned());
            list.extend(family.into_iter().cloned());
        }
        files
    }
}

pub fn load(client: &mut postgres::Client) -> io::Result<Option<World>> {
    let mut out = World::default();
    let mut found = false;
    for row in client.query("SELECT name, body FROM content_files ORDER BY name", &[]).map_err(crate::db::error)? {
        let (name, body): (String, String) = (row.get(0), row.get(1));
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
mod tests {
    use super::*;

    fn sha(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }
    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pyrl-prep-{}-{}", std::process::id(), crate::auth::hex(&crate::auth::random::<6>().unwrap())));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn write(content: &Path, name: &str, rows: &[&str], header: &str, trailing: bool) {
        let mut text = format!("{header}\n{}", rows.join("\n"));
        if trailing {
            text.push('\n');
        }
        std::fs::write(content.join(name), text).unwrap();
    }
    fn index(portal: &[u32], cell: &[u32]) -> AssetIndex {
        AssetIndex { portal: portal.iter().copied().collect(), cell: cell.iter().copied().collect() }
    }
    const FULL: Options = Options { training_placements: true, shelter_quest: true };
    fn run(index: &AssetIndex, visual: u32, content: &Path) -> io::Result<Prepared> {
        prepare_with(index, &mut |_| Some(visual), "dat", content, FULL)
    }
    fn world_lines(p: &Prepared) -> Vec<String> {
        p.world.lines().map(str::to_owned).collect()
    }
    /// The base files every fixture needs, plus the four quest tables the preparation copies.
    fn base(content: &Path, header: &str) {
        for name in ["world-observed-expanded.tsv", "training-world.tsv", "capture-spawns.tsv"] {
            write(content, name, &[], header, false);
        }
        for name in TABLES {
            std::fs::write(content.join(name), "").unwrap();
        }
    }

    #[test]
    fn evidence_hash_accepts_checkout_line_endings_without_accepting_edits() {
        let d = dir();
        let path = d.join("content.tsv");
        let lf = b"header\nentry\tvalue\n".to_vec();
        let crlf = String::from_utf8(lf.clone()).unwrap().replace('\n', "\r\n").into_bytes();
        let expected = [sha(&lf), sha(&crlf)];
        for data in [&lf, &crlf] {
            std::fs::write(&path, data).unwrap();
            assert_eq!(sha256_any_eol(&path).unwrap(), expected);
        }
        std::fs::write(&path, b"header\nentry\tchanged\n").unwrap();
        assert!(sha256_any_eol(&path).unwrap().iter().all(|d| !expected.contains(d)));
        std::fs::remove_dir_all(d).unwrap();
    }

    fn authored_pack_evidence(pack: &str, version: u32, catalog_name: Option<&str>) {
        let content = dir();
        base(&content, "pyrealium-world-v4");
        let row = "8140c0000077b800\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t-\tCreature";
        let manifest = content.join(format!("{pack}-world-v{version}.tsv"));
        std::fs::write(&manifest, format!("pyrealium-world-v4\n{row}\n")).unwrap();
        let catalog = content.join(catalog_name.map_or_else(|| format!("{pack}-content-v{version}.json"), str::to_owned));
        std::fs::write(&catalog, "{}").unwrap();
        let evidence = json!({"native_ground_verified": true,
            "manifest_sha256": sha(&std::fs::read(&manifest).unwrap()),
            "source_sha256": sha(&std::fs::read(&catalog).unwrap())});
        std::fs::write(manifest.with_extension("report.json"), evidence.to_string()).unwrap();
        let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff]);
        let out = run(&idx, 0x1f000001, &content).unwrap();
        assert_eq!(world_lines(&out), ["pyrealium-world-v4", row]);
        let out = run(&idx, 0x1f000002, &content).unwrap();
        assert_eq!(world_lines(&out).len(), 1, "a visual the DATs disagree with is withheld");
        std::fs::write(&catalog, "{\"changed\":true}").unwrap();
        let error = run(&idx, 0x1f000001, &content).err().unwrap();
        assert!(error.to_string().contains("fresh native ground"), "{error}");
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn authored_packs_require_matching_ground_proof_and_native_visual() {
        authored_pack_evidence("regional", 1, None);
        authored_pack_evidence("exploration", 1, None);
        authored_pack_evidence("hunting", 2, None);
        authored_pack_evidence("npc", 1, Some("npc-dialogue-v1.json"));
    }

    const OLD: &str = "8140000000000001\t47001c27\t1f001042\t76b80027\t98.1885986\t133.502838\t168.323868\t1\t0\t0\t0\t-";
    const NEW: &str = "8140000000000002\t47001c27\t1f001042\t77b80006\t1.37962341\t100.779083\t182.50499\t1\t0\t0\t0\t-\tOrven";
    fn replacement_fixture() -> (PathBuf, AssetIndex, String) {
        let content = dir();
        base(&content, "pyrealium-world-v4");
        // The same template in another location is not automatically a duplicate.
        let other = OLD.replace("8140000000000001", "8140000000000003").replace("76b80027", "80b80027");
        write(&content, "world-observed-expanded.tsv", &[OLD, &other], "pyrealium-world-v4", true);
        write(&content, "training-world.tsv", &[], "pyrealium-world-v4", true);
        write(&content, "capture-spawns.tsv", &[], "pyrealium-world-v4", true);
        write(&content, "recovered-population.tsv", &[NEW], "pyrealium-world-v4", true);
        let recovered = content.join("recovered-population.tsv");
        std::fs::write(recovered.with_extension("report.json"),
            json!({"native_ground_verified": true, "manifest_sha256": sha(&std::fs::read(&recovered).unwrap())}).to_string()).unwrap();
        let endpoint = |source: &str, row: &str| json!({"source": source, "id": first(row), "row_sha256": sha(row.as_bytes())});
        let policy = json!({"schema": 1, "replacements": [{"name": "Orven", "reason": "Audited duplicate",
            "superseded": endpoint("world-observed-expanded.tsv", OLD),
            "retained": endpoint("recovered-population.tsv", NEW)}]});
        std::fs::write(content.join("placement-replacements.json"), policy.to_string()).unwrap();
        (content, index(&[0x47001c27, 0x1f001042], &[0x76b8ffff, 0x77b8ffff, 0x80b8ffff]), other)
    }
    #[test]
    fn audited_npc_replacement_crosses_cells_without_template_deduplication() {
        let (content, idx, other) = replacement_fixture();
        let out = run(&idx, 0x1f001042, &content).unwrap();
        assert_eq!(world_lines(&out), ["pyrealium-world-v4".to_owned(), format!("{other}\t-"), NEW.to_owned()]);
        assert_eq!(out.report["included"], 2);
        assert_eq!(out.report["excluded"], 1);
        assert_eq!(out.report["superseded_placements"][0]["superseded"]["id"], first(OLD));
        assert!(out.report["unapplied_placement_replacements"].as_array().unwrap().is_empty());
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn unavailable_replacement_preserves_original_npc() {
        let (content, mut idx, other) = replacement_fixture();
        idx.cell.remove(&0x77b8ffff);
        let out = run(&idx, 0, &content).unwrap();
        assert_eq!(world_lines(&out), ["pyrealium-world-v4".to_owned(), format!("{OLD}\t-"), format!("{other}\t-")]);
        assert!(out.report["superseded_placements"].as_array().unwrap().is_empty());
        assert_eq!(out.report["unapplied_placement_replacements"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn stale_replacement_proof_is_refused() {
        for endpoint in ["superseded", "retained"] {
            let (content, idx, _) = replacement_fixture();
            let path = content.join("placement-replacements.json");
            let mut policy: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            policy["replacements"][0][endpoint]["row_sha256"] = json!("stale");
            std::fs::write(&path, policy.to_string()).unwrap();
            let error = run(&idx, 0x1f001042, &content).err().unwrap();
            assert!(error.to_string().contains("Stale placement replacement evidence"), "{endpoint}: {error}");
            std::fs::remove_dir_all(content).unwrap();
        }
    }
    #[test]
    fn verified_translation_wins_over_old_cell_snapshots() {
        let content = dir();
        base(&content, "pyrealium-world-v4");
        let row = "8140000000000001\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t-\tBeetle";
        let old = row.replace("00000001\t470", "00000002\t470");
        write(&content, "capture-spawns.tsv", &[&old], "pyrealium-world-v4", true);
        write(&content, "recovered-population.tsv", &[row], "pyrealium-world-v4", true);
        let path = content.join("recovered-population.tsv");
        std::fs::write(path.with_extension("report.json"),
            json!({"native_ground_verified": true, "manifest_sha256": sha(&std::fs::read(&path).unwrap())}).to_string()).unwrap();
        let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff]);
        assert_eq!(world_lines(&run(&idx, 0x1f000001, &content).unwrap()), ["pyrealium-world-v4", row]);
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("5\t5\t0", "6\t5\t0")).unwrap();
        assert!(run(&idx, 0x1f000001, &content).err().unwrap().to_string().contains("ground verification"));
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn audited_pack_respects_upstream_cells_and_exact_state_evidence() {
        let content = dir();
        base(&content, "pyrealium-world-v4");
        let row = "8140000000000001\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t-\t-";
        let changed = row.replace("00000001\t470", "00000002\t470").replace("01020001", "01030001");
        let overlap = row.replace("00000001\t470", "00000003\t470");
        let fresh = row.replace("00000001\t470", "00000004\t470").replace("01020001", "01040001");
        write(&content, "capture-spawns-upstream.tsv", &[row, &changed], "pyrealium-world-v4", false);
        write(&content, "capture-spawns-audited.tsv", &[&overlap, &fresh], "pyrealium-world-v4", false);
        let proof = json!({"excluded_existing": [
            {"id": first(row), "source": "capture-spawns-upstream.tsv", "row_sha256": sha(row.as_bytes())},
            {"id": first(&changed), "source": "capture-spawns-upstream.tsv", "row_sha256": "outdated-evidence"}]});
        std::fs::write(content.join("capture-spawns-audited.report.json"), proof.to_string()).unwrap();
        let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff, 0x0103ffff, 0x0104ffff]);
        let out = run(&idx, 0x1f000001, &content).unwrap();
        assert_eq!(world_lines(&out), ["pyrealium-world-v4", &changed, &fresh]);
        assert_eq!(out.report["transient_capture_placements"].as_array().unwrap().len(), 1);
        assert_eq!(out.report["overlapping_capture_cells"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn door_defaults_respect_nested_strings_and_truncated_tables() {
        let words = |v: &[u32]| v.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
        // Display name contains a tagged nested StringInfo, followed by a DID table.
        let mut name = words(&[0, 0, 1, 0x41000001]);
        name.extend(9u16.to_le_bytes());
        name.extend(0u16.to_le_bytes());
        name.extend(words(&[0, 0, 0]));
        let mut data = words(&[0x21, 0x8100114a, 4]);
        data.extend(name);
        data.extend(words(&[1, 1, 0x47001145]));
        assert_eq!(door_physical(&data).unwrap(), Some(0x47001145));
        for end in 0..data.len() {
            assert!(door_physical(&data[..end]).is_err(), "accepted a table cut at {end}");
        }
    }
    #[test]
    fn existing_did_with_wrong_model_is_withheld_not_repositioned() {
        let content = dir();
        base(&content, "pyrealium-world-v3");
        let wrong = "8140aab372ef1689\t4700334c\t1f0010a8\t77b80006\t1.37962341\t100.779083\t182.50499\t1\t0\t0\t0\t-";
        let valid = wrong.replace("8140aab372ef1689", "8140000000000002").replace("1f0010a8", "1f0010a6");
        write(&content, "capture-spawns.tsv", &[wrong, &valid], "pyrealium-world-v3", false);
        let idx = index(&[0x4700334c, 0x1f0010a8, 0x1f0010a6], &[0x77b8ffff]);
        let out = run(&idx, 0x1f0010a6, &content).unwrap();
        assert_eq!(world_lines(&out), ["pyrealium-world-v3", &valid]);
        assert_eq!(out.report["incompatible_capture_placements"][0]["id"], "8140aab372ef1689");
        assert_eq!(out.report["incompatible_capture_placements"][0]["dat_visual"], "1f0010a6");
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn landmark_comparison_crosses_outdoor_cells_but_not_dungeon_rooms() {
        assert!(nearby_landmark(0x76b80022, [85.5575, 29.5398, 182.505], 0x76b8001a, [77.260666, 27.62573, 182.5]));
        assert!(nearby_landmark(0x76b80040, [159.0, 20.0, 182.5], 0x77b80001, [1.0, 20.0, 182.5]));
        assert!(!nearby_landmark(0x02990109, [0.0, 0.0, -44.0], 0x0299010a, [0.0, 0.0, -44.0]));
        assert!(!nearby_landmark(0x76b80022, [0.0, 0.0, 182.0], 0x76b80022, [13.0, 0.0, 182.0]));
    }
    #[test]
    fn new_capture_names_preserved_without_stacking_cell_snapshots() {
        let content = dir();
        base(&content, "pyrealium-world-v4");
        let old = "8140000000000001\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t1";
        let overlap = format!("{}\tNew observation", old.replace("00000001\t470", "00000002\t470"));
        let fresh = format!("{}\tNamed gateway", old.replace("00000001\t470", "00000003\t470").replace("01020001", "01030001"));
        write(&content, "capture-spawns.tsv", &[old], "pyrealium-world-v4", false);
        write(&content, "capture-spawns-upstream.tsv", &[&overlap, &fresh], "pyrealium-world-v4", false);
        let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff, 0x0103ffff]);
        let out = run(&idx, 0x1f000001, &content).unwrap();
        assert_eq!(world_lines(&out), ["pyrealium-world-v4".to_owned(), format!("{old}\t-"), fresh]);
        assert_eq!(out.report["overlapping_capture_cells"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn stripped_training_placements_leave_the_mirrored_shelter_and_its_quest() {
        let content = dir();
        std::fs::write(content.join("training-world.tsv"),
            "pyrealium-world-v3\n# kit\n8140029900000001\t47000001\t1f000001\t0299011a\n").unwrap();
        std::fs::write(content.join("human-shelter-world.tsv"),
            "pyrealium-world-v4\n# evidence\n8140029900000100\t47000002\t1f000002\t0299014d\n").unwrap();
        let mut portal: Vec<String> = ["8140abcd12345678", "47001451", "1f000003", "02980120"].map(str::to_owned).to_vec();
        portal.extend(std::iter::repeat_n("0".to_owned(), 8));
        portal.extend(["-".to_owned(), "Portal".to_owned()]);
        let lines: Vec<String> = vec![
            "pyrealium-world-v4".into(),
            "8140029900000001\t47000001\t1f000001\t0299011a".into(),
            portal.join("\t"),
            "8140029912345678\t47000004\t1f000004\t01020001\t5".into(),
        ];
        let (result, counts) = without_training_placements(&content, lines.clone(), true).unwrap();
        let mut mirrored: Vec<String> = ["8140029912345679", "47001451", "1f000003", "02990120"].map(str::to_owned).to_vec();
        mirrored.extend(std::iter::repeat_n("0".to_owned(), 8));
        mirrored.extend([QUICK_EXIT_NAME.to_owned(), "Portal".to_owned()]);
        let mut expected = vec![lines[0].clone(), lines[2].clone(), lines[3].clone()];
        expected.push(mirrored.join("\t"));
        expected.push("8140029900000100\t47000002\t1f000002\t0299014d".into());
        assert_eq!(result, expected);
        assert_eq!(counts, json!({"training_placements_removed": 1, "shelter_mirrored": 1, "shelter_quest_placements": 1}));
        let (result, counts) = without_training_placements(&content, lines, false).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(counts["shelter_quest_placements"], 0);
        std::fs::remove_dir_all(content).unwrap();
    }
    #[test]
    fn readers_reject_overruns() {
        assert!(door_physical(&[1, 0, 0, 0]).is_err());
        let mut r = Reader::new(&[3, 0, b'a', b'b', b'c', 0]);
        assert!(r.string(1).is_err(), "aligned extent passes the end");
        let mut r = Reader::new(&[2, 0, b'a', b'b']);
        assert!(r.string(1).is_ok() && r.pos == 4);
    }
    /// The 7230 visual resolution, checked against the installed DATs when present (Legions-era
    /// expectations of the Python suite did not carry over; 7230 is the target data).
    #[test]
    fn installed_7230_templates_resolve_to_visuals() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dataset-7230");
        if !directory.join("portal.dat").exists() {
            eprintln!("skipped: no dataset-7230 beside the workspace");
            return;
        }
        let mut profile = ContentProfile::open(&directory).unwrap();
        assert_eq!(profile.visual(0x470007de), Some(0x1f00002d), "lifestone, as the prepared world places it");
    }
}
