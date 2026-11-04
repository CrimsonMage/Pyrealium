//! Durable authored development quest state and native build-7148 journal data.
//!
//! Native layout: gmQuestInfo::StreamPack 0060af00 and PList<gmQuestInfo>::Pack
//! 0060b500. Status semantics: gmQuestInfo::InqExtraText 0060aca0. The training
//! objective/rewards are explicit development policy, not original quest rules.
//! Later captures carry an extra maxed-out word absent from 7148; it is omitted.
use crate::{dat::Dat, script, world, world::WorldError};
use std::io;

pub const UPDATE: u32 = 0x82130024;
const ICON: u32 = 0x4100034b;
const UNDERWAY: u32 = 0x10000000;
const COMPLETED: u32 = 0x30000000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    pub killed: u8,
    pub claimed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Book {
    /// A legacy inferred quest's saved run, until the start moves it (`legacy_quests`).
    pub recovered: crate::legacy_quests::Runs,
    /// A legacy obelisk search's saved run, until the start moves it (`legacy_quests`).
    pub exploration: crate::legacy_quests::Searches,
    /// A legacy hunt's saved run, until the start moves it onto its data quest (`legacy_quests`).
    pub hunting: crate::legacy_quests::Hunts,
    /// The legacy Arwic lesson's saved state, until the start moves it (`legacy_quests`).
    pub arwic: crate::legacy_quests::Lesson,
    /// The training lessons' legacy state, until the start moves it onto their rows
    /// (`legacy_quests`): Training Grounds, Field Supplies (accepted / claimed), Potion Practice
    /// (0 absent, 1 accepted, 2 crafted, 3 rewarded) and A Safe Journey (0 absent, 1 accepted, 2
    /// bound, 3 travelled, 4 rewarded).
    pub training: Option<Progress>,
    pub supplies: Option<bool>,
    pub potion: u8,
    pub journey: u8,
    /// Each data-driven quest's stage (`quest_emotes`), by journal id; Rescue the Shelter Hold's
    /// is also the legacy Shelter phase the `Shelter*` records advanced.
    stages: std::collections::BTreeMap<u32, u8>,
    /// Each data-driven quest's counter (kill tasks), by journal id.
    counts: std::collections::BTreeMap<u32, u16>,
    /// A data-driven quest's further counters (several kill tasks at once), by journal id and
    /// counter number.
    #[serde(with = "crate::db::pairs")]
    subcounts: std::collections::BTreeMap<(u32, u8), u16>,
    /// The character's kingdom (1 Order, 2 Dominion, 3 Shadow; 0 none) and Kingdom Points, set by
    /// the quest tables (`quest_emotes` SetKingdom, AwardKingdomPoints).
    kingdom: u8,
    kingdom_points: u32,
    /// Quest Points: the 2002-2003 quest rewards the AC2 Vault records (AwardQuestPoints). No
    /// client in hand shows them; the server keeps them for later use.
    quest_points: u32,
    /// The quest tables' timed effects on the character (`quest_emotes` ApplyEffect): the
    /// `Effect` state, when it was put on and when it runs out (Unix seconds).
    effects: std::collections::BTreeMap<u32, (u64, u64)>,
    /// Each data-driven quest's times (`quest_emotes`), by journal id: when its current run
    /// started, when it was last solved and how often (`hunting_quests`' State precedent).
    timers: std::collections::BTreeMap<u32, Timer>,
    /// Story scenes the character has earned (`crate::story`), by scene id.
    scenes: std::collections::BTreeSet<u32>,
    /// The instance lanes the character holds (`quest_emotes` ClaimInstance), by group id.
    #[serde(default)]
    lanes: std::collections::BTreeMap<u32, u8>,
    /// Recipes the quests taught the character (`quest_emotes` LearnRecipe).
    #[serde(default)]
    recipes: std::collections::BTreeSet<u32>,
}

/// A data-driven quest's times, in Unix seconds (0: never).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Timer { pub started: u64, pub completed: u64, pub solves: u16 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// A story scene earned (`crate::story`): the player's act registry gains it.
    Scene(u32),
    ShelterGuard,
    ShelterLieutenant,
    ShelterSentry,
    ShelterTool, ShelterGlyph, ShelterAdvance, ShelterComplete,
    /// The glyph used on the retail Menhir Portal of human Shelter copy `block` (`shelter`).
    ShelterMenhir(u16),
    /// A data-driven quest (`quest_emotes`) set to a stage.
    Stage { quest: u32, stage: u8 },
    /// A data-driven quest's counter set to a value.
    Count { quest: u32, count: u16 },
    /// A data-driven quest's run started at `at` (Unix seconds); its counter starts again.
    Started { quest: u32, at: u64 },
    /// A data-driven quest solved at `at` (Unix seconds).
    Solved { quest: u32, at: u64 },
    /// A data-driven quest reset to stage 0: its counters and run start go, its solves stay
    /// (`quest_emotes` EraseQuest).
    Erase { quest: u32 },
    /// The character learned recipe `0` (`quest_emotes` LearnRecipe).
    Recipe(u32),
    /// The character holds lane `lane` of instance group `group` (`quest_emotes` ClaimInstance;
    /// the group's id is `quest_emotes::group_id` of its name).
    Lane { group: u32, lane: u8 },
    /// The character leaves its lane of instance group `group` (ReleaseInstance).
    LaneOff { group: u32 },
    /// A legacy hunt's state leaves the hunting book (`Store::migrate_legacy_quests`).
    RetireHunt { quest: u32 },
    /// A legacy inferred quest's state leaves its book (`Store::migrate_legacy_quests`).
    RetireRun { quest: u32 },
    /// A legacy obelisk search's state leaves its book.
    RetireSearch { quest: u32 },
    /// The legacy Arwic lesson's state leaves the book.
    RetireLesson,
    /// The training lessons' legacy state leaves the book.
    RetireTraining,
    /// A quest effect (an `Effect` state) put on at `at` until `until` (Unix seconds).
    EffectOn { effect: u32, at: u64, until: u64 },
    /// A quest effect taken off.
    EffectOff { effect: u32 },
    /// Counter `counter` of a data-driven quest set to a value.
    SubCount { quest: u32, counter: u8, count: u16 },
    /// The character's kingdom set (0 none, 1 Order, 2 Dominion, 3 Shadow).
    Kingdom(u8),
    /// The character's Kingdom Points set to a value.
    KingdomPoints(u32),
    /// The character's Quest Points set to a value.
    QuestPoints(u32),
}
/// Rescue the Shelter Hold's journal id (`quest-registry.tsv`), which the legacy `Shelter*`
/// records advanced before the quest became data.
const LEGACY_SHELTER: u32 = 0x4000ff05;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestError {
    InvalidRecord,
    AlreadyAccepted,
    AlreadyCompleted,
    NotAccepted,
    NotReady,
}
impl std::fmt::Display for QuestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}
impl std::error::Error for QuestError {}
impl QuestError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::AlreadyAccepted => "You already accepted the training quest.",
            Self::AlreadyCompleted => "You already claimed this quest's reward.",
            Self::NotAccepted => "Accept the training quest first.",
            Self::NotReady => "Finish the current training objective before claiming its reward.",
            Self::InvalidRecord => "Invalid development quest state.",
        }
    }
}

impl Command {
    pub fn encode(&self) -> String {
        match self {
            Self::Scene(scene) => format!("scene,{scene:08x}"),
            Self::ShelterTool => "shelter.tool".into(),
            Self::ShelterGlyph => "shelter.glyph".into(),
            Self::ShelterMenhir(block) => format!("shelter.menhir,{block:04x}"),
            Self::Stage { quest, stage } => format!("stage,{quest:08x},{stage}"),
            Self::Count { quest, count } => format!("count,{quest:08x},{count}"),
            Self::Started { quest, at } => format!("started,{quest:08x},{at}"),
            Self::Solved { quest, at } => format!("solved,{quest:08x},{at}"),
            Self::Erase { quest } => format!("erase,{quest:08x}"),
            Self::Lane { group, lane } => format!("lane,{group:08x},{lane}"),
            Self::Recipe(r) => format!("recipe,{r:08x}"),
            Self::LaneOff { group } => format!("laneoff,{group:08x}"),
            Self::RetireHunt { quest } => format!("retire,{quest:08x}"),
            Self::RetireRun { quest } => format!("retirerun,{quest:08x}"),
            Self::RetireSearch { quest } => format!("retiresearch,{quest:08x}"),
            Self::RetireLesson => "retirelesson".into(),
            Self::RetireTraining => "retiretraining".into(),
            Self::EffectOn { effect, at, until } => format!("effect,{effect:08x},{at},{until}"),
            Self::EffectOff { effect } => format!("effectoff,{effect:08x}"),
            Self::SubCount { quest, counter, count } => format!("subcount,{quest:08x},{counter},{count}"),
            Self::Kingdom(k) => format!("kingdom,{k}"),
            Self::KingdomPoints(p) => format!("kp,{p}"),
            Self::QuestPoints(p) => format!("qp,{p}"),
            Self::ShelterAdvance => "shelter.advance".into(),
            Self::ShelterComplete => "shelter.complete".into(),
            Self::ShelterGuard => "shelter.guard".into(),
            Self::ShelterLieutenant => "shelter.lieutenant".into(),
            Self::ShelterSentry => "shelter.sentry".into(),
        }
    }
    pub fn decode(text: &str) -> Result<Self, QuestError> {
        match text {
            "shelter.tool" => Ok(Self::ShelterTool),
            "shelter.glyph" => Ok(Self::ShelterGlyph),
            "shelter.advance" => Ok(Self::ShelterAdvance),
            "shelter.complete" => Ok(Self::ShelterComplete),
            "shelter.guard" => Ok(Self::ShelterGuard),
            "shelter.lieutenant" => Ok(Self::ShelterLieutenant),
            "shelter.sentry" => Ok(Self::ShelterSentry),
            _ if text.starts_with("kingdom,") => {
                let k: u8 = text["kingdom,".len()..].parse().map_err(|_| QuestError::InvalidRecord)?;
                if k > 3 { return Err(QuestError::InvalidRecord); }
                Ok(Self::Kingdom(k))
            }
            _ if text.starts_with("kp,") => Ok(Self::KingdomPoints(text[3..].parse().map_err(|_| QuestError::InvalidRecord)?)),
            _ if text.starts_with("qp,") => Ok(Self::QuestPoints(text[3..].parse().map_err(|_| QuestError::InvalidRecord)?)),
            _ if text.starts_with("subcount,") => {
                let f: Vec<&str> = text.split(',').collect();
                if f.len() != 4 || f[1].len() != 8 { return Err(QuestError::InvalidRecord); }
                Ok(Self::SubCount { quest: u32::from_str_radix(f[1], 16).map_err(|_| QuestError::InvalidRecord)?,
                    counter: f[2].parse().map_err(|_| QuestError::InvalidRecord)?, count: f[3].parse().map_err(|_| QuestError::InvalidRecord)? })
            }
            _ if text.starts_with("effect,") || text.starts_with("effectoff,") => {
                let mut f = text.split(',');
                let on = f.next() == Some("effect");
                let effect = f.next().filter(|h| h.len() == 8).and_then(|h| u32::from_str_radix(h, 16).ok())
                    .filter(|e| e >> 24 == 0x6f).ok_or(QuestError::InvalidRecord)?;
                if !on { return if f.next().is_none() { Ok(Self::EffectOff { effect }) } else { Err(QuestError::InvalidRecord) }; }
                let at = f.next().and_then(|v| v.parse::<u64>().ok()).ok_or(QuestError::InvalidRecord)?;
                let until = f.next().and_then(|v| v.parse::<u64>().ok()).filter(|u| *u > at).ok_or(QuestError::InvalidRecord)?;
                if f.next().is_some() { return Err(QuestError::InvalidRecord); }
                Ok(Self::EffectOn { effect, at, until })
            }
            _ if text.starts_with("recipe,") => {
                let hex = &text["recipe,".len()..];
                if hex.len() != 8 { return Err(QuestError::InvalidRecord); }
                Ok(Self::Recipe(u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)?))
            }
            _ if text.starts_with("lane,") || text.starts_with("laneoff,") => {
                let mut f = text.split(',');
                let on = f.next() == Some("lane");
                let group = f.next().filter(|h| h.len() == 8).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or(QuestError::InvalidRecord)?;
                let lane = if on { Some(f.next().and_then(|v| v.parse::<u8>().ok()).ok_or(QuestError::InvalidRecord)?) } else { None };
                if f.next().is_some() { return Err(QuestError::InvalidRecord); }
                Ok(match lane { Some(lane) => Self::Lane { group, lane }, None => Self::LaneOff { group } })
            }
            _ if text.starts_with("erase,") => {
                let hex = &text["erase,".len()..];
                if hex.len() != 8 { return Err(QuestError::InvalidRecord); }
                Ok(Self::Erase { quest: u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)? })
            }
            "retirelesson" => Ok(Self::RetireLesson),
            "retiretraining" => Ok(Self::RetireTraining),
            _ if text.starts_with("retiresearch,") => {
                let hex = &text["retiresearch,".len()..];
                if hex.len() != 8 { return Err(QuestError::InvalidRecord); }
                Ok(Self::RetireSearch { quest: u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)? })
            }
            _ if text.starts_with("retirerun,") => {
                let hex = &text["retirerun,".len()..];
                if hex.len() != 8 { return Err(QuestError::InvalidRecord); }
                Ok(Self::RetireRun { quest: u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)? })
            }
            _ if text.starts_with("retire,") => {
                let hex = &text["retire,".len()..];
                if hex.len() != 8 { return Err(QuestError::InvalidRecord); }
                let quest = u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)?;
                Ok(Self::RetireHunt { quest })
            }
            _ if text.starts_with("started,") || text.starts_with("solved,") => {
                let mut f = text.split(',').skip(1);
                let quest = f.next().filter(|h| h.len() == 8).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or(QuestError::InvalidRecord)?;
                let at = f.next().and_then(|v| v.parse::<u64>().ok()).ok_or(QuestError::InvalidRecord)?;
                if f.next().is_some() { return Err(QuestError::InvalidRecord); }
                Ok(if text.starts_with("started,") { Self::Started { quest, at } } else { Self::Solved { quest, at } })
            }
            _ if text.starts_with("stage,") || text.starts_with("count,") => {
                let mut f = text.split(',').skip(1);
                let quest = f.next().filter(|h| h.len() == 8).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or(QuestError::InvalidRecord)?;
                let value = f.next().and_then(|v| v.parse::<u16>().ok()).ok_or(QuestError::InvalidRecord)?;
                if f.next().is_some() { return Err(QuestError::InvalidRecord); }
                if text.starts_with("stage,") {
                    Ok(Self::Stage { quest, stage: u8::try_from(value).map_err(|_| QuestError::InvalidRecord)? })
                } else { Ok(Self::Count { quest, count: value }) }
            }
            _ if text.starts_with("shelter.menhir,") => {
                let hex = &text["shelter.menhir,".len()..];
                let block = (hex.len() == 4 && hex.bytes().all(|value| value.is_ascii_hexdigit()))
                    .then(|| u16::from_str_radix(hex, 16).ok()).flatten().ok_or(QuestError::InvalidRecord)?;
                if ![0x028d, 0x0298, 0x0299].contains(&block) { return Err(QuestError::InvalidRecord); }
                Ok(Self::ShelterMenhir(block))
            }
            _ if text.starts_with("scene,") => {
                let hex = &text["scene,".len()..];
                if hex.len() != 8 || !hex.bytes().all(|value| value.is_ascii_hexdigit()) {
                    return Err(QuestError::InvalidRecord);
                }
                Ok(Self::Scene(u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)?))
            }
            _ => Err(QuestError::InvalidRecord),
        }
    }
}

impl Book {
    pub fn shelter_phase(&self) -> u8 { self.stage(LEGACY_SHELTER) }
    /// A data-driven quest's stage (0 before it starts).
    pub fn stage(&self, quest: u32) -> u8 { self.stages.get(&quest).copied().unwrap_or(0) }
    /// A data-driven quest's counter.
    pub fn count(&self, quest: u32) -> u16 { self.counts.get(&quest).copied().unwrap_or(0) }
    /// The character's kingdom (0 none) and Kingdom Points.
    pub fn kingdom(&self) -> (u8, u32) { (self.kingdom, self.kingdom_points) }
    /// The character's Quest Points.
    pub fn quest_points(&self) -> u32 { self.quest_points }
    /// A data-driven quest's further counter `counter`.
    pub fn subcount(&self, quest: u32, counter: u8) -> u16 { self.subcounts.get(&(quest, counter)).copied().unwrap_or(0) }
    /// The quest effects still running at `now` (Unix seconds): each `Effect` state and its end.
    pub fn quest_effects(&self, now: u64) -> Vec<(u32, u64)> {
        self.effects.iter().filter(|(_, (_, until))| *until > now).map(|(e, (_, until))| (*e, *until)).collect()
    }
    /// A data-driven quest's times (all zero for a quest never started under them).
    /// Whether a quest taught the character `recipe`.
    pub fn knows(&self, recipe: u32) -> bool { self.recipes.contains(&recipe) }
    /// The lane of instance group `group` the character holds.
    pub fn lane(&self, group: u32) -> Option<u8> { self.lanes.get(&group).copied() }
    pub fn lanes(&self) -> impl Iterator<Item = (u32, u8)> + '_ { self.lanes.iter().map(|(g, l)| (*g, *l)) }
    pub fn timer(&self, quest: u32) -> Timer { self.timers.get(&quest).copied().unwrap_or_default() }
    /// The story scenes earned, in id order.
    pub fn scenes(&self) -> impl Iterator<Item = u32> + '_ { self.scenes.iter().copied() }
    /// Shared player guidance for mentor speech, quest commands and the journal.
    /// The quest state and immutable rewards remain the persisted authored profile.
    pub fn apply(&self, command: &Command) -> Result<Self, QuestError> {
        let mut next = self.clone();
        match command {
            Command::Scene(scene) => {
                if !crate::story::is_scene(*scene) { return Err(QuestError::InvalidRecord); }
                next.scenes.insert(*scene);
            }
            Command::ShelterTool | Command::ShelterGlyph | Command::ShelterMenhir(_) | Command::ShelterAdvance | Command::ShelterComplete => {
                let previous=match command {Command::ShelterTool=>3,Command::ShelterGlyph|Command::ShelterMenhir(_)=>4,Command::ShelterAdvance=>5,_=>6};
                if self.shelter_phase()!=previous {return Err(QuestError::NotReady);}
                next.stages.insert(LEGACY_SHELTER,previous+1);
            }
            Command::Stage { quest, stage } => { next.stages.insert(*quest, *stage); }
            Command::Count { quest, count } => { next.counts.insert(*quest, *count); }
            Command::Started { quest, at } => {
                next.timers.entry(*quest).or_default().started = *at;
                next.counts.remove(quest);
                next.subcounts.retain(|(q, _), _| q != quest);
            }
            Command::RetireHunt { quest } => { next.hunting.0.remove(quest); }
            Command::RetireRun { quest } => { next.recovered.0.remove(quest); }
            Command::RetireSearch { quest } => { next.exploration.0.remove(quest); }
            Command::RetireLesson => { next.arwic = crate::legacy_quests::Lesson::default(); }
            Command::RetireTraining => { next.training = None; next.supplies = None; next.potion = 0; next.journey = 0; }
            Command::Lane { group, lane } => { next.lanes.insert(*group, *lane); }
            Command::Recipe(r) => { next.recipes.insert(*r); }
            Command::LaneOff { group } => { next.lanes.remove(group); }
            Command::Erase { quest } => {
                next.stages.remove(quest);
                next.counts.remove(quest);
                next.subcounts.retain(|(q, _), _| q != quest);
                if let Some(t) = next.timers.get_mut(quest) { t.started = 0; }
            }
            Command::EffectOn { effect, at, until } => { next.effects.insert(*effect, (*at, *until)); }
            Command::EffectOff { effect } => { next.effects.remove(effect); }
            Command::SubCount { quest, counter, count } => { next.subcounts.insert((*quest, *counter), *count); }
            Command::Kingdom(k) => { next.kingdom = *k; }
            Command::KingdomPoints(p) => { next.kingdom_points = *p; }
            Command::QuestPoints(p) => { next.quest_points = *p; }
            Command::Solved { quest, at } => {
                let t = next.timers.entry(*quest).or_default();
                t.completed = *at;
                t.solves = t.solves.saturating_add(1);
            }
            Command::ShelterGuard | Command::ShelterLieutenant | Command::ShelterSentry => {
                let previous = match command {Command::ShelterGuard=>0,Command::ShelterLieutenant=>1,_=>2};
                if self.shelter_phase() != previous {return Err(QuestError::NotReady);}
                next.stages.insert(LEGACY_SHELTER,previous+1);
            }
        }
        Ok(next)
    }
    /// One live journal change. Never call for an unaccepted quest.
    /// The gmQuestInfoList argument used by UI_Load for saved journal restoration.
    pub fn list_graph(&self) -> Result<Vec<u8>, WorldError> {
        let mut payload = Vec::new();
        word(&mut payload, self.journal_stages().count() as u32);
        for (quest, stage) in self.journal_stages() { payload.extend(stage_payload(quest, stage, self.timer(quest), self.count(quest), false)?); }
        Ok(native_graph(0xa1, &payload))
    }
}

pub fn validate_assets(portal: &mut Dat) -> io::Result<()> {
    let icon = portal.read(ICON)?;
    if icon.get(..4) != Some(ICON.to_le_bytes().as_slice()) {
        return Err(io::Error::other(
            "quest icon has an incompatible DAT identity",
        ));
    }
    Ok(())
}

fn word(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend(value.to_le_bytes());
}
fn native_graph(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [5, 1, 1, 0x44000100, 0, 0xffff0000 | u32::from(kind)] {
        word(&mut bytes, value);
    }
    bytes.extend(payload);
    word(&mut bytes, 0); // Native object has no interpreted heap fields.
    bytes
}

impl Book {
    pub fn updates(&self) -> Result<Vec<Vec<u8>>, WorldError> {
        self.updates_since(&Self::default())
    }
    /// Native UpdateQuestInfo replaces one entry. Resending completed entries
    /// on every kill grows traffic with journal history and can fill the GUN
    /// reliable queue. Login still sends the complete list_graph snapshot.
    pub fn updates_since(&self, previous:&Self) -> Result<Vec<Vec<u8>>, WorldError> {
        let mut events = Vec::new();
        // A stage change, or a kill counted on a stage whose line shows the count.
        for (quest, stage) in self.journal_stages().filter(|(q, s)| previous.stage(*q) != *s || previous.count(*q) != self.count(*q)) {
            let mut args=script::integer(0);
            args.extend(native_graph(0xa0,&stage_payload(quest,stage,self.timer(quest),self.count(quest),true)?));
            args.extend(script::integer(if stage_completed(quest,stage) {COMPLETED} else {UNDERWAY}));
            events.push(script::private_call(UPDATE,&args)?);
        }
        Ok(events)
    }
}

impl Book {
    /// The data-driven quests the journal lists: those past stage 0 that the registry knows.
    fn journal_stages(&self) -> impl Iterator<Item = (u32, u8)> + '_ {
        let tables = crate::quest_emotes::tables();
        self.stages.iter().filter(move |(q, s)| **s != 0 && tables.as_ref().is_some_and(|t| t.quests().any(|r| r.journal_id == **q)))
            .map(|(q, s)| (*q, *s))
    }
}




fn stage_completed(quest: u32, stage: u8) -> bool {
    crate::quest_emotes::tables().is_some_and(|t| t.stage(quest, stage).is_some_and(|(done, _)| done))
}
/// A data-driven quest's journal entry: the registry's title and level, the stage's line and the
/// run's start and solve times.
fn stage_payload(quest: u32, stage: u8, timer: Timer, count: u16, play_fx: bool) -> Result<Vec<u8>, WorldError> {
    let tables = crate::quest_emotes::tables().ok_or(WorldError::InvalidValue)?;
    let registry = tables.quests().find(|q| q.journal_id == quest).ok_or(WorldError::InvalidValue)?;
    let title = &registry.title_text;
    let (done, text) = tables.stage(quest, stage).ok_or(WorldError::InvalidValue)?;
    // The journal line may show the kill counter: `{count}` and `{goal}` (the registry's goal).
    let text = &text.replace("{count}", &count.to_string()).replace("{goal}", &registry.goal.to_string());
    let mut bytes=Vec::new(); word(&mut bytes,quest);
    bytes.extend(world::literal_name(title)?);
    bytes.extend(world::literal_name(text)?);
    word(&mut bytes,ICON); word(&mut bytes,if done {COMPLETED} else {UNDERWAY}); word(&mut bytes,1);
    bytes.extend(world::literal_name(text)?);
    let time = |t: u64| if t == 0 { -1.0f64 } else { t as f64 };
    word(&mut bytes,u32::from(registry.min_level.max(1))); bytes.extend(time(timer.started).to_le_bytes());
    bytes.extend((if done { time(timer.completed) } else { -1.0 }).to_le_bytes());
    // gmQuestInfo::StreamPack 0060af00: _bExpires, _ttSecondsRemaining, _ttSecondsUntilRetry (the
    // registry's time limit for an underway run, its repeat delay after a solve).
    let now = crate::clock::now();
    let remaining = registry.time_limit.filter(|_| !done && timer.started != 0).map(|l| timer.started.saturating_add(l).saturating_sub(now));
    let retry = registry.repeat.filter(|_| done && timer.completed != 0).map_or(0, |r| timer.completed.saturating_add(r).saturating_sub(now));
    word(&mut bytes,u32::from(remaining.is_some())); bytes.extend((remaining.unwrap_or(0) as f64).to_le_bytes());
    bytes.extend((retry as f64).to_le_bytes());
    word(&mut bytes,u32::from(play_fx)); Ok(bytes)
}
