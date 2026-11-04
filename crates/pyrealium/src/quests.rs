//! Durable authored development quest state and native build-7148 journal data.
//!
//! Native layout: gmQuestInfo::StreamPack 0060af00 and PList<gmQuestInfo>::Pack
//! 0060b500. Status semantics: gmQuestInfo::InqExtraText 0060aca0. The training
//! objective/rewards are explicit development policy, not original quest rules.
//! Later captures carry an extra maxed-out word absent from 7148; it is omitted.
use crate::{dat::Dat, script, world, world::WorldError};
use std::io;

pub const UPDATE: u32 = 0x82130024;
// Immutable authored profile. Changing rewards requires a new journal revision.
pub const TRAINING_QUEST: u32 = 0x4000ff01;
pub const TARGET_ENTITY: u32 = 0x470007d1;
pub const GOAL: u8 = 3;
pub const REWARD_XP: u64 = 120;
pub const REWARD_GOLD: u32 = 10;
// New authored profile, separate from the immutable original training reward.
pub const SUPPLIES_QUEST: u32 = 0x4000ff02;
pub const SUPPLIES_XP: u64 = 80;
pub const SUPPLIES_GOLD: u32 = 6;
pub const POTION_QUEST: u32 = 0x4000ff03;
pub const JOURNEY_QUEST: u32 = 0x4000ff04;
pub fn rewards(command: &Command) -> Vec<crate::progression::Command> {
    if matches!(command, Command::Claim) {
        vec![
            crate::progression::Command::AwardXp(REWARD_XP),
            crate::progression::Command::AwardGold(REWARD_GOLD),
        ]
    } else if matches!(command, Command::ClaimSupplies) {
        vec![crate::progression::Command::AwardXp(SUPPLIES_XP), crate::progression::Command::AwardGold(SUPPLIES_GOLD)]
    } else if matches!(command, Command::ClaimPotion | Command::ClaimJourney) {
        vec![crate::progression::Command::AwardXp(80), crate::progression::Command::AwardGold(if matches!(command, Command::ClaimPotion) { 8 } else { 10 })]
    } else {
        Vec::new()
    }
}
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
    pub recovered: crate::recovered_quests::Book,
    pub exploration: crate::exploration_quests::Book,
    pub hunting: crate::hunting_quests::Book,
    pub arwic: crate::arwic_mines::State,
    training: Option<Progress>,
    supplies: Option<bool>,
    potion: u8, // 0 absent, 1 accepted, 2 crafted, 3 rewarded
    journey: u8, // 0 absent, 1 accepted, 2 bound, 3 travelled, 4 rewarded
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
    /// The quest tables' timed effects on the character (`quest_emotes` ApplyEffect): the
    /// `Effect` state, when it was put on and when it runs out (Unix seconds).
    effects: std::collections::BTreeMap<u32, (u64, u64)>,
    /// Each data-driven quest's times (`quest_emotes`), by journal id: when its current run
    /// started, when it was last solved and how often (`hunting_quests`' State precedent).
    timers: std::collections::BTreeMap<u32, Timer>,
    /// Story scenes the character has earned (`crate::story`), by scene id.
    scenes: std::collections::BTreeSet<u32>,
}

/// A data-driven quest's times, in Unix seconds (0: never).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Timer { pub started: u64, pub completed: u64, pub solves: u16 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// A story scene earned (`crate::story`): the player's act registry gains it.
    Scene(u32),
    Accept,
    Kill(u32),
    Claim,
    AcceptSupplies,
    ClaimSupplies,
    AcceptPotion,
    CraftedPotion,
    ClaimPotion,
    AcceptJourney,
    BoundLifestone,
    Travelled,
    ClaimJourney,
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
    /// A retired hunt's state leaves the hunting book (`Store::migrate_retired_hunts`).
    RetireHunt { quest: u32 },
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
            Self::RetireHunt { quest } => format!("retire,{quest:08x}"),
            Self::EffectOn { effect, at, until } => format!("effect,{effect:08x},{at},{until}"),
            Self::EffectOff { effect } => format!("effectoff,{effect:08x}"),
            Self::SubCount { quest, counter, count } => format!("subcount,{quest:08x},{counter},{count}"),
            Self::Kingdom(k) => format!("kingdom,{k}"),
            Self::KingdomPoints(p) => format!("kp,{p}"),
            Self::ShelterAdvance => "shelter.advance".into(),
            Self::ShelterComplete => "shelter.complete".into(),
            Self::ShelterGuard => "shelter.guard".into(),
            Self::ShelterLieutenant => "shelter.lieutenant".into(),
            Self::ShelterSentry => "shelter.sentry".into(),
            Self::Accept => "a".into(),
            Self::Kill(entity) => format!("k,{entity:08x}"),
            Self::Claim => "c".into(),
            Self::AcceptSupplies => "s.a".into(),
            Self::ClaimSupplies => "s.c".into(),
            Self::AcceptPotion => "p.a".into(), Self::CraftedPotion => "p.f".into(), Self::ClaimPotion => "p.c".into(),
            Self::AcceptJourney => "j.a".into(), Self::BoundLifestone => "j.b".into(), Self::Travelled => "j.t".into(), Self::ClaimJourney => "j.c".into(),
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
            "a" => Ok(Self::Accept),
            "c" => Ok(Self::Claim),
            "s.a" => Ok(Self::AcceptSupplies),
            "s.c" => Ok(Self::ClaimSupplies),
            "p.a" => Ok(Self::AcceptPotion), "p.f" => Ok(Self::CraftedPotion), "p.c" => Ok(Self::ClaimPotion),
            "j.a" => Ok(Self::AcceptJourney), "j.b" => Ok(Self::BoundLifestone), "j.t" => Ok(Self::Travelled), "j.c" => Ok(Self::ClaimJourney),
            _ if text.starts_with("kingdom,") => {
                let k: u8 = text["kingdom,".len()..].parse().map_err(|_| QuestError::InvalidRecord)?;
                if k > 3 { return Err(QuestError::InvalidRecord); }
                Ok(Self::Kingdom(k))
            }
            _ if text.starts_with("kp,") => Ok(Self::KingdomPoints(text[3..].parse().map_err(|_| QuestError::InvalidRecord)?)),
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
            _ if text.starts_with("retire,") => {
                let hex = &text["retire,".len()..];
                if hex.len() != 8 { return Err(QuestError::InvalidRecord); }
                let quest = u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)?;
                if !crate::hunting_quests::retired(quest) { return Err(QuestError::InvalidRecord); }
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
            _ => {
                let hex = text.strip_prefix("k,").ok_or(QuestError::InvalidRecord)?;
                if hex.len() != 8 || !hex.bytes().all(|value| value.is_ascii_hexdigit()) {
                    return Err(QuestError::InvalidRecord);
                }
                let entity = u32::from_str_radix(hex, 16).map_err(|_| QuestError::InvalidRecord)?;
                if entity >> 24 != 0x47 {
                    return Err(QuestError::InvalidRecord);
                }
                Ok(Self::Kill(entity))
            }
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
    /// A data-driven quest's further counter `counter`.
    pub fn subcount(&self, quest: u32, counter: u8) -> u16 { self.subcounts.get(&(quest, counter)).copied().unwrap_or(0) }
    /// The quest effects still running at `now` (Unix seconds): each `Effect` state and its end.
    pub fn quest_effects(&self, now: u64) -> Vec<(u32, u64)> {
        self.effects.iter().filter(|(_, (_, until))| *until > now).map(|(e, (_, until))| (*e, *until)).collect()
    }
    /// A data-driven quest's times (all zero for a quest never started under them).
    pub fn timer(&self, quest: u32) -> Timer { self.timers.get(&quest).copied().unwrap_or_default() }
    /// The story scenes earned, in id order.
    pub fn scenes(&self) -> impl Iterator<Item = u32> + '_ { self.scenes.iter().copied() }
    pub fn get(&self) -> Option<&Progress> {
        self.training.as_ref()
    }
    /// Shared player guidance for mentor speech, quest commands and the journal.
    /// The quest state and immutable rewards remain the persisted authored profile.
    pub fn guidance(&self) -> String {
        if self.journey != 0 { return self.journey_guidance(); }
        if self.potion != 0 { return self.potion_guidance(); }
        if self.supplies.is_some() { return self.supplies_guidance(); }
        match self.training {
            None => "Speak to a training mentor to begin Training Grounds, or use @quest start.".into(),
            Some(progress) if progress.claimed => format!(
                "Training complete. You received {REWARD_XP} experience and {REWARD_GOLD} gold. Speak to a training mentor again for the Field Supplies lesson."
            ),
            Some(progress) if progress.killed == GOAL => format!(
                "Training creatures defeated: {GOAL}/{GOAL}. Leave combat and speak to a training mentor for {REWARD_XP} experience and {REWARD_GOLD} gold, or use @quest claim."
            ),
            Some(progress) => format!(
                "Training creatures defeated: {}/{GOAL}. Equip a starter weapon from your bag and defeat Idol Spawn. Return to a training mentor after {GOAL} kills.", progress.killed
            ),
        }
    }
    pub fn ready(&self) -> bool {
        self.training
            .is_some_and(|state| state.killed == GOAL && !state.claimed)
    }
    pub fn apply(&self, command: &Command) -> Result<Self, QuestError> {
        if self
            .training
            .is_some_and(|state| state.killed > GOAL || state.claimed && state.killed != GOAL)
        {
            return Err(QuestError::InvalidRecord);
        }
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
            Command::EffectOn { effect, at, until } => { next.effects.insert(*effect, (*at, *until)); }
            Command::EffectOff { effect } => { next.effects.remove(effect); }
            Command::SubCount { quest, counter, count } => { next.subcounts.insert((*quest, *counter), *count); }
            Command::Kingdom(k) => { next.kingdom = *k; }
            Command::KingdomPoints(p) => { next.kingdom_points = *p; }
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
            Command::AcceptPotion => {
                if self.supplies != Some(true) { return Err(QuestError::NotReady); }
                if self.potion != 0 { return Err(QuestError::AlreadyAccepted); }
                next.potion = 1;
            }
            Command::CraftedPotion => {
                if self.potion != 1 { return Err(QuestError::NotReady); } next.potion = 2;
            }
            Command::ClaimPotion => {
                if self.potion != 2 { return Err(if self.potion == 3 { QuestError::AlreadyCompleted } else { QuestError::NotReady }); } next.potion = 3;
            }
            Command::AcceptJourney => {
                if self.potion != 3 { return Err(QuestError::NotReady); }
                if self.journey != 0 { return Err(QuestError::AlreadyAccepted); } next.journey = 1;
            }
            Command::BoundLifestone => {
                if self.journey != 1 { return Err(QuestError::NotReady); } next.journey = 2;
            }
            Command::Travelled => {
                if self.journey != 2 { return Err(QuestError::NotReady); } next.journey = 3;
            }
            Command::ClaimJourney => {
                if self.journey != 3 { return Err(if self.journey == 4 { QuestError::AlreadyCompleted } else { QuestError::NotReady }); } next.journey = 4;
            }
            Command::AcceptSupplies => {
                if !self.training.is_some_and(|state| state.claimed) { return Err(QuestError::NotReady); }
                if self.supplies.is_some() { return Err(QuestError::AlreadyAccepted); }
                next.supplies = Some(false);
            }
            Command::ClaimSupplies => {
                match self.supplies {
                    None => return Err(QuestError::NotAccepted),
                    Some(true) => return Err(QuestError::AlreadyCompleted),
                    Some(false) => next.supplies = Some(true),
                }
            }
            Command::Accept => {
                if let Some(state) = self.training {
                    return Err(if state.claimed {
                        QuestError::AlreadyCompleted
                    } else {
                        QuestError::AlreadyAccepted
                    });
                }
                next.training = Some(Progress {
                    killed: 0,
                    claimed: false,
                });
            }
            Command::Kill(entity) => {
                next = self.record_kill(*entity);
            }
            Command::Claim => {
                let state = next.training.as_mut().ok_or(QuestError::NotAccepted)?;
                if state.claimed {
                    return Err(QuestError::AlreadyCompleted);
                }
                if state.killed != GOAL {
                    return Err(QuestError::NotReady);
                }
                state.claimed = true;
            }
        }
        Ok(next)
    }
    /// A qualifying kill is credited once by the authoritative encounter owner.
    /// The caller commits this candidate in the same transaction as kill rewards.
    pub fn record_kill(&self, entity: u32) -> Self {
        let mut next = self.clone();
        if entity == TARGET_ENTITY {
            if let Some(state) = &mut next.training {
                if !state.claimed && state.killed < GOAL {
                    state.killed += 1;
                }
            }
        }
        next
    }
    /// One live journal change. Never call for an unaccepted quest.
    pub fn update(&self) -> Result<Vec<u8>, WorldError> {
        let state = self.training.ok_or(WorldError::InvalidValue)?;
        let mut arguments = script::integer(0);
        arguments.extend(native_graph(0xa0, &info_payload(state, true)?));
        arguments.extend(script::integer(status(state)));
        script::private_call(UPDATE, &arguments)
    }
    /// The gmQuestInfoList argument used by UI_Load for saved journal restoration.
    pub fn list_graph(&self) -> Result<Vec<u8>, WorldError> {
        let mut payload = Vec::new();
        word(&mut payload, u32::from(self.training.is_some()) + u32::from(self.supplies.is_some()) + u32::from(self.potion != 0) + u32::from(self.journey != 0) + self.journal_stages().count() as u32 + self.recovered.0.len() as u32 + self.exploration.0.len() as u32 + self.hunting.0.len() as u32 + u32::from(self.arwic.phase>0));
        if let Some(state) = self.training {
            payload.extend(info_payload(state, false)?);
        }
        if let Some(claimed) = self.supplies { payload.extend(supplies_payload(claimed, false)?); }
        if self.potion != 0 { payload.extend(self.lesson_payload(false, false)?); }
        if self.journey != 0 { payload.extend(self.lesson_payload(true, false)?); }
        for (quest, stage) in self.journal_stages() { payload.extend(stage_payload(quest, stage, self.timer(quest), false)?); }
        for (id,state) in &self.recovered.0 { payload.extend(recovered_payload(*id,state,false)?); }
        for (id,state) in &self.exploration.0 { payload.extend(exploration_payload(*id,state,false)?); }
        for (id,state) in &self.hunting.0 { payload.extend(hunting_payload(*id,state,false)?); }
        if self.arwic.phase>0 {payload.extend(arwic_payload(&self.arwic,false)?);}
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

fn status(state: Progress) -> u32 {
    if state.claimed { COMPLETED } else { UNDERWAY }
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
fn info_payload(state: Progress, play_fx: bool) -> Result<Vec<u8>, WorldError> {
    if state.killed > GOAL || state.claimed && state.killed != GOAL {
        return Err(WorldError::InvalidValue);
    }
    let mut bytes = Vec::new();
    word(&mut bytes, TRAINING_QUEST);
    bytes.extend(world::literal_name("Training Grounds")?);
    bytes.extend(world::literal_name(&format!(
        "Defeat three Idol Spawn training creatures, then speak to a training mentor for {REWARD_XP} experience and {REWARD_GOLD} gold. You may also use @quest claim."
    ))?);
    word(&mut bytes, ICON);
    word(&mut bytes, status(state));
    word(&mut bytes, if state.claimed { 0 } else { 1 });
    let entry = Book { training: Some(state), ..Book::default() }.guidance();
    bytes.extend(world::literal_name(&entry)?);
    word(&mut bytes, 1); // Authored challenge level.
    bytes.extend((-1.0f64).to_le_bytes()); // Unset bestowal timestamp, native Clear sentinel.
    bytes.extend((-1.0f64).to_le_bytes()); // Unset completion timestamp.
    word(&mut bytes, 0); // No expiration.
    bytes.extend(0f64.to_le_bytes());
    bytes.extend(0f64.to_le_bytes());
    word(&mut bytes, u32::from(play_fx));
    Ok(bytes)
}

impl Book {
    pub fn supplies(&self) -> Option<bool> { self.supplies }
    pub fn supplies_guidance(&self) -> String {
        if self.supplies == Some(true) {
            "Field Supplies complete. You received a health potion, 80 experience and 6 gold. Use the crafting panel to combine a Mucor Mushroom and 100 Tallow into a health or vigor potion. Speak to a mentor again to begin Potion Practice.".into()
        } else {
            "Field Supplies: bring one Mucor Mushroom and one full 100-unit Tallow bundle to a training mentor. Buy them from the quartermaster for 2 gold each and keep them in your bag. Your reward is a health potion, 80 experience and 6 gold.".into()
        }
    }
    pub fn updates(&self) -> Result<Vec<Vec<u8>>, WorldError> {
        self.updates_since(&Self::default())
    }
    /// Native UpdateQuestInfo replaces one entry. Resending completed entries
    /// on every kill grows traffic with journal history and can fill the GUN
    /// reliable queue. Login still sends the complete list_graph snapshot.
    pub fn updates_since(&self, previous:&Self) -> Result<Vec<Vec<u8>>, WorldError> {
        let mut events = Vec::new();
        if self.arwic.phase>0 && self.arwic!=previous.arwic {
            let mut args=script::integer(0);args.extend(native_graph(0xa0,&arwic_payload(&self.arwic,true)?));
            args.extend(script::integer(if self.arwic.phase==5{COMPLETED}else{UNDERWAY}));
            events.push(script::private_call(UPDATE,&args)?);
        }
        for (id,state) in &self.exploration.0 {
            if previous.exploration.0.get(id)==Some(state) {continue;}
            let mut args=script::integer(0);args.extend(native_graph(0xa0,&exploration_payload(*id,state,true)?));
            args.extend(script::integer(if state.completed>0 {COMPLETED}else{UNDERWAY}));
            events.push(script::private_call(UPDATE,&args)?);
        }
        for (id,state) in &self.hunting.0 {
            if previous.hunting.0.get(id)==Some(state) {continue;}
            let mut args=script::integer(0);args.extend(native_graph(0xa0,&hunting_payload(*id,state,true)?));
            args.extend(script::integer(if state.completed>0 {COMPLETED}else{UNDERWAY}));
            events.push(script::private_call(UPDATE,&args)?);
        }
        for (id,state) in &self.recovered.0 {
            if previous.recovered.0.get(id)==Some(state) {continue;}
            let mut args=script::integer(0);args.extend(native_graph(0xa0,&recovered_payload(*id,state,true)?));
            args.extend(script::integer(if state.completed>0 {COMPLETED}else{UNDERWAY}));
            events.push(script::private_call(UPDATE,&args)?);
        }
        for (quest, stage) in self.journal_stages().filter(|(q, s)| previous.stage(*q) != *s) {
            let mut args=script::integer(0);
            args.extend(native_graph(0xa0,&stage_payload(quest,stage,self.timer(quest),true)?));
            args.extend(script::integer(if stage_completed(quest,stage) {COMPLETED} else {UNDERWAY}));
            events.push(script::private_call(UPDATE,&args)?);
        }
        if self.training.is_some() && self.training!=previous.training { events.push(self.update()?); }
        if let Some(claimed) = self.supplies.filter(|_|self.supplies!=previous.supplies) {
            let mut args = script::integer(0);
            args.extend(native_graph(0xa0, &supplies_payload(claimed, true)?));
            args.extend(script::integer(if claimed { COMPLETED } else { UNDERWAY }));
            events.push(script::private_call(UPDATE, &args)?);
        }
        for journey in [false, true] {
            if if journey {self.journey==previous.journey} else {self.potion==previous.potion} {continue;}
            if if journey { self.journey != 0 } else { self.potion != 0 } {
                let mut args = script::integer(0);
                args.extend(native_graph(0xa0, &self.lesson_payload(journey, true)?));
                let claimed = if journey { self.journey == 4 } else { self.potion == 3 };
                args.extend(script::integer(if claimed { COMPLETED } else { UNDERWAY }));
                events.push(script::private_call(UPDATE, &args)?);
            }
        }
        Ok(events)
    }
}
fn supplies_payload(claimed: bool, play_fx: bool) -> Result<Vec<u8>, WorldError> {
    let mut bytes = Vec::new();
    word(&mut bytes, SUPPLIES_QUEST);
    bytes.extend(world::literal_name("Field Supplies")?);
    bytes.extend(world::literal_name("Bring a Mucor Mushroom and a full 100-unit Tallow bundle to a training mentor. The mentor keeps these supplies and gives you one health potion, 80 experience and 6 gold.")?);
    word(&mut bytes, ICON);
    word(&mut bytes, if claimed { COMPLETED } else { UNDERWAY });
    word(&mut bytes, u32::from(!claimed));
    bytes.extend(world::literal_name(&Book { supplies: Some(claimed), ..Book::default() }.supplies_guidance())?);
    word(&mut bytes, 1);
    bytes.extend((-1.0f64).to_le_bytes()); bytes.extend((-1.0f64).to_le_bytes());
    word(&mut bytes, 0); bytes.extend(0f64.to_le_bytes()); bytes.extend(0f64.to_le_bytes());
    word(&mut bytes, u32::from(play_fx));
    Ok(bytes)
}
/// Canonical once-only supply exchange. Only owned bag items with the exact
/// supported profiles qualify; bought or otherwise acquired supplies are accepted.
pub fn supplies_items(bag: &crate::inventory::Inventory, first: u64) -> io::Result<Vec<crate::inventory::Command>> {
    use crate::inventory::{Command as ItemCommand, Location};
    let mushroom = bag.items().find(|item| matches!(item.location, Location::Bag(_)) && item.quantity == 1 && item.definition == crate::crafting::MUCOR && item.visual == crate::merchants::MUCOR_VISUAL)
        .ok_or_else(|| io::Error::other("Bring one Mucor Mushroom in your bag."))?;
    let tallow = bag.items().find(|item| matches!(item.location, Location::Bag(_)) && item.quantity == crate::crafting::TALLOW_QUANTITY && item.definition == crate::crafting::TALLOW)
        .ok_or_else(|| io::Error::other("Bring a full 100-unit Tallow bundle in your bag."))?;
    let mut commands = vec![ItemCommand::ConsumeDense { item: mushroom.id }, ItemCommand::ConsumeStackDense { item: tallow.id, quantity: crate::crafting::TALLOW_QUANTITY }];
    let mut candidate = bag.clone();
    for command in &commands { candidate = candidate.apply(command).map_err(io::Error::other)?; }
    let slot = candidate.free_slot().ok_or_else(|| io::Error::other("Make room for the reward potion."))?;
    commands.push(ItemCommand::Grant(crate::item_services::potion(first, slot)));
    Ok(commands)
}
/// Used both before writing and on journal replay. An arbitrary inventory
/// transaction cannot claim the objective or inject a different payout.
pub fn validate_supplies_exchange(book: &Book, bag: &crate::inventory::Inventory, items: &[crate::inventory::Command], changes: &[crate::progression::Command], first: u64) -> io::Result<Book> {
    let next = book.apply(&Command::ClaimSupplies).map_err(io::Error::other)?;
    if crate::player_transaction::encode_items(items) != crate::player_transaction::encode_items(&supplies_items(bag, first)?)
        || crate::player_transaction::encode_changes(changes) != crate::player_transaction::encode_changes(&rewards(&Command::ClaimSupplies)) {
        return Err(io::Error::other("Invalid Field Supplies exchange."));
    }
    Ok(next)
}

impl Book {
    /// The data-driven quests the journal lists: those past stage 0 that the registry knows.
    fn journal_stages(&self) -> impl Iterator<Item = (u32, u8)> + '_ {
        let tables = crate::quest_emotes::tables();
        self.stages.iter().filter(move |(q, s)| **s != 0 && tables.as_ref().is_some_and(|t| t.quests().any(|r| r.journal_id == **q)))
            .map(|(q, s)| (*q, *s))
    }
    pub fn potion_phase(&self) -> u8 { self.potion }
    pub fn journey_phase(&self) -> u8 { self.journey }
    pub fn potion_guidance(&self) -> String {
        match self.potion {
            2 => "Potion Practice: you crafted a potion. Return to a training mentor for 80 experience and 8 gold. You keep your potion.".into(),
            3 => "Potion Practice complete. You received 80 experience and 8 gold. Speak to a mentor again to begin A Safe Journey.".into(),
            _ => "Potion Practice: buy a Mucor Mushroom and a 100-unit Tallow bundle, then use the crafting panel to create a health or vigor potion. Bought potions do not count. Return to a mentor after crafting for 80 experience and 8 gold.".into(),
        }
    }
    pub fn journey_guidance(&self) -> String {
        match self.journey {
            2 => "A Safe Journey: your lifestone is bound. Leave combat and use a training portal, then speak to a training mentor at your destination.".into(),
            3 => "A Safe Journey: you bound a lifestone and used a training portal. Speak to a training mentor for 80 experience and 10 gold.".into(),
            4 => "Training complete! You finished combat, supplies, potion crafting and safe travel. You received the final 80 experience and 10 gold. The outdoor hub connects all three racial starts; its mentors and quartermasters remain available.".into(),
            _ => "A Safe Journey: use a lifestone to bind it, then use a training portal. Leave combat before both actions. Speak to a mentor at your destination for 80 experience and 10 gold. Rebind now even if you bound a lifestone before accepting this lesson.".into(),
        }
    }
    fn lesson_payload(&self, journey: bool, play_fx: bool) -> Result<Vec<u8>, WorldError> {
        let (id, name, text, claimed) = if journey {
            (JOURNEY_QUEST, "A Safe Journey", self.journey_guidance(), self.journey == 4)
        } else { (POTION_QUEST, "Potion Practice", self.potion_guidance(), self.potion == 3) };
        let mut bytes = Vec::new(); word(&mut bytes, id);
        bytes.extend(world::literal_name(name)?); bytes.extend(world::literal_name(&text)?);
        word(&mut bytes, ICON); word(&mut bytes, if claimed { COMPLETED } else { UNDERWAY });
        word(&mut bytes, u32::from(!claimed)); bytes.extend(world::literal_name(&text)?); word(&mut bytes, 1);
        bytes.extend((-1.0f64).to_le_bytes()); bytes.extend((-1.0f64).to_le_bytes()); word(&mut bytes, 0);
        bytes.extend(0f64.to_le_bytes()); bytes.extend(0f64.to_le_bytes()); word(&mut bytes, u32::from(play_fx));
        Ok(bytes)
    }
}
pub fn is_training_portal(id: u64) -> bool {
    matches!(id, 0x8140000000030002 | 0x8140000000030004 | 0x8140000000030005 | 0x8140000000030022 | 0x8140000000030025 | 0x8140000000030030 | 0x8140000000030031)
}
/// The a3 revision credits only a genuine canonical crafting exchange. Shop
/// purchases, potion grants and reward exchanges cannot advance this lesson.
pub fn validate_practice_craft(book: &Book, bag: &crate::inventory::Inventory, items: &[crate::inventory::Command], changes: &[crate::progression::Command], first: u64) -> io::Result<Book> {
    use crate::inventory::Command as ItemCommand;
    let bad = || io::Error::other("Invalid Potion Practice craft transaction.");
    let [ItemCommand::ConsumeDense { item: mushroom }, ItemCommand::ConsumeStackDense { item: tallow, quantity }, ItemCommand::Grant(output)] = items else { return Err(bad()); };
    if *quantity != crate::crafting::TALLOW_QUANTITY || !changes.is_empty() { return Err(bad()); }
    let recipe = match output.definition { crate::crafting::HEALTH_OUTPUT => crate::crafting::HEALTH_RECIPE, crate::crafting::VIGOR_OUTPUT => crate::crafting::VIGOR_RECIPE, _ => return Err(bad()) };
    let request = crate::crafting::ExecuteRequest { primary: *mushroom, ingredients: std::collections::BTreeMap::from([(1, *mushroom), (2, *tallow)]), spinner: 0, recipe };
    request.require_inputs(bag, true).map_err(|_| bad())?;
    let candidate = bag.apply(&items[0]).and_then(|bag| bag.apply(&items[1])).map_err(|_| bad())?;
    let expected = crate::crafting::output(recipe, first, candidate.free_slot().ok_or_else(bad)?).ok_or_else(bad)?;
    if output != &expected { return Err(bad()); }
    book.apply(&Command::CraftedPotion).map_err(io::Error::other)
}

fn arwic_payload(s:&crate::arwic_mines::State,fx:bool)->Result<Vec<u8>,WorldError> {
    let c=crate::arwic_mines::content();let mut b=Vec::new();word(&mut b,crate::arwic_mines::QUEST);
    b.extend(world::literal_name(&c.title)?);b.extend(world::literal_name(&c.summary)?);word(&mut b,ICON);
    word(&mut b,if s.phase==5{COMPLETED}else{UNDERWAY});word(&mut b,if s.phase==5{0}else{s.phase});
    b.extend(world::literal_name(&s.guidance())?);word(&mut b,1);b.extend((s.started as f64).to_le_bytes());
    b.extend((if s.phase==5{s.completed as f64}else{-1.}).to_le_bytes());
    word(&mut b,0);b.extend(0f64.to_le_bytes());b.extend(0f64.to_le_bytes());word(&mut b,u32::from(fx));Ok(b)
}
fn exploration_payload(id:u32,state:&crate::exploration_quests::State,fx:bool)->Result<Vec<u8>,WorldError> {
    let q=crate::exploration_quests::profile(id).ok_or(WorldError::InvalidValue)?;
    let mut b=Vec::new();word(&mut b,id);b.extend(world::literal_name(&q.title)?);
    b.extend(world::literal_name(&q.text)?);word(&mut b,ICON);
    word(&mut b,if state.completed>0 {COMPLETED}else{UNDERWAY});
    word(&mut b,if state.completed>0 {0}else{state.phase as u32});
    b.extend(world::literal_name(&crate::exploration_quests::Book::guidance(q,state))?);word(&mut b,u32::from(q.level));
    b.extend((state.started as f64).to_le_bytes());b.extend((if state.completed>0{state.completed as f64}else{-1.}).to_le_bytes());
    word(&mut b,0);b.extend(0f64.to_le_bytes());b.extend(0f64.to_le_bytes());word(&mut b,u32::from(fx));Ok(b)
}

fn hunting_payload(id:u32,state:&crate::hunting_quests::State,fx:bool)->Result<Vec<u8>,WorldError> {
    let q=crate::hunting_quests::profile(id).ok_or(WorldError::InvalidValue)?;
    let mut b=Vec::new();word(&mut b,id);b.extend(world::literal_name(&q.title)?);
    b.extend(world::literal_name(&q.text)?);word(&mut b,ICON);
    word(&mut b,if state.completed>0 {COMPLETED}else{UNDERWAY});
    word(&mut b,if state.completed>0 {0}else{u32::from(state.count)+1});
    b.extend(world::literal_name(&crate::hunting_quests::Book::guidance(q,state))?);word(&mut b,u32::from(q.level));
    b.extend((state.started as f64).to_le_bytes());b.extend((if state.completed>0{state.completed as f64}else{-1.}).to_le_bytes());
    word(&mut b,0);b.extend(0f64.to_le_bytes());b.extend(0f64.to_le_bytes());word(&mut b,u32::from(fx));Ok(b)
}

fn recovered_payload(id:u32,state:&crate::recovered_quests::State,fx:bool)->Result<Vec<u8>,WorldError> {
    let q=crate::recovered_quests::profiles().iter().find(|q|q.id==id).ok_or(WorldError::InvalidValue)?;
    let mut b=Vec::new();word(&mut b,id);b.extend(world::literal_name(&q.title)?);
    b.extend(world::literal_name(&q.text)?);word(&mut b,ICON);
    word(&mut b,if state.completed>0 {COMPLETED}else{UNDERWAY});word(&mut b,u32::from(state.completed==0));
    b.extend(world::literal_name(&crate::recovered_quests::Book::guidance(q,state))?);word(&mut b,u32::from(q.level));
    b.extend((state.started as f64).to_le_bytes());b.extend((if state.completed>0{state.completed as f64}else{-1.}).to_le_bytes());
    word(&mut b,0);b.extend(0f64.to_le_bytes());b.extend(0f64.to_le_bytes());word(&mut b,u32::from(fx));Ok(b)
}

fn stage_completed(quest: u32, stage: u8) -> bool {
    crate::quest_emotes::tables().is_some_and(|t| t.stage(quest, stage).is_some_and(|(done, _)| done))
}
/// A data-driven quest's journal entry: the registry's title and level, the stage's line and the
/// run's start and solve times.
fn stage_payload(quest: u32, stage: u8, timer: Timer, play_fx: bool) -> Result<Vec<u8>, WorldError> {
    let tables = crate::quest_emotes::tables().ok_or(WorldError::InvalidValue)?;
    let registry = tables.quests().find(|q| q.journal_id == quest).ok_or(WorldError::InvalidValue)?;
    let title = &registry.title_text;
    let (done, text) = tables.stage(quest, stage).ok_or(WorldError::InvalidValue)?;
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
    let now = crate::recovered_quests::now();
    let remaining = registry.time_limit.filter(|_| !done && timer.started != 0).map(|l| timer.started.saturating_add(l).saturating_sub(now));
    let retry = registry.repeat.filter(|_| done && timer.completed != 0).map_or(0, |r| timer.completed.saturating_add(r).saturating_sub(now));
    word(&mut bytes,u32::from(remaining.is_some())); bytes.extend((remaining.unwrap_or(0) as f64).to_le_bytes());
    bytes.extend((retry as f64).to_le_bytes());
    word(&mut bytes,u32::from(play_fx)); Ok(bytes)
}
