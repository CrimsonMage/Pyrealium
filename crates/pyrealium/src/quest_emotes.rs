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
//!   range, probability, source. Who reacts to what (`Trigger`): an object's Use, Give,
//!   ItemUsedOn or Enter, a creature's Kill, a Crafted item, a lost escort; the QuestSuccess /
//!   QuestFailure / Goto targets of other sets; and the player's own PlayerDeath, QuestExpired
//!   and Logout (`run_personal`; quest state and talk only).
//! - `emote_action` (`emote-actions.tsv`): set, order, action, delay_ms, p1..p5, source. What a set does, in
//!   order (`spec::ACTIONS`); a delay pauses the set (`Outcome::resume`, `pending`).
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
pub enum Trigger {
    Use, Give, Kill, ItemUsedOn, Enter, QuestSuccess, QuestFailure, Crafted, EscortLost,
    /// A set reached only by the Goto action (no owner).
    Goto,
    /// The player died (a set with a quest condition: a run failed by death).
    PlayerDeath,
    /// The player's run of the set's quest outlived its time limit (the server's tick).
    QuestExpired,
    /// The player left the world.
    Logout,
    /// An escort gave up on a player who stayed out of its radar too long (owner_entity: the
    /// escorted entity; a quest without one fails by its EscortLost sets).
    EscortAbandoned,
}

impl Trigger {
    pub const NAMES: [(&'static str, Trigger); 14] = [
        ("Use", Trigger::Use), ("Give", Trigger::Give), ("Kill", Trigger::Kill), ("ItemUsedOn", Trigger::ItemUsedOn),
        ("Enter", Trigger::Enter), ("Crafted", Trigger::Crafted), ("EscortLost", Trigger::EscortLost),
        ("QuestSuccess", Trigger::QuestSuccess), ("QuestFailure", Trigger::QuestFailure), ("Goto", Trigger::Goto),
        ("PlayerDeath", Trigger::PlayerDeath), ("QuestExpired", Trigger::QuestExpired), ("Logout", Trigger::Logout),
        ("EscortAbandoned", Trigger::EscortAbandoned),
    ];
    pub fn parse(name: &str) -> Option<Self> { Self::NAMES.iter().find(|(n, _)| *n == name).map(|(_, t)| *t) }
    /// A set with no owner: reached from another set (a branch or Goto).
    pub fn branch(self) -> bool { matches!(self, Trigger::QuestSuccess | Trigger::QuestFailure | Trigger::Goto) }
    /// A set about the player alone (no owner object; it names its quest).
    pub fn personal(self) -> bool { matches!(self, Trigger::PlayerDeath | Trigger::QuestExpired | Trigger::Logout) }
}

/// A player stat an InqStat row reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat { Level, Gold, Health, Vigor, Kingdom, KingdomPoints }
impl Stat {
    pub const NAMES: [&'static str; 6] = ["level", "gold", "health", "vigor", "kingdom", "kingdom_points"];
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name { "level" => Stat::Level, "gold" => Stat::Gold, "health" => Stat::Health, "vigor" => Stat::Vigor,
            "kingdom" => Stat::Kingdom, "kingdom_points" => Stat::KingdomPoints, _ => return None })
    }
}

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
    TakeItem { entity: u32, count: u16 },
    /// Resets a quest to stage 0 (counters and run start; solves stay).
    EraseQuest { quest: u32 },
    SetStage { quest: u32, stage: u8 },
    IncrementQuest { quest: u32 },
    InqStage { quest: u32, min: u8, max: u8, success: Option<String>, failure: Option<String> },
    InqQuestCount { quest: u32, count: u16, success: Option<String>, failure: Option<String> },
    /// Whether the bag holds at least `count` of the item (InqItem: one).
    InqOwnsItems { entity: u32, count: u16, success: Option<String>, failure: Option<String> },
    /// Whether the quest's solve count is in min..=max.
    InqQuestSolves { quest: u32, min: u16, max: Option<u16>, success: Option<String>, failure: Option<String> },
    /// Whether a player stat is in min..=max.
    InqStat { stat: Stat, min: u32, max: Option<u32>, success: Option<String>, failure: Option<String> },
    /// Runs another set, then carries on.
    Goto(String),
    DecrementCounter { quest: u32, counter: u8 },
    /// Takes a free lane of the group for the player (success), or none is free (failure); a
    /// player holding one already succeeds.
    ClaimInstance { group: String, success: Option<String>, failure: Option<String> },
    /// Frees the player's lane of the group, sending its slots' creatures away.
    ReleaseInstance(String),
    /// Sends away the creatures of the group's slots in the player's lane.
    DismissSlots(String),
    /// Summons the slot's creature in the player's lane of its group.
    Generate(String),
    /// Moves the player to the entry of its lane of the group.
    MoveToLane(String),
    SetCounter { quest: u32, counter: u8, value: u16 },
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
    /// Each set's actions in order, each with the delay before it (milliseconds).
    actions: BTreeMap<String, Vec<(u32, Action)>>,
    /// Instance groups and spawn slots, by name (`instances`).
    groups: BTreeMap<String, instances::Group>,
    slots: BTreeMap<String, instances::Slot>,
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
    /// The set paused for an action's delay: the caller runs `Tables::resume` after it.
    pub resume: Option<Resume>,
    /// Creatures summoned or sent away (`instances`; applied by the server's tick).
    pub world: Vec<instances::WorldOp>,
    /// Instance lanes taken and freed (group id, lane).
    pub claims: Vec<(u32, u8)>,
    pub releases: Vec<(u32, u8)>,
}

/// Where a set paused for a delay (ACE's per-action Delay): the sets to carry on from, innermost
/// first, each with the action to start at, and how long to wait. Nothing durable: a resume is
/// lost with a restart or a logout, as ACE's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resume { pub frames: Vec<(String, usize)>, pub after_ms: u32 }

/// The tables the server runs: set at start (`install`), swapped whole by a live edit (`replace`).
static INSTALLED: RwLock<Option<Arc<Tables>>> = RwLock::new(None);

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

/// The shipped tables (`content/`), installed, for unit tests.
#[cfg(test)]
pub fn shipped() -> Arc<Tables> {
    let tables = Tables::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")).unwrap();
    install(tables).unwrap();
    installed().unwrap()
}

pub mod db;
pub mod instances;
pub mod pending;
mod items;
mod load;
mod run;
pub mod spec;
mod templates;
pub use items::*;
pub use load::{Row, families};
pub use run::owned;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod template_tests;
#[cfg(test)]
mod instance_tests;

/// A Tell action's line: the StringInfo `table`/`key` as a chat line (display type 1).
pub fn tell(table: u32, key: u32) -> Result<Vec<u8>, crate::world::WorldError> {
    let mut args = crate::script::localized(table, key)?;
    args.extend(crate::script::integer(1));
    crate::script::private_call(crate::script::CHAT_STRING, &args)
}
