//! Scripted quests as data, in the shape of ACEmulator's emote tables (`weenie_properties_emote`
//! and `weenie_properties_emote_action`): four tables of the quest content database (`db`,
//! checked in as `content/quests.sql`), read at start, so an edited row and a restart change a
//! quest. The columns below are each table's, in the order `parse_rows` takes them (the TSV
//! order a generator still writes, `db::import_tsv`).
//!
//! - `quest` (`quest-registry.tsv`): quest, journal_id, title_table, title_key, title, goal, credit,
//!   min_level, time_limit_seconds, max_solves, repeat_seconds, source. One row per quest: its
//!   journal entry, kill goal and rules. A run starts when SetStage leaves stage 0 (or a
//!   completed stage) for an underway stage: below `min_level`, before `repeat_seconds` have
//!   passed since the last solve, or after `max_solves` solves, the start is refused (rows branch
//!   first with InqLevel / InqQuestReady to say why). An underway run older than
//!   `time_limit_seconds` counts as stage 0 again (it can be started anew). The run's start and
//!   solve times are durable (`quests::Command::Started` / `Solved`).
//! - `quest_stage` (`quest-stages.tsv`): quest, stage, completed, journal_text, source. What the journal (and the
//!   chat line a stage change prints) says at each stage.
//! - `emote_set` (`emote-sets.tsv`): set, owner_entity, owner_iid, trigger, item, quest, stage_min, stage_max,
//!   range, probability, source. Who reacts to what: Use, Give, Kill, ItemUsedOn, Enter (a player
//!   coming within `range` of the world object `owner_iid`), and the QuestSuccess / QuestFailure
//!   branch targets of the Inq actions.
//! - `emote_action` (`emote-actions.tsv`): set, order, action, delay_ms, p1..p5, source. What a set does, in order.
//!
//! Rows of every family (`hand`, `vault`) share one namespace of quests and sets. A bad row stops
//! the load naming its table and key (or a TSV's file and line). `run` interprets one trigger for one
//! player and returns the outcome; the caller (`server.rs`, `store.rs`) commits it as one
//! durable record and sends its lines.
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::inventory::{Command as I, Inventory, Item, Location};
use crate::movement::Placement;
use crate::quests::{Book, Command as Q};

pub const FILES: [&str; 4] = ["quest-registry.tsv", "quest-stages.tsv", "emote-sets.tsv", "emote-actions.tsv"];
const HEADERS: [&str; 4] = [
    "quest\tjournal_id\ttitle_table\ttitle_key\ttitle\tgoal\tcredit\tmin_level\ttime_limit_seconds\tmax_solves\trepeat_seconds\tsource",
    "quest\tstage\tcompleted\tjournal_text\tsource",
    "set\towner_entity\towner_iid\ttrigger\titem\tquest\tstage_min\tstage_max\trange\tprobability\tsource",
    "set\torder\taction\tdelay_ms\tp1\tp2\tp3\tp4\tp5\tsource",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trigger { Use, Give, Kill, ItemUsedOn, Enter, QuestSuccess, QuestFailure, Crafted, EscortLost }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credit {
    Killer,
    /// The killer's fellows in the world are credited too (a fellowship's shared kill).
    Fellowship,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quest {
    pub journal_id: u32, pub title: (u32, u32), pub title_text: String, pub goal: u16, pub credit: Credit,
    /// The level a run needs to start (0: none).
    pub min_level: u16,
    /// How long a run may stay underway, in seconds.
    pub time_limit: Option<u64>,
    /// How often the quest may be solved.
    pub max_solves: Option<u16>,
    /// How long after a solve a new run may start, in seconds.
    pub repeat: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub id: String, pub owner_entity: Option<u32>, pub owner_iid: Option<u64>, pub trigger: Trigger,
    pub item: Option<u32>, pub quest: Option<u32>, pub stages: (u8, u8), pub range: f32, pub probability: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Tell { table: u32, key: u32 },
    Message(String),
    AwardXp(u64),
    /// `basis_points` (hundredths of a percent) of the experience the player's level spans,
    /// within min..max (max 0: no cap).
    AwardLevelProportionalXp { basis_points: u64, min: u64, max: u64 },
    AwardGold(u32),
    GiveItem { entity: u32, count: u32 },
    TakeItem { entity: u32 },
    SetStage { quest: u32, stage: u8 },
    IncrementQuest { quest: u32 },
    InqStage { quest: u32, min: u8, max: u8, success: Option<String>, failure: Option<String> },
    InqQuestCount { quest: u32, count: u16, success: Option<String>, failure: Option<String> },
    InqItem { entity: u32, success: Option<String>, failure: Option<String> },
    /// Whether the player's level reaches the quest's `min_level`.
    InqLevel { quest: u32, success: Option<String>, failure: Option<String> },
    /// Whether a new run of the quest may start now (its repeat delay and solve limit).
    InqQuestReady { quest: u32, success: Option<String>, failure: Option<String> },
    /// Whether the quest has been solved (a prerequisite of another).
    InqSolved { quest: u32, success: Option<String>, failure: Option<String> },
    MoveTo(Placement),
    /// Puts the `Effect` state on the player for `seconds` (a quest's curse or blessing).
    ApplyEffect { effect: u32, seconds: u64 },
    /// Takes the `Effect` state off the player (if on).
    RemoveEffect { effect: u32 },
    /// Gives one item drawn by weight (the run's roll): (entity, weight) pairs.
    RandomGive(Vec<(u32, u32)>),
    /// Counts one more on the quest's counter `counter` (several counters in one quest).
    IncrementCounter { quest: u32, counter: u8, cap: u16 },
    /// Whether counter `counter` of the quest reached `count`.
    InqCounter { quest: u32, counter: u8, count: u16, success: Option<String>, failure: Option<String> },
    /// The nearest creature of the entity starts following the player.
    StartEscort(u32),
    /// The player's escorts stop following.
    EndEscort,
    /// Sets the player's kingdom (1 Order, 2 Dominion, 3 Shadow; 0 leaves it).
    SetKingdom(u8),
    /// Whether the player is in `kingdom` (0: in any).
    InqKingdom { kingdom: u8, success: Option<String>, failure: Option<String> },
    /// Adds Kingdom Points, up to `max` (0: no cap).
    AwardKingdomPoints { points: u32, max: u32 },
    /// Whether the player has at least `points` Kingdom Points.
    InqKingdomPoints { points: u32, success: Option<String>, failure: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tables {
    quests: BTreeMap<u32, Quest>,
    keys: BTreeMap<String, u32>,
    stages: BTreeMap<(u32, u8), (bool, String)>,
    sets: Vec<Set>,
    actions: BTreeMap<String, Vec<Action>>,
}

/// What one trigger did for one player, to be committed as one record.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub set: String,
    pub range: f32,
    pub tells: Vec<(u32, u32)>,
    /// Pyrealium text lines (Message actions, then the journal line of each stage change), in order.
    pub lines: Vec<String>,
    pub quests: Vec<Q>,
    pub items: Vec<I>,
    pub changes: Vec<crate::progression::Command>,
    pub moveto: Option<Placement>,
    /// Items granted (entity DIDs), for a kill's loot path.
    pub given: Vec<u32>,
    /// A creature of this entity starts following the player (StartEscort).
    pub escort: Option<u32>,
    /// The player's escorts stop following (EndEscort).
    pub end_escort: bool,
}

/// The tables the server runs: set at start (`install`), swapped whole by a live edit (`replace`).
static INSTALLED: RwLock<Option<Arc<Tables>>> = RwLock::new(None);

fn rows<'a>(text: &'a str, name: &'a str, header: &str, columns: usize)
    -> io::Result<Vec<(usize, Vec<&'a str>)>> {
    let mut lines = text.lines().enumerate();
    let first = lines.next().map(|(_, l)| l.trim_start_matches('\u{feff}').trim_end_matches('\r'));
    if first != Some(header) {
        return Err(io::Error::other(format!("{name}:1: the header must be `{}`", header.replace('\t', " TAB "))));
    }
    let mut out = Vec::new();
    for (index, line) in lines {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') { continue; }
        let f: Vec<&str> = line.split('\t').map(str::trim).collect();
        if f.len() != columns {
            return Err(io::Error::other(format!("{name}:{}: {} columns, {columns} expected", index + 1, f.len())));
        }
        out.push((index + 1, f));
    }
    Ok(out)
}

fn hex32(v: &str) -> Option<u32> { u32::from_str_radix(v.trim_start_matches("0x").trim_start_matches("0X"), 16).ok() }
fn hex64(v: &str) -> Option<u64> { u64::from_str_radix(v.trim_start_matches("0x").trim_start_matches("0X"), 16).ok() }
fn opt(v: &str) -> Option<String> { (!v.is_empty()).then(|| v.to_owned()) }

/// A table row: where it came from (`file:line`, or the database row) and its fields in the
/// TSV column order.
pub type Row = (String, Vec<String>);

/// Every file's rows of one table, each named `file:line`.
fn all_rows(files: &[(String, String)], header: &str, columns: usize) -> io::Result<Vec<Row>> {
    let mut out = Vec::new();
    for (name, text) in files {
        out.extend(rows(text, name, header, columns)?.into_iter()
            .map(|(line, f)| (format!("{name}:{line}"), f.into_iter().map(str::to_owned).collect())));
    }
    Ok(out)
}

impl Tables {
    /// Parses the four tables (`texts` in `FILES` order; `names` for the errors).
    pub fn parse(texts: [&str; 4], names: [&str; 4]) -> io::Result<Self> {
        Self::parse_files(std::array::from_fn(|i| vec![(names[i].to_owned(), texts[i].to_owned())]))
    }

    /// Parses the four tables, each from one or more files `(name, text)` (`FILES` order).
    pub fn parse_files(files: [Vec<(String, String)>; 4]) -> io::Result<Self> {
        Self::parse_rows([
            all_rows(&files[0], HEADERS[0], 12)?, all_rows(&files[1], HEADERS[1], 5)?,
            all_rows(&files[2], HEADERS[2], 11)?, all_rows(&files[3], HEADERS[3], 10)?,
        ])
    }

    /// Checks and builds the tables from their rows (`FILES` order, TSV column order). Rows come
    /// from the TSVs (`parse_files`) or the content database (`db::load`); a bad row's error names
    /// where it came from.
    pub fn parse_rows(tables: [Vec<Row>; 4]) -> io::Result<Self> {
        let mut t = Tables::default();
        for (at, f) in &tables[0] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let id = hex32(f[1]).ok_or_else(|| bad("journal_id must be hex"))?;
            let title = (hex32(f[2]).ok_or_else(|| bad("title_table must be hex"))?, hex32(f[3]).ok_or_else(|| bad("title_key must be hex"))?);
            let goal = if f[5].is_empty() { 0 } else { f[5].parse().map_err(|_| bad("goal must be a whole number"))? };
            let credit = match f[6] { "killer" => Credit::Killer, "fellowship" => Credit::Fellowship,
                _ => return Err(bad("credit must be `killer` or `fellowship`")) };
            let min_level = if f[7].is_empty() { 0 } else { f[7].parse().map_err(|_| bad("min_level must be a whole number"))? };
            let positive = |v: &str, what: &str| if v.is_empty() { Ok(None) } else {
                v.parse::<u64>().ok().filter(|n| *n > 0).map(Some).ok_or_else(|| bad(&format!("{what} must be a positive whole number")))
            };
            let time_limit = positive(f[8], "time_limit_seconds")?;
            let max_solves = positive(f[9], "max_solves")?.map(|n| u16::try_from(n).map_err(|_| bad("max_solves is at most 65535"))).transpose()?;
            let repeat = positive(f[10], "repeat_seconds")?;
            if f[0].is_empty() || f[4].is_empty() || f[11].is_empty() { return Err(bad("quest, title and source must not be empty")); }
            let quest = Quest { journal_id: id, title, title_text: f[4].to_owned(), goal, credit, min_level, time_limit, max_solves, repeat };
            if t.keys.insert(f[0].to_owned(), id).is_some() || t.quests.insert(id, quest).is_some() {
                return Err(bad("duplicate quest"));
            }
        }
        for (at, f) in &tables[1] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let quest = *t.keys.get(f[0]).ok_or_else(|| bad("quest is not in the registry"))?;
            let stage: u8 = f[1].parse().map_err(|_| bad("stage must be 0 to 255"))?;
            let completed = match f[2] { "0" => false, "1" => true, _ => return Err(bad("completed must be 0 or 1")) };
            if f[3].is_empty() || f[4].is_empty() { return Err(bad("journal_text and source must not be empty")); }
            if t.stages.insert((quest, stage), (completed, f[3].to_owned())).is_some() { return Err(bad("duplicate stage")); }
        }
        let mut lines = BTreeMap::new();
        for (at, f) in &tables[2] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let trigger = match f[3] {
                "Use" => Trigger::Use, "Give" => Trigger::Give, "Kill" => Trigger::Kill, "ItemUsedOn" => Trigger::ItemUsedOn,
                "Enter" => Trigger::Enter, "QuestSuccess" => Trigger::QuestSuccess, "QuestFailure" => Trigger::QuestFailure,
                "Crafted" => Trigger::Crafted, "EscortLost" => Trigger::EscortLost,
                _ => return Err(bad("trigger must be Use, Give, Kill, ItemUsedOn, Enter, Crafted, EscortLost, QuestSuccess or QuestFailure")),
            };
            let owner_entity = if f[1].is_empty() { None } else { Some(hex32(f[1]).filter(|e| e >> 24 == 0x47).ok_or_else(|| bad("owner_entity must be an EntityDesc DID"))?) };
            let owner_iid = if f[2].is_empty() { None } else { Some(hex64(f[2]).ok_or_else(|| bad("owner_iid must be hex"))?) };
            let branch = matches!(trigger, Trigger::QuestSuccess | Trigger::QuestFailure);
            if matches!(trigger, Trigger::Crafted | Trigger::EscortLost) && (owner_entity.is_none() || owner_iid.is_some()) {
                return Err(bad("a Crafted or EscortLost set names an EntityDesc in owner_entity (no owner_iid)"));
            }
            if trigger == Trigger::Enter && f[2].is_empty() {
                return Err(bad("an Enter set names its point: owner_iid, the world object it is around (range: the radius)"));
            }
            if branch == (owner_entity.is_some() || owner_iid.is_some()) {
                return Err(bad("a branch set has no owner; every other set names owner_entity or owner_iid"));
            }
            let item = if f[4].is_empty() { None } else { Some(hex32(f[4]).ok_or_else(|| bad("item must be hex"))?) };
            let quest = if f[5].is_empty() { None } else { Some(*t.keys.get(f[5]).ok_or_else(|| bad("quest is not in the registry"))?) };
            let stage = |v: &str, d: u8| if v.is_empty() { Ok(d) } else { v.parse::<u8>().map_err(|_| bad("stages are 0 to 255")) };
            let stages = (stage(f[6], 0)?, stage(f[7], 255)?);
            if quest.is_none() && (f[6].len() + f[7].len()) > 0 { return Err(bad("a stage condition needs its quest")); }
            let range = if f[8].is_empty() { 4.0 } else { f[8].parse().ok().filter(|r: &f32| *r > 0.0).ok_or_else(|| bad("range must be positive metres"))? };
            let probability = if f[9].is_empty() { 1.0 } else { f[9].parse().ok().filter(|p: &f32| (0.0..=1.0).contains(p) && *p > 0.0).ok_or_else(|| bad("probability must be in (0, 1]"))? };
            if f[0].is_empty() || f[10].is_empty() { return Err(bad("set and source must not be empty")); }
            if lines.insert(f[0].to_owned(), at.clone()).is_some() { return Err(bad("duplicate set")); }
            t.sets.push(Set { id: f[0].to_owned(), owner_entity, owner_iid, trigger, item, quest, stages, range, probability });
        }
        let mut orders = BTreeMap::<String, Vec<(u32, String, Action)>>::new();
        for (at, f) in &tables[3] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            if !lines.contains_key(f[0]) { return Err(bad("set is not in emote-sets")); }
            let order: u32 = f[1].parse().map_err(|_| bad("order must be a whole number"))?;
            if !matches!(f[3], "" | "0") { return Err(bad("delay_ms is not scheduled yet; leave it 0")); }
            if f[9].is_empty() { return Err(bad("source must not be empty")); }
            let p = &f[4..9];
            let used = |n: usize| if p[n..].iter().all(|v| v.is_empty()) { Ok(()) } else { Err(bad(&format!("{} takes {n} parameters", f[2]))) };
            let num = |v: &str| v.parse::<u64>().map_err(|_| bad("expected a whole number"));
            let entity = |v: &str| hex32(v).filter(|e| e >> 24 == 0x47).ok_or_else(|| bad("expected an EntityDesc DID"));
            let quest = |v: &str| t.keys.get(v).copied().ok_or_else(|| bad("quest is not in the registry"));
            let set = |v: &str| if v.is_empty() { Ok(None) } else if lines.contains_key(v) { Ok(Some(v.to_owned())) } else { Err(bad("branch set is not in emote-sets")) };
            let stage = |v: &str| v.parse::<u8>().map_err(|_| bad("stages are 0 to 255"));
            let float = |v: &str| v.parse::<f32>().map_err(|_| bad("expected a number"));
            let action = match f[2] {
                "Tell" => { used(2)?; Action::Tell { table: hex32(p[0]).ok_or_else(|| bad("table must be hex"))?, key: hex32(p[1]).ok_or_else(|| bad("key must be hex"))? } }
                "Message" => { used(1)?; Action::Message(opt(p[0]).ok_or_else(|| bad("Message needs its text"))?) }
                "AwardXP" => { used(1)?; Action::AwardXp(num(p[0]).ok().filter(|x| *x > 0).ok_or_else(|| bad("xp must be positive"))?) }
                "AwardLevelProportionalXP" => {
                    used(3)?;
                    // The percent may have up to two decimals (the Vault's '2.5% XP').
                    let percent = p[0].parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= 100.0 && (v * 100.0).fract().abs() < 1e-9)
                        .ok_or_else(|| bad("percent must be in (0, 100] with at most two decimals"))?;
                    let (min, max) = (if p[1].is_empty() { 0 } else { num(p[1])? }, if p[2].is_empty() { 0 } else { num(p[2])? });
                    if max != 0 && max < min { return Err(bad("max must not be below min")); }
                    Action::AwardLevelProportionalXp { basis_points: (percent * 100.0).round() as u64, min, max }
                }
                "GiveItem" => { used(2)?; Action::GiveItem { entity: entity(p[0])?, count: num(p[1]).ok().filter(|c| (1..=16).contains(c)).ok_or_else(|| bad("count must be 1 to 16"))? as u32 } }
                "TakeItem" => { used(2)?; if p[1] != "1" { return Err(bad("TakeItem takes count 1")); } Action::TakeItem { entity: entity(p[0])? } }
                "SetStage" => { used(2)?; Action::SetStage { quest: quest(p[0])?, stage: stage(p[1])? } }
                "IncrementQuest" => { used(1)?; Action::IncrementQuest { quest: quest(p[0])? } }
                "InqStage" => Action::InqStage { quest: quest(p[0])?, min: stage(p[1])?, max: stage(p[2])?, success: set(p[3])?, failure: set(p[4])? },
                "InqQuestCount" => { used(4)?; Action::InqQuestCount { quest: quest(p[0])?, count: num(p[1])? as u16, success: set(p[2])?, failure: set(p[3])? } }
                "InqItem" => { used(3)?; Action::InqItem { entity: entity(p[0])?, success: set(p[1])?, failure: set(p[2])? } }
                "InqLevel" => { used(3)?; Action::InqLevel { quest: quest(p[0])?, success: set(p[1])?, failure: set(p[2])? } }
                "InqQuestReady" => { used(3)?; Action::InqQuestReady { quest: quest(p[0])?, success: set(p[1])?, failure: set(p[2])? } }
                "InqSolved" => { used(3)?; Action::InqSolved { quest: quest(p[0])?, success: set(p[1])?, failure: set(p[2])? } }
                "AwardGold" => { used(1)?; Action::AwardGold(num(p[0]).ok().filter(|g| (1..=u64::from(u32::MAX)).contains(g)).ok_or_else(|| bad("gold must be positive"))? as u32) }
                "ApplyEffect" => {
                    used(2)?;
                    let effect = hex32(p[0]).filter(|e| e >> 24 == 0x6f).ok_or_else(|| bad("effect must be an Effect state (WState 0x6f) DID"))?;
                    let seconds = num(p[1]).ok().filter(|s| (1..=31_536_000).contains(s)).ok_or_else(|| bad("seconds must be 1 to a year"))?;
                    Action::ApplyEffect { effect, seconds }
                }
                "RandomGive" => {
                    used(1)?;
                    let mut pairs = Vec::new();
                    for part in p[0].split(',') {
                        let (e, w) = part.split_once(':').ok_or_else(|| bad("RandomGive takes `0x47...:weight,...`"))?;
                        let w = w.trim().parse::<u32>().ok().filter(|w| *w > 0).ok_or_else(|| bad("weights are positive whole numbers"))?;
                        pairs.push((entity(e.trim())?, w));
                    }
                    if pairs.is_empty() { return Err(bad("RandomGive needs at least one item")); }
                    Action::RandomGive(pairs)
                }
                "IncrementCounter" => {
                    used(3)?;
                    Action::IncrementCounter { quest: quest(p[0])?, counter: stage(p[1])?,
                        cap: num(p[2]).ok().and_then(|c| u16::try_from(c).ok()).filter(|c| *c > 0).ok_or_else(|| bad("cap must be 1 to 65535"))? }
                }
                "InqCounter" => Action::InqCounter { quest: quest(p[0])?, counter: stage(p[1])?,
                    count: num(p[2]).ok().and_then(|c| u16::try_from(c).ok()).ok_or_else(|| bad("count must be 0 to 65535"))?,
                    success: set(p[3])?, failure: set(p[4])? },
                "StartEscort" => { used(1)?; Action::StartEscort(entity(p[0])?) }
                "EndEscort" => { used(0)?; Action::EndEscort }
                "SetKingdom" | "InqKingdom" => {
                    let k = p[0].parse::<u8>().ok().filter(|k| *k <= 3).ok_or_else(|| bad("kingdom is 0 to 3 (none, Order, Dominion, Shadow)"))?;
                    if f[2] == "SetKingdom" { used(1)?; Action::SetKingdom(k) } else { used(3)?; Action::InqKingdom { kingdom: k, success: set(p[1])?, failure: set(p[2])? } }
                }
                "AwardKingdomPoints" => {
                    used(2)?;
                    let points = num(p[0]).ok().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| bad("points must be positive"))?;
                    let max = if p[1].is_empty() { 0 } else { num(p[1]).ok().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad("max is a whole number"))? };
                    Action::AwardKingdomPoints { points, max }
                }
                "InqKingdomPoints" => { used(3)?; Action::InqKingdomPoints { points: num(p[0]).ok().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad("points is a whole number"))?, success: set(p[1])?, failure: set(p[2])? } }
                "RemoveEffect" => {
                    used(1)?;
                    Action::RemoveEffect { effect: hex32(p[0]).filter(|e| e >> 24 == 0x6f).ok_or_else(|| bad("effect must be an Effect state (WState 0x6f) DID"))? }
                }
                "MoveTo" => {
                    let placement = Placement { cell: hex32(p[0]).ok_or_else(|| bad("cell must be hex"))?,
                        coordinates: [float(p[1])?, float(p[2])?, float(p[3])?], heading: stage(p[4])? };
                    if !placement.valid() { return Err(bad("not a valid placement")); }
                    Action::MoveTo(placement)
                }
                other => return Err(bad(&format!("unknown action `{other}`"))),
            };
            orders.entry(f[0].to_owned()).or_default().push((order, at.clone(), action));
        }
        for (set, mut list) in orders {
            list.sort_by_key(|(order, _, _)| *order);
            if let Some(w) = list.windows(2).find(|w| w[0].0 == w[1].0) {
                return Err(io::Error::other(format!("{}: duplicate order in set {set}", w[1].1)));
            }
            t.actions.insert(set, list.into_iter().map(|(_, _, a)| a).collect());
        }
        Ok(t)
    }

    /// Every TakeItem's set and entity.
    pub fn takes(&self) -> impl Iterator<Item = (&str, u32)> {
        self.actions.iter().flat_map(|(set, list)| list.iter().filter_map(move |a| match a {
            Action::TakeItem { entity } => Some((set.as_str(), *entity)), _ => None }))
    }
    /// Every GiveItem's set and entity, for the start's item-profile check.
    pub fn gives(&self) -> impl Iterator<Item = (&str, u32)> {
        self.actions.iter().flat_map(|(set, list)| list.iter().flat_map(move |a| match a {
            Action::GiveItem { entity, .. } => vec![(set.as_str(), *entity)],
            Action::RandomGive(pairs) => pairs.iter().map(|(e, _)| (set.as_str(), *e)).collect(),
            _ => Vec::new() }))
    }
    pub fn quest(&self, key: &str) -> Option<&Quest> { self.keys.get(key).and_then(|id| self.quests.get(id)) }
    pub fn quests(&self) -> impl Iterator<Item = &Quest> { self.quests.values() }
    /// Every quest's name (`quest.name`).
    pub fn quest_names(&self) -> impl Iterator<Item = &str> { self.keys.keys().map(String::as_str) }
    /// The journal line and finished flag of `quest` at `stage`.
    pub fn stage(&self, quest: u32, stage: u8) -> Option<(bool, &str)> {
        self.stages.get(&(quest, stage)).map(|(c, t)| (*c, t.as_str()))
    }

    fn owned_by(set: &Set, entity: u32, iid: u64) -> bool {
        set.owner_iid.is_none_or(|i| i == iid) && set.owner_entity.is_none_or(|e| e == entity)
    }
    /// The world objects Enter sets are around, with their radius: a player's position within
    /// it runs the set (`server.rs`, on each position update).
    pub fn enter_points(&self) -> impl Iterator<Item = (u64, f32)> + '_ {
        self.sets.iter().filter(|s| s.trigger == Trigger::Enter).filter_map(|s| s.owner_iid.map(|i| (i, s.range)))
    }
    /// Whether the object (entity, iid) has sets for `trigger` at all.
    pub fn reacts(&self, entity: u32, iid: u64, trigger: Trigger) -> bool {
        self.sets.iter().any(|s| s.trigger == trigger && Self::owned_by(s, entity, iid))
    }

    /// The quest's stage for a player at `now` (Unix seconds): its recorded stage, or 0 when an
    /// underway run is older than the quest's time limit (it may be started anew).
    pub fn stage_now(&self, book: &Book, quest: u32, now: u64) -> u8 {
        let stage = book.stage(quest);
        let underway = stage != 0 && !self.stage(quest, stage).is_some_and(|(done, _)| done);
        let started = book.timer(quest).started;
        match self.quests.get(&quest).and_then(|q| q.time_limit) {
            Some(limit) if underway && started != 0 && now > started.saturating_add(limit) => 0,
            _ => stage,
        }
    }
    /// Whether a new run of `quest` may start at `now` (its solve limit and repeat delay).
    pub fn ready(&self, book: &Book, quest: u32, now: u64) -> bool {
        let Some(q) = self.quests.get(&quest) else { return false };
        let t = book.timer(quest);
        q.max_solves.is_none_or(|max| t.solves < max)
            && (t.solves == 0 || q.repeat.is_none_or(|r| now >= t.completed.saturating_add(r)))
    }
    /// Whether `quest` has been solved: a recorded solve, or a completed stage (runs recorded
    /// before solves were).
    pub fn solved(&self, book: &Book, quest: u32) -> bool {
        book.timer(quest).solves > 0 || self.stage(quest, book.stage(quest)).is_some_and(|(done, _)| done)
    }
    fn level_ok(&self, progression: &crate::progression::Progression, quest: u32) -> bool {
        self.quests.get(&quest).is_none_or(|q| progression.level().number >= q.min_level)
    }

    #[allow(clippy::too_many_arguments)]
    fn find(&self, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book, roll: f32, now: u64) -> Option<&Set> {
        self.sets.iter().find(|s| s.trigger == trigger && Self::owned_by(s, entity, iid)
            && s.item.is_none_or(|i| Some(i) == item)
            && s.quest.is_none_or(|q| (s.stages.0..=s.stages.1).contains(&self.stage_now(book, q, now)))
            && roll < s.probability)
    }
    /// Whether a set of the object (entity, iid) would answer `trigger` for this player at `now`
    /// (any probability): callers dispatch to `run` only then, so an object with no matching set
    /// keeps its other behaviour (a portal's route).
    pub fn answers(&self, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book, now: u64) -> bool {
        self.find(trigger, entity, iid, item, book, 0.0, now).is_some()
    }
    /// Runs `trigger` on the object (entity, iid) for a player with this state at `now` (Unix
    /// seconds). The first set in file order whose owner, item and stage condition match and whose
    /// probability passes `roll` (0 to 1) runs; `None` when no set matches.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&self, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book,
        inventory: &Inventory, progression: &crate::progression::Progression, first: u64, roll: f32, now: u64,
        make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Option<Outcome>> {
        let Some(set) = self.find(trigger, entity, iid, item, book, roll, now) else { return Ok(None) };
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now, roll,
            outcome: Outcome { set: set.id.clone(), range: set.range, ..Outcome::default() } };
        self.perform(&set.id, &mut state, make, 0)?;
        Ok(Some(state.outcome))
    }

    #[allow(clippy::too_many_arguments)]
    fn perform_named(&self, set: &str, range: f32, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Outcome> {
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now, roll: 0.0,
            outcome: Outcome { set: set.to_owned(), range, ..Outcome::default() } };
        self.perform(set, &mut state, make, 0)?;
        Ok(state.outcome)
    }
    /// Runs one set's actions directly (the replay of records made before a quest was data).
    pub fn perform_set(&self, set: &str, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Outcome> {
        if !self.actions.contains_key(set) { return Err(io::Error::other(format!("no emote set {set}"))); }
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now: 0, roll: 0.0,
            outcome: Outcome { set: set.to_owned(), range: 4.0, ..Outcome::default() } };
        self.perform(set, &mut state, make, 0)?;
        Ok(state.outcome)
    }
    fn perform(&self, set: &str, s: &mut State, make: &dyn Fn(u32, u64, u16) -> Option<Item>, depth: u8) -> io::Result<()> {
        // An NPC giving many quests falls from one offer to the next: up to 32 branches deep.
        if depth > 32 { return Err(io::Error::other("emote branches nest too deep")); }
        for action in self.actions.get(set).map_or(&[][..], Vec::as_slice) {
            let branch = |ok: bool, success: &Option<String>, failure: &Option<String>| if ok { success.clone() } else { failure.clone() };
            match action {
                Action::Tell { table, key } => s.outcome.tells.push((*table, *key)),
                Action::Message(text) => s.outcome.lines.push(text.clone()),
                Action::AwardXp(xp) => s.change(crate::progression::Command::AwardXp(*xp))?,
                Action::AwardGold(gold) => s.change(crate::progression::Command::AwardGold(*gold))?,
                Action::AwardLevelProportionalXp { basis_points, min, max } => {
                    let span = s.progression.level_span().unwrap_or(0);
                    let xp = (span.saturating_mul(*basis_points) / 10_000).max(*min);
                    let xp = if *max == 0 { xp } else { xp.min(*max) };
                    if xp > 0 { s.change(crate::progression::Command::AwardXp(xp))?; }
                }
                Action::GiveItem { entity, count } => for _ in 0..*count {
                    let slot = s.bag.free_slot().ok_or_else(|| io::Error::other("Make room in your bag for the quest's rewards."))?;
                    let item = make(*entity, s.next, slot).ok_or_else(|| io::Error::other(format!("No item profile for {entity:08x}")))?;
                    s.next = s.next.checked_add(1).ok_or_else(|| io::Error::other("Item identity exhausted"))?;
                    s.item(I::Grant(item))?;
                    s.outcome.given.push(*entity);
                },
                Action::TakeItem { entity } => {
                    let id = owned(&s.bag, *entity).ok_or_else(|| io::Error::other("You do not have the required quest item."))?;
                    s.item(I::ConsumeDense { item: id })?;
                }
                Action::SetStage { quest, stage } => {
                    let done = |st: u8| self.stage(*quest, st).is_some_and(|(d, _)| d);
                    let before = self.stage_now(&s.book, *quest, s.now);
                    let starts = *stage != 0 && !done(*stage) && (before == 0 || done(before));
                    if starts {
                        // The registry's rules hold whatever the rows checked first.
                        if !self.level_ok(&s.progression, *quest) {
                            return Err(io::Error::other("You are not experienced enough to begin this quest."));
                        }
                        if !self.ready(&s.book, *quest, s.now) {
                            return Err(io::Error::other("You cannot begin this quest again yet."));
                        }
                        s.quest(Q::Started { quest: *quest, at: s.now })?;
                    }
                    s.quest(Q::Stage { quest: *quest, stage: *stage })?;
                    if done(*stage) && !done(before) { s.quest(Q::Solved { quest: *quest, at: s.now })?; }
                    if let Some((_, text)) = self.stage(*quest, *stage) { s.outcome.lines.push(text.to_owned()); }
                }
                Action::IncrementQuest { quest } => {
                    let goal = self.quests.get(quest).map_or(0, |q| q.goal);
                    let count = s.book.count(*quest);
                    if count < goal { s.quest(Q::Count { quest: *quest, count: count + 1 })?; }
                }
                Action::InqStage { quest, min, max, success, failure } => {
                    if let Some(next) = branch((*min..=*max).contains(&self.stage_now(&s.book, *quest, s.now)), success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::InqQuestCount { quest, count, success, failure } => {
                    if let Some(next) = branch(s.book.count(*quest) >= *count, success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::InqItem { entity, success, failure } => {
                    if let Some(next) = branch(owned(&s.bag, *entity).is_some(), success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::InqLevel { quest, success, failure } => {
                    if let Some(next) = branch(self.level_ok(&s.progression, *quest), success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::InqQuestReady { quest, success, failure } => {
                    if let Some(next) = branch(self.ready(&s.book, *quest, s.now), success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::InqSolved { quest, success, failure } => {
                    if let Some(next) = branch(self.solved(&s.book, *quest), success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::MoveTo(placement) => s.outcome.moveto = Some(*placement),
                Action::RandomGive(pairs) => {
                    let total: u64 = pairs.iter().map(|(_, w)| u64::from(*w)).sum();
                    let pick = ((f64::from(s.roll.clamp(0.0, 0.999_999)) * total as f64) as u64).min(total - 1);
                    let mut acc = 0u64;
                    let entity = pairs.iter().find(|(_, w)| { acc += u64::from(*w); pick < acc }).map(|(e, _)| *e).unwrap_or(pairs[0].0);
                    let slot = s.bag.free_slot().ok_or_else(|| io::Error::other("Make room in your bag for the quest's rewards."))?;
                    let item = make(entity, s.next, slot).ok_or_else(|| io::Error::other(format!("No item profile for {entity:08x}")))?;
                    s.next = s.next.checked_add(1).ok_or_else(|| io::Error::other("Item identity exhausted"))?;
                    s.item(I::Grant(item))?;
                    s.outcome.given.push(entity);
                }
                Action::IncrementCounter { quest, counter, cap } => {
                    let count = s.book.subcount(*quest, *counter);
                    if count < *cap { s.quest(Q::SubCount { quest: *quest, counter: *counter, count: count + 1 })?; }
                }
                Action::InqCounter { quest, counter, count, success, failure } => {
                    if let Some(next) = branch(s.book.subcount(*quest, *counter) >= *count, success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::StartEscort(e) => s.outcome.escort = Some(*e),
                Action::EndEscort => s.outcome.end_escort = true,
                Action::SetKingdom(k) => { if s.book.kingdom().0 != *k { s.quest(Q::Kingdom(*k))?; } }
                Action::InqKingdom { kingdom, success, failure } => {
                    let mine = s.book.kingdom().0;
                    if let Some(next) = branch(mine != 0 && (*kingdom == 0 || mine == *kingdom), success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::AwardKingdomPoints { points, max } => {
                    let have = s.book.kingdom().1;
                    let next = if *max == 0 { have.saturating_add(*points) } else { have.saturating_add(*points).min((*max).max(have)) };
                    if next != have { s.quest(Q::KingdomPoints(next))?; }
                }
                Action::InqKingdomPoints { points, success, failure } => {
                    if let Some(next) = branch(s.book.kingdom().1 >= *points, success, failure) { self.perform(&next, s, make, depth + 1)?; }
                }
                Action::ApplyEffect { effect, seconds } => {
                    s.quest(Q::EffectOn { effect: *effect, at: s.now, until: s.now.saturating_add(*seconds) })?;
                }
                Action::RemoveEffect { effect } => {
                    if s.book.quest_effects(s.now).iter().any(|(e, _)| e == effect) { s.quest(Q::EffectOff { effect: *effect })?; }
                }
            }
        }
        Ok(())
    }
}

struct State { book: Book, bag: Inventory, progression: crate::progression::Progression, next: u64, now: u64, roll: f32, outcome: Outcome }
impl State {
    fn change(&mut self, change: crate::progression::Command) -> io::Result<()> {
        self.progression = self.progression.apply(&change).map_err(io::Error::other)?;
        self.outcome.changes.push(change);
        Ok(())
    }
    fn item(&mut self, command: I) -> io::Result<()> {
        self.bag = self.bag.apply(&command).map_err(io::Error::other)?;
        self.outcome.items.push(command);
        Ok(())
    }
    fn quest(&mut self, command: Q) -> io::Result<()> {
        self.book = self.book.apply(&command).map_err(io::Error::other)?;
        self.outcome.quests.push(command);
        Ok(())
    }
}

/// The family files of the base table `table` in `directory` (`quest-registry-vault.tsv` for
/// `quest-registry.tsv`), in name order.
pub fn families(directory: &Path, table: &str) -> io::Result<Vec<std::path::PathBuf>> {
    let prefix = format!("{}-", table.trim_end_matches(".tsv"));
    let mut out = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| io::Error::other(format!("{}: {e}", directory.display())))? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with(&prefix) && name.ends_with(".tsv") && path.is_file() { out.push(path); }
    }
    out.sort();
    Ok(out)
}

/// The first bag item of `entity`.
pub fn owned(bag: &Inventory, entity: u32) -> Option<u64> {
    bag.items().find(|i| i.definition == entity && matches!(i.location, Location::Bag(_))).map(|i| i.id)
}

/// The tables the server runs with, set once at start, before the journal replays.
pub fn install(tables: Tables) -> io::Result<()> {
    let mut installed = INSTALLED.write().unwrap_or_else(|e| e.into_inner());
    if installed.as_deref().is_some_and(|t| *t != tables) { return Err(io::Error::other("different quest tables are already installed")); }
    if installed.is_none() { *installed = Some(Arc::new(tables)); }
    Ok(())
}
/// A live edit's tables (`admin.rs`, checked on the admin thread): the next trigger runs them.
/// What a player has done stays in their quest book; a run of a quest the new tables drop is
/// only no longer answered.
pub fn replace(tables: Arc<Tables>) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(tables);
}
pub fn installed() -> io::Result<Arc<Tables>> {
    tables().ok_or_else(|| io::Error::other("the quest tables (quests.sql) are not loaded"))
}
/// The tables if loaded (journal views tolerate their absence).
pub fn tables() -> Option<Arc<Tables>> { INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone() }

/// Refuses tables the running server cannot take: an item a set grants that is not an entity,
/// or a retired hunt (`hunting_quests::retired_hunts`) whose replacing quest is missing.
pub fn check_runnable(tables: &Tables) -> io::Result<()> {
    if let Some((set, entity)) = tables.gives().find(|(_, e)| e >> 24 != 0x47) {
        return Err(io::Error::other(format!("emote set {set} gives {entity:08x}, which is not an entity")));
    }
    if let Some(id) = crate::hunting_quests::retired_hunts().iter().find(|id| !tables.quests().any(|q| q.journal_id == **id)) {
        return Err(io::Error::other(format!("retired hunt {id:08x} has no quest in the quest tables (the vault family of quests.sql)")));
    }
    Ok(())
}

/// A kill credited to its killer: the creature's Kill sets, for the stage, counter and experience
/// changes (`store.rs` commits them with the kill) and the items to drop (the encounter's loot).
/// `now` is the kill's time (Unix seconds; the replay passes the record's).
pub fn kill(book: &Book, inventory: &Inventory, progression: &crate::progression::Progression, entity: u32, iid: u64, now: u64)
    -> Option<Outcome> {
    let tables = tables()?;
    // Item grants are the encounter's loot transaction's; the identities here are placeholders.
    let first = inventory.high_water().max(crate::inventory::ITEM_NAMESPACE) + 1;
    tables.run(Trigger::Kill, entity, iid, None, book, inventory, progression, first, 0.0, now, &item).ok().flatten()
}

/// A kill credited to a fellow of the killer: the creature's Kill sets of the quests whose
/// registry credit is `fellowship` (AC2 Vault: "This is a fellowship quest").
pub fn fellow_kill(book: &Book, inventory: &Inventory, progression: &crate::progression::Progression, entity: u32, now: u64)
    -> Option<Outcome> {
    let tables = tables()?;
    let first = inventory.high_water().max(crate::inventory::ITEM_NAMESPACE) + 1;
    let set = tables.sets.iter().find(|s| s.trigger == Trigger::Kill && s.owner_entity == Some(entity)
        && s.quest.is_some_and(|q| tables.quests.get(&q).is_some_and(|r| r.credit == Credit::Fellowship)
            && (s.stages.0..=s.stages.1).contains(&tables.stage_now(book, q, now))))?;
    tables.perform_named(&set.id, set.range, book, inventory, progression, first, now, &item).ok()
}

/// The item profiles a quest may grant beyond the Shelter's pinned five: entity DID to its
/// visual description and equipment slots, read from the DATs at start (`install_items`).
static ITEMS: RwLock<BTreeMap<u32, (u32, u16)>> = RwLock::new(BTreeMap::new());

/// A quest item `definition` as a new bag item: the Shelter's pinned profiles, else a profile
/// `install_items` read. (Unit tests, which have no DATs, give any other entity a placeholder
/// visual.)
pub fn item(definition: u32, id: u64, slot: u16) -> Option<Item> {
    if let Some(item) = crate::shelter::item(definition, id, slot) { return Some(item); }
    let profile = ITEMS.read().unwrap_or_else(|e| e.into_inner()).get(&definition).copied();
    let (visual, equipment_mask) = match profile {
        Some(profile) => profile,
        #[cfg(test)]
        None if definition >> 24 == 0x47 => (TEST_VISUAL, 0),
        None => return None,
    };
    Some(Item { roll: None, id, definition, visual, quantity: 1, stack_limit: 1, equipment_mask, location: Location::Bag(slot) })
}
#[cfg(test)]
pub const TEST_VISUAL: u32 = 0x1f00_fffe;

/// Reads every item the tables grant from the DATs and checks it as the loot catalog checks its
/// items (`loot::Catalog::load_with_rules`): the class defaults give the bag metadata, the visual
/// exists, and the acquisition notice encodes. Returns each item's profile and the metadata to
/// register (`register_items`); the admin thread reads a live edit's this way on its own DATs.
pub fn read_items(tables: &Tables, dat: &mut crate::dat::Dat) -> io::Result<Vec<ItemProfile>> {
    let mut out: Vec<ItemProfile> = Vec::new();
    for (set, entity) in tables.gives() {
        if crate::shelter::item(entity, crate::inventory::ITEM_NAMESPACE + 1, 0).is_some() || out.iter().any(|p| p.entity == entity) { continue; }
        let bad = |why: &str| io::Error::other(format!("emote set {set} names item {entity:08x}: {why}"));
        // As the tradeskills' items (`tradeskills::item`): the class defaults' bag metadata, and
        // the visual its physical record names.
        dat.read(entity).map_err(|e| bad(&e.to_string()))?;
        let info = crate::inventory_request::Metadata::load(dat, entity).map_err(|e| bad(&e.to_string()))?;
        let visual = crate::tradeskills::visual_of(dat, info.physical).map_err(|e| bad(&e.to_string()))?;
        dat.read(visual).map_err(|e| bad(&e.to_string()))?;
        let mask = crate::apparel::recovered_slot(entity, visual).unwrap_or(info.equipment.map_or(0, |e| e.valid as u16 & 0x7ff));
        let probe = Item { roll: None, id: crate::inventory::ITEM_NAMESPACE + 1, definition: entity, visual, quantity: 1, stack_limit: 1,
            equipment_mask: mask, location: Location::Bag(0) };
        crate::item_services::acquired(7, &probe, info).map_err(|_| bad("its acquisition notice does not encode"))?;
        out.push(ItemProfile { entity, visual, mask, metadata: info });
    }
    Ok(out)
}

/// A quest item's profile read from the DATs (`read_items`).
#[derive(Clone, Copy, Debug)]
pub struct ItemProfile { pub entity: u32, pub visual: u32, pub mask: u16, pub metadata: crate::inventory_request::Metadata }

/// Makes the profiles grantable (`item`). Profiles are only added: an item a live edit no longer
/// grants keeps its profile, since players may hold it.
pub fn register_items(profiles: &[ItemProfile]) {
    let mut items = ITEMS.write().unwrap_or_else(|e| e.into_inner());
    for p in profiles { items.insert(p.entity, (p.visual, p.mask)); }
}

/// `read_items` then `register_items`, at start. Returns the metadata to register.
pub fn install_items(tables: &Tables, dat: &mut crate::dat::Dat)
    -> io::Result<Vec<(u32, crate::inventory_request::Metadata)>> {
    let profiles = read_items(tables, dat)?;
    register_items(&profiles);
    Ok(profiles.iter().map(|p| (p.entity, p.metadata)).collect())
}

/// The shipped tables (`content/`), installed, for unit tests.
#[cfg(test)]
pub fn shipped() -> Arc<Tables> {
    let tables = Tables::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")).unwrap();
    install(tables).unwrap();
    installed().unwrap()
}

pub mod db;

#[cfg(test)]
mod tests;

/// A Tell action's line: the StringInfo `table`/`key` as a chat line (display type 1).
pub fn tell(table: u32, key: u32) -> Result<Vec<u8>, crate::world::WorldError> {
    let mut args = crate::script::localized(table, key)?;
    args.extend(crate::script::integer(1));
    crate::script::private_call(crate::script::CHAT_STRING, &args)
}
