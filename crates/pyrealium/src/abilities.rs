//! Active skill use driven by portal.dat skill records (`content/skill-effects.tsv`
//! and `content/skill-travel.tsv`).
//!
//! Each record supplies reset time, vigor cost, max range, valid targets, hand
//! implement requirements, story behavior and FX, an attack damage table and its
//! effects, all valued at the player's trained rank by linear interpolation
//! between the record's level points. Values match archived retail skill pages
//! (Ease Suffering: 100 vigor, 40 m, 7 s, heal 25 at L1 to 250 at L50; Flame
//! Bolt: stave, 32 vigor, 60 m, 3 to 75 plus weapon damage, burn 1 to 4 per
//! second for 30 s).
//!
//! Applied effects: instant and over-time health (0x21, 0x11) and vigor (0x22,
//! 0x12), health and vigor regeneration multipliers (stats 0x30106, 0x30107),
//! attack damage added to the encounter's base damage, weapon-only attacks,
//! and timed armor, attack and
//! defense ratings. Armor uses NaturalArmor 0x40125 and ArmorLevel 0x40262.
//! Attack ratings are 0x800001/7/c and defense ratings 0x800002/8/d (magic,
//! melee, missile). Other buff stats, projectile flight, area attacks, combos
//! and summoned portals are not applied. Recovered recall destinations are
//! retained as evidence; only the server's existing bound-lifestone recall is
//! executable. Base damage and the hit curve remain authored server policy.
//!
//! A prepared cast borrows the runtime exclusively. Admit its UI batch, persist
//! `commands()` atomically, then call `commit()`. Dropping it on any earlier
//! error spends no cooldown. The consuming commit cannot fail after persistence.
use crate::{
    encounters::{Over, Regen},
    movement::Placement,
    progression::{Command, Progression},
    skills::Book,
};
use std::{collections::BTreeMap, fmt};

/// Prevent a matching skill ID from hiding effects imported from another DAT build.
pub fn validate_assets(portal: &mut crate::dat::Dat) -> std::io::Result<()> {
    use sha2::{Digest, Sha256};
    #[derive(serde::Deserialize)]
    struct Report {
        record_sha256: BTreeMap<String, String>,
    }
    let report: Report =
        serde_json::from_str(crate::data_profile::table(
            crate::content_db::files::text("skill-effects.report.json"),
            crate::content_db::files::text("7230/skill-effects.report.json")))?;
    for (record, expected) in report.record_sha256 {
        let did = u32::from_str_radix(&record, 16).map_err(std::io::Error::other)?;
        let actual = format!("{:x}", Sha256::digest(portal.read(did)?));
        if actual != expected {
            return Err(std::io::Error::other(format!(
                "skill effect record changed: {did:08x}; regenerate skill-effects.tsv"
            )));
        }
    }
    Ok(())
}

pub const GLOBAL_COOLDOWN_MS: u64 = 1_000;
/// Retail skill pages list a minimum vigor of 1; the cost then drains what remains.
pub const MIN_VIGOR: u32 = 1;
const MAX_SKILL_COOLDOWNS: usize = 256;
const MAX_PLAYER_COOLDOWNS: usize = 128;
const SELF: u32 = 0x1;
const MONSTER: u32 = 0x80;
const PLAYER: u32 = 0x100;
const HEALTH: u32 = 0x21;
const VIGOR: u32 = 0x22;
const HEALTH_OVER_TIME: u32 = 0x11;
const VIGOR_OVER_TIME: u32 = 0x12;
const HEALTH_REGEN: u32 = 0x30106;
const VIGOR_REGEN: u32 = 0x30107;
const NATURAL_ARMOR: u32 = 0x40125;
const ARMOR_LEVEL: u32 = 0x40262;
/// Magic, melee, missile. Matches SkillList attack 1/7/c and defense 2/8/d.
const ATTACK_RATING: [u32; 3] = [0x800001, 0x800007, 0x80000c];
const DEFENSE_RATING: [u32; 3] = [0x800002, 0x800008, 0x80000d];
/// Weapon-attack effect type carried by the racial basic attacks.
const WEAPON_ATTACK: u32 = 0x6;
const BASIC_ATTACK_KIND: u32 = 0x44;
const ANY_AND_NONE: u32 = 0xffff_ffff;
const NO_IMPLEMENT: u32 = 0x1;
/// Implement type names (enum 23000003) used in requirement messages.
const IMPLEMENTS: [(u32, &str); 20] = [
    (0x1, "an empty hand"),
    (0x2, "a sword"),
    (0x4, "a bow"),
    (0x8, "a drum"),
    (0x10, "a shield"),
    (0x20, "an axe"),
    (0x40, "a dagger"),
    (0x100, "a hammer"),
    (0x400, "a two-handed sword"),
    (0x800, "an alchemy kit"),
    (0x8000, "a spear"),
    (0x10000, "a jailai"),
    (0x20000, "a flail"),
    (0x40000, "a stave"),
    (0x80000, "a scythe"),
    (0x80062, "a one-handed weapon"),
    (0x100000, "a hand weapon"),
    (0x200000, "an orb"),
    (0x3fffee, "a weapon"),
    (0xfffffffe, "an item"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbilityError {
    Catalog,
    Unsupported,
    InvalidCharacter,
    NotLearned,
    InvalidTarget,
    TargetCreature,
    OutOfRange,
    Hand { right: bool, required: u32 },
    Dead,
    NothingToRestore,
    InsufficientVigor,
    Cooldown { remaining_ms: u64 },
    GlobalCooldown { remaining_ms: u64 },
    Capacity,
    Clock,
    InvalidProgression,
}
impl fmt::Display for AbilityError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog => out.write_str("The skill effect table is invalid."),
            Self::Unsupported => out.write_str("This skill's effects are not supported yet."),
            Self::InvalidCharacter => out.write_str("The active character is unavailable."),
            Self::NotLearned => out.write_str("You have not learned this skill."),
            Self::InvalidTarget => out.write_str("That is not a valid target for this skill."),
            Self::TargetCreature => out.write_str("Select a creature to use this skill."),
            Self::OutOfRange => out.write_str("Your target is too far away for this skill."),
            Self::Hand { right, required } => write!(
                out,
                "This skill requires {} in your {} hand.",
                implement_name(*required),
                if *right { "right" } else { "left" }
            ),
            Self::Dead => out.write_str("You cannot use this skill while dead."),
            Self::NothingToRestore => out.write_str("Your health and vigor are already full."),
            Self::InsufficientVigor => {
                out.write_str("You do not have enough vigor for this skill.")
            }
            Self::Cooldown { remaining_ms } => write!(
                out,
                "This skill is recovering. Wait {} second(s).",
                remaining_ms.div_ceil(1_000)
            ),
            Self::GlobalCooldown { remaining_ms } => write!(
                out,
                "Wait {} second(s) before using another skill.",
                remaining_ms.div_ceil(1_000)
            ),
            Self::Capacity => out.write_str("The skill service is at capacity. Try again shortly."),
            Self::Clock => out.write_str("The skill clock is unavailable."),
            Self::InvalidProgression => {
                out.write_str("The current character vitals cannot accept this skill.")
            }
        }
    }
}
impl std::error::Error for AbilityError {}

fn implement_name(mask: u32) -> &'static str {
    IMPLEMENTS
        .iter()
        .find(|(bits, _)| *bits == mask)
        .or_else(|| {
            IMPLEMENTS
                .iter()
                .find(|(bits, _)| mask & bits != 0 && bits.count_ones() == 1)
        })
        .map_or("a different implement", |(_, name)| name)
}

/// Level points (level, value), sorted by level.
#[derive(Clone, Debug, Default, PartialEq)]
struct Table(Vec<(f32, f32)>);
impl Table {
    fn parse(text: &str) -> Result<Self, AbilityError> {
        if text == "-" {
            return Ok(Self::default());
        }
        let mut points = Vec::new();
        for pair in text.split(',') {
            let (level, value) = pair.split_once(':').ok_or(AbilityError::Catalog)?;
            let level: f32 = level.parse().map_err(|_| AbilityError::Catalog)?;
            let value: f32 = value.parse().map_err(|_| AbilityError::Catalog)?;
            if !level.is_finite()
                || !value.is_finite()
                || !(0.0..=10000.0).contains(&level)
                || value.abs() > 100000.0
                || points.len() >= 64
            {
                return Err(AbilityError::Catalog);
            }
            points.push((level, value));
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        if points.windows(2).any(|p| p[0].0 == p[1].0) {
            return Err(AbilityError::Catalog);
        }
        Ok(Self(points))
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn scaled(mut self, scale: f32) -> Self {
        for point in &mut self.0 {
            point.1 *= scale;
        }
        self
    }
    /// Linear between points, flat outside them.
    fn at(&self, rank: u16) -> f32 {
        let rank = f32::from(rank.max(1));
        let points = &self.0;
        let Some(&(first_level, first_value)) = points.first() else {
            return 0.0;
        };
        if rank <= first_level {
            return first_value;
        }
        for window in points.windows(2) {
            let ((low, low_value), (high, high_value)) = (window[0], window[1]);
            if rank <= high {
                let span = (high - low).max(f32::EPSILON);
                return low_value + (high_value - low_value) * (rank - low) / span;
            }
        }
        points.last().unwrap().1
    }
}

/// Development policy, test servers only: `PYREALIUM_DEV_EFFECT_SCALE` (a number in 0.01 to 1)
/// shortens every timed effect's duration by that factor, so a live test can watch an effect
/// run out in seconds. Unset, durations are the skill records'.
fn effect_scale() -> f32 {
    std::env::var("PYREALIUM_DEV_EFFECT_SCALE")
        .ok()
        .and_then(|text| text.parse::<f32>().ok())
        .filter(|scale| (0.01..=1.0).contains(scale))
        .unwrap_or(1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Target,
    Own,
}

#[derive(Clone, Debug)]
struct Timed {
    side: Side,
    vigor: bool,
    values: Table,
    period_ms: u64,
    duration: Table,
}

#[derive(Clone, Debug)]
struct Buff {
    side: Side,
    health: Table,
    vigor: Table,
    duration: Table,
}
#[derive(Clone, Debug)]
struct RatingSpec {
    side: Side,
    duration: Table,
    armor: Vec<Table>,
    attack: [Vec<Table>; 3],
    defense: [Vec<Table>; 3],
}
impl RatingSpec {
    fn useful(&self) -> bool {
        !self.armor.is_empty()
            || self.attack.iter().any(|tables| !tables.is_empty())
            || self.defense.iter().any(|tables| !tables.is_empty())
    }
    fn resolve(&self, rank: u16) -> (crate::combat::Ratings, u64) {
        let sum = |tables: &[Table]| -> i32 {
            tables
                .iter()
                .map(|table| table.at(rank).round() as i32)
                .sum()
        };
        let ratings = crate::combat::Ratings {
            armor: sum(&self.armor),
            attack: std::array::from_fn(|index| sum(&self.attack[index])),
            defense: std::array::from_fn(|index| sum(&self.defense[index])),
        };
        let seconds = self.duration.at(rank).max(0.0);
        let duration_ms = if seconds <= 0.0 {
            180_000
        } else {
            (seconds * 1000.0) as u64
        };
        (ratings, duration_ms)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Travel {
    Lifestone,
    Portal,
    Telepads(Vec<Placement>),
}

/// A timed effect record of a skill (its `Effect` state and how long it lasts), which the
/// client lists in its effect strip while it lasts.
#[derive(Clone, Debug)]
struct Applied {
    side: Side,
    did: u32,
    duration: Table,
    /// A health or vigor change over time: its record is flagged pulsed.
    pulsed: bool,
    /// What the effect does, in a few words (a creature's examination lists it).
    label: String,
}

/// A few words for an effect from its type code and first stat or value: Pyrealium's own text,
/// the creature's examination has no retail text for another creature's effects.
fn effect_label(kind: u32, stats: &[(u32, Table)], values: &Table) -> String {
    let first = |t: &Table| t.0.first().map_or(0.0, |p| p.1);
    let up = |v: f32| v >= 0.0 || (v - 1.0).abs() < f32::EPSILON;
    let text = match kind {
        HEALTH_OVER_TIME if first(values) < 0.0 => "Burning (health lost over time)",
        HEALTH_OVER_TIME => "Healing over time",
        VIGOR_OVER_TIME if first(values) < 0.0 => "Vigor drained over time",
        VIGOR_OVER_TIME => "Vigor restored over time",
        _ => match stats.first() {
            Some((id, t)) if [NATURAL_ARMOR, ARMOR_LEVEL].contains(id) => {
                if up(first(t)) { "Armor raised" } else { "Armor lowered" }
            }
            Some((id, t)) if ATTACK_RATING.contains(id) => {
                if up(first(t)) { "Attack raised" } else { "Attack lowered" }
            }
            Some((id, t)) if DEFENSE_RATING.contains(id) => {
                if up(first(t)) { "Defense raised" } else { "Defense lowered" }
            }
            Some((id, _)) if *id == HEALTH_REGEN => "Health regeneration changed",
            Some((id, _)) if *id == VIGOR_REGEN => "Vigor regeneration changed",
            _ => "Lingering effect",
        },
    };
    text.to_owned()
}

/// A timed effect a prepared cast puts on `entity` for `duration_ms`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedEffect {
    pub entity: u64,
    /// The `Effect` state (DID) the client shows.
    pub did: u32,
    pub duration_ms: u64,
    /// The caster's rank in the skill, the level its tables were read at (the record's
    /// spellcraft).
    pub power: f32,
    pub pulsed: bool,
}

#[derive(Clone, Debug)]
struct Definition {
    applied: Vec<Applied>,
    kind: u32,
    reset_ms: u64,
    vigor: u32,
    range: f32,
    targets: u32,
    right: u32,
    left: u32,
    story: u32,
    fx: u32,
    damage: Table,
    health: Table,
    vigor_effect: Table,
    timed: Vec<Timed>,
    buff: Option<Buff>,
    ratings: Vec<RatingSpec>,
    unresolved: bool,
    weapon: bool,
    travel: Option<Travel>,
}
impl Definition {
    fn harmful(&self) -> bool {
        self.weapon
            || !self.damage.is_empty()
            || self.health.0.iter().any(|&(_, value)| value < 0.0)
            || self.vigor_effect.0.iter().any(|&(_, value)| value < 0.0)
            || self
                .timed
                .iter()
                .any(|t| t.side == Side::Target && t.values.0.iter().any(|&(_, v)| v < 0.0))
            || self
                .ratings
                .iter()
                .filter(|r| r.side == Side::Target)
                .any(|r| {
                    r.armor
                        .iter()
                        .chain(r.attack.iter().flatten())
                        .chain(r.defense.iter().flatten())
                        .any(|t| t.0.iter().any(|(_, v)| *v < 0.0))
                })
    }
    fn usable(&self) -> bool {
        self.weapon
            || self.travel.is_some()
            || !self.damage.is_empty()
            || !self.health.is_empty()
            || !self.vigor_effect.is_empty()
            || !self.timed.is_empty()
            || self.buff.is_some()
            || self.ratings.iter().any(RatingSpec::useful)
    }
    fn ratings_for(&self, side: Side, rank: u16) -> Option<(crate::combat::Ratings, u64)> {
        let mut total = crate::combat::Ratings::default();
        let mut duration = 0u64;
        let mut found = false;
        for spec in &self.ratings {
            if spec.side != side || !spec.useful() {
                continue;
            }
            let (part, ms) = spec.resolve(rank);
            total.add(part);
            duration = duration.max(ms);
            found = true;
        }
        (found && !total.is_empty()).then_some((total, duration))
    }
}

/// What the caster holds. `Unknown` items have no implement metadata and cannot satisfy a required implement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Empty,
    Implement(u32),
    Unknown,
}
fn holds(required: u32, hand: Hand) -> bool {
    match hand {
        _ if required == ANY_AND_NONE => true,
        Hand::Unknown => false,
        Hand::Empty => required & NO_IMPLEMENT != 0,
        Hand::Implement(bits) => required & bits != 0,
    }
}

/// Where the player aimed the skill. `Player` carries the distance in meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aim {
    Own,
    Creature(u64),
    Player(u64, f32),
    Other,
}

/// What the server must do with a prepared cast.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Instant vitals are in `commands()`; `mend` and `regen` go to encounters.
    Restore {
        health: u32,
        vigor: u32,
        mend: Option<Over>,
        regen: Option<Regen>,
    },
    /// Vigor is in `commands()`; the target's vitals land on the next tick.
    Support {
        target: u64,
        health: u32,
        vigor: u32,
        mend: Option<Over>,
        regen: Option<Regen>,
    },
    /// Queue one encounter strike; its vigor is spent when it lands.
    Strike {
        target: u64,
        bonus: u32,
        vigor_damage: u32,
        weapon_damage: bool,
        cost: u32,
        range: f32,
        burn: Option<Over>,
    },
    /// Move the caster; vigor is in `commands()`.
    Travel(Travel),
}

pub struct Runtime {
    definitions: BTreeMap<u32, Definition>,
    skills: BTreeMap<(u64, u32), u64>,
    players: BTreeMap<u64, u64>,
    last_now: u64,
}
impl Runtime {
    /// A few words for the effect `did` of `skill` (see [`effect_label`]).
    pub fn effect_label(&self, skill: u32, did: u32) -> &str {
        self.definitions
            .get(&skill)
            .and_then(|d| d.applied.iter().find(|a| a.did == did))
            .map_or("Lingering effect", |a| a.label.as_str())
    }

    pub fn supports(&self, skill: u32) -> bool {
        self.definitions
            .get(&skill)
            .is_some_and(|d| !d.unresolved && d.travel.is_none())
    }

    pub fn supported(&self) -> usize {
        self.definitions
            .values()
            .filter(|d| !d.unresolved && d.travel.is_none())
            .count()
    }

    /// Reproducible content audit; a decoded record is not proof of executable behavior.
    pub fn coverage(&self) -> serde_json::Value {
        let mut counts = BTreeMap::<&str, usize>::new();
        let mut playable = 0;
        let rows: Vec<_> = crate::content_db::files::text("skills.tsv")
            .lines()
            .filter(|l| !l.starts_with('#') && l.contains('\t'))
            .map(|line| {
                let f: Vec<_> = line.split('\t').collect();
                let id = u32::from_str_radix(f[0], 16).unwrap();
                let race: u32 = f[4].parse().unwrap();
                let status = if id == crate::skills::LIFESTONE_RECALL {
                    "bound-lifestone recall"
                } else if race == 0 && matches!(id, 3 | 4 | 5 | 9 | 10 | 11 | 14 | 15 | 16) {
                    "passive mastery implemented"
                } else if self.supports(id) {
                    "executable core effects"
                } else if self
                    .definitions
                    .get(&id)
                    .is_some_and(|d| d.travel.is_some())
                {
                    "recall entitlement/destination pending"
                } else if self.definitions.contains_key(&id) {
                    "specialized effect handler pending"
                } else {
                    "no executable active effect decoded (includes passives)"
                };
                *counts.entry(status).or_default() += 1;
                if self.supports(id) && [1, 2, 4].contains(&race) {
                    playable += 1;
                }
                serde_json::json!({"skill": f[0], "record": f[1], "race": race, "status": status})
            })
            .collect();
        serde_json::json!({"scope": "Core single-target effects only. Multi-hit timing, area targeting, combos, stacking categories and specialized statuses are not fully recovered. Training and racial requirements still apply.",
            "counts": counts, "playable_race_core_profiles": playable, "skills": rows})
    }

    /// Helpful skills default to self when an enemy or scenery remains selected.
    /// A selected player remains explicit, including range and visibility checks.
    pub fn normalize_aim(&self, skill: u32, aim: Aim) -> Aim {
        if self
            .definitions
            .get(&skill)
            .is_some_and(|d| !d.harmful() && d.targets & SELF != 0)
            && matches!(aim, Aim::Creature(_) | Aim::Other)
        {
            Aim::Own
        } else {
            aim
        }
    }

    /// The skill's story behavior and FX, for presentation.
    pub fn presentation(&self, skill: u32) -> (u32, u32) {
        self.definitions
            .get(&skill)
            .map_or((0, 0), |d| (d.story, d.fx))
    }

    /// Expiry is safe during failed casts and disconnects. Never erase an
    /// unexpired entry when the same character logs out or changes targets.
    pub fn expire(&mut self, now: u64) -> Result<(), AbilityError> {
        if now < self.last_now {
            return Err(AbilityError::Clock);
        }
        self.last_now = now;
        self.skills.retain(|_, until| *until > now);
        self.players.retain(|_, until| *until > now);
        Ok(())
    }

}
mod load;
mod prepare;

/// Not Clone: a prepared cast can spend its validated cooldown exactly once.
/// Its mutable runtime borrow prevents another cast from invalidating capacity
/// or cooldown checks while the caller admits packets and persists progression.
pub struct PreparedCast<'a> {
    runtime: &'a mut Runtime,
    player: u64,
    skill: u32,
    story: u32,
    fx: u32,
    until: u64,
    global_until: u64,
    outcome: Outcome,
    commands: Vec<Command>,
    previous: Progression,
    next: Progression,
    caster_ratings: Option<(crate::combat::Ratings, u64)>,
    other_ratings: Option<(u64, crate::combat::Ratings, u64)>,
    applied: Vec<AppliedEffect>,
    creature_effects: Vec<(u32, u64)>,
}
impl PreparedCast<'_> {
    /// The timed effects the cast puts on players now (`Strike`'s target-side effects wait for
    /// the hit, `creature_effects`).
    pub fn applied(&self) -> &[AppliedEffect] {
        &self.applied
    }
    /// The target-side effects of a strike, up to two: the `Effect` state and its duration.
    pub fn creature_effects(&self) -> [(u32, u64); 2] {
        let mut out = [(0, 0); 2];
        for (slot, effect) in out.iter_mut().zip(&self.creature_effects) {
            *slot = *effect;
        }
        out
    }
    pub fn player(&self) -> u64 {
        self.player
    }
    pub fn skill(&self) -> u32 {
        self.skill
    }
    /// Story behavior and FX from the skill record (0 = none).
    pub fn presentation(&self) -> (u32, u32) {
        (self.story, self.fx)
    }
    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }
    pub fn previous(&self) -> &Progression {
        &self.previous
    }
    pub fn next(&self) -> &Progression {
        &self.next
    }
    pub fn caster_ratings(&self) -> Option<(crate::combat::Ratings, u64)> {
        self.caster_ratings
    }
    pub fn other_ratings(&self) -> Option<(u64, crate::combat::Ratings, u64)> {
        self.other_ratings
    }
    pub fn healed(&self) -> u32 {
        self.next.health - self.previous.health
    }
    pub fn ready_at(&self) -> u64 {
        self.until
    }
    /// Call only after the atomic progression batch has been durably saved.
    pub fn commit(self) {
        self.runtime
            .skills
            .insert((self.player, self.skill), self.until);
        self.runtime.players.insert(self.player, self.global_until);
    }
}

/// A harmful skill a creature's entity record gives it (`NPC_StartingSkill`), at its level: what
/// it puts on a player it fights. Only skills whose record targets players and takes health or
/// vigor over time, or lowers ratings, are kept.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatureAbility {
    pub skill: u32,
    /// The creature's level in the skill, which the record's tables are read at.
    pub level: u16,
    /// Health and vigor lost every `period_ms` for `duration_ms`.
    pub drain: Option<Over>,
    pub ratings: Option<(crate::combat::Ratings, u64)>,
    /// The effect states the player's strip lists: the state, its duration and whether it is
    /// pulsed.
    pub effects: Vec<(u32, u64, bool)>,
}

/// The harmful abilities of each creature entity, from the entity records' skills
/// (`content/7230/creature-skills.tsv`) and the skill records.
pub fn creature_abilities() -> BTreeMap<u32, Vec<CreatureAbility>> {
    // A creature's ability lasts what its record says; the development scale is for players' skills.
    let Ok(runtime) = Runtime::load_scaled(1.0) else {
        return BTreeMap::new();
    };
    let text = crate::data_profile::table(
        crate::content_db::files::text("creature-skills.tsv"),
        crate::content_db::files::text("7230/creature-skills.tsv"),
    );
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with("pyrealium-") {
            continue;
        }
        let Some((entity, skills)) = line.split_once('\t') else {
            continue;
        };
        let Ok(entity) = u32::from_str_radix(entity, 16) else {
            continue;
        };
        let mut list = Vec::new();
        for item in skills.split(',') {
            let Some((skill, level)) = item.split_once(':') else {
                continue;
            };
            let (Ok(skill), Ok(level)) = (u32::from_str_radix(skill, 16), level.parse::<u16>())
            else {
                continue;
            };
            if let Some(ability) = runtime.creature_ability(skill, level.max(1)) {
                list.push(ability);
            }
        }
        if !list.is_empty() {
            out.insert(entity, list);
        }
    }
    out
}

impl Runtime {
    fn creature_ability(&self, skill: u32, level: u16) -> Option<CreatureAbility> {
        let d = self.definitions.get(&skill)?;
        if d.unresolved || d.travel.is_some() || d.targets & PLAYER == 0 || !d.harmful() {
            return None;
        }
        let drain = d.timed.iter().find_map(|t| {
            let amount = -t.values.at(level);
            let seconds = t.duration.at(level);
            (t.side == Side::Target && amount > 0.5 && seconds > 0.0).then(|| Over {
                health: if t.vigor { 0 } else { amount.round() as u32 },
                vigor: if t.vigor { amount.round() as u32 } else { 0 },
                period_ms: t.period_ms,
                duration_ms: (seconds * 1000.0) as u64,
            })
        });
        let ratings = d
            .ratings_for(Side::Target, level)
            .filter(|(ratings, _)| !ratings.is_empty());
        if drain.is_none() && ratings.is_none() {
            return None;
        }
        let effects = d
            .applied
            .iter()
            .filter(|a| a.side == Side::Target)
            .map(|a| {
                (
                    a.did,
                    (a.duration.at(level).max(1.0) * 1000.0) as u64,
                    a.pulsed,
                )
            })
            .collect();
        Some(CreatureAbility {
            skill,
            level,
            drain,
            ratings,
            effects,
        })
    }
}

#[cfg(test)]
mod tests;
