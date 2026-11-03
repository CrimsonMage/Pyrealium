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
            include_str!("../../../content/skill-effects.report.json"),
            include_str!("../../../content/7230/skill-effects.report.json")))?;
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
    pub fn load() -> Result<Self, AbilityError> {
        Self::load_scaled(effect_scale())
    }

    /// The skill records at their own durations, whatever `PYREALIUM_DEV_EFFECT_SCALE` says.
    fn load_scaled(scale: f32) -> Result<Self, AbilityError> {
        Self::parse(
            crate::data_profile::table(include_str!("../../../content/skill-effects.tsv"), include_str!("../../../content/7230/skill-effects.tsv")),
            crate::data_profile::table(include_str!("../../../content/skill-travel.tsv"), include_str!("../../../content/7230/skill-travel.tsv")),
            scale,
        )
    }

    fn parse(text: &str, travel: &str, scale: f32) -> Result<Self, AbilityError> {
        let mut routes = BTreeMap::new();
        let mut travel_lines = travel.lines();
        if travel_lines.next() != Some("pyrealium-skill-travel-v1") {
            return Err(AbilityError::Catalog);
        }
        for line in travel_lines {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            if f.len() != 3 {
                return Err(AbilityError::Catalog);
            }
            let skill = u32::from_str_radix(f[0], 16).map_err(|_| AbilityError::Catalog)?;
            let route = match f[2] {
                "lifestone" => Travel::Lifestone,
                "portal" => Travel::Portal,
                list => {
                    let mut pads = Vec::new();
                    for pad in list.split(';') {
                        let p: Vec<_> = pad.split(':').collect();
                        if p.len() != 5 || pads.len() >= 32 {
                            return Err(AbilityError::Catalog);
                        }
                        let number =
                            |text: &str| text.parse::<f32>().map_err(|_| AbilityError::Catalog);
                        let placement = Placement {
                            cell: u32::from_str_radix(p[0], 16)
                                .map_err(|_| AbilityError::Catalog)?,
                            coordinates: [number(p[1])?, number(p[2])?, number(p[3])?],
                            heading: p[4].parse().map_err(|_| AbilityError::Catalog)?,
                        };
                        if !placement.valid() {
                            return Err(AbilityError::Catalog);
                        }
                        pads.push(placement);
                    }
                    Travel::Telepads(pads)
                }
            };
            routes.insert(skill, route);
        }

        let mut lines = text.lines();
        if lines.next() != Some("pyrealium-skill-effects-v2") {
            return Err(AbilityError::Catalog);
        }
        let mut definitions = BTreeMap::new();
        for line in lines {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            if f.len() != 13 {
                return Err(AbilityError::Catalog);
            }
            let hex = |text: &str| u32::from_str_radix(text, 16).map_err(|_| AbilityError::Catalog);
            let number = |text: &str| text.parse::<f32>().map_err(|_| AbilityError::Catalog);
            let skill = hex(f[0])?;
            let kind = hex(f[2])?;
            let reset = number(f[3])?;
            let vigor: i64 = f[4].parse().map_err(|_| AbilityError::Catalog)?;
            let range = number(f[5])?;
            if !(0.0..=3600.0).contains(&reset) || !(-1.0..=1000.0).contains(&range) {
                return Err(AbilityError::Catalog);
            }
            let mut definition = Definition {
                applied: Vec::new(),
                kind,
                reset_ms: (reset * 1000.0) as u64,
                vigor: u32::try_from(vigor.unsigned_abs().min(100_000)).unwrap(),
                // -1 means the record gives no range; strikes then use melee range.
                range: range.max(0.0),
                targets: hex(f[6])?,
                right: hex(f[7])?,
                left: hex(f[8])?,
                story: hex(f[9])?,
                fx: hex(f[10])?,
                damage: Table::parse(f[11])?,
                health: Table::default(),
                vigor_effect: Table::default(),
                timed: Vec::new(),
                buff: None,
                ratings: Vec::new(),
                unresolved: false,
                weapon: false,
                travel: routes.remove(&skill),
            };
            if f[12] != "-" {
                for effect in f[12].split(';') {
                    let p: Vec<_> = effect.split('/').collect();
                    if p.len() != 10 {
                        return Err(AbilityError::Catalog);
                    }
                    let side = if p[0] == "self" {
                        Side::Own
                    } else {
                        Side::Target
                    };
                    let kind = hex(p[2])?;
                    if definition.fx == 0 {
                        definition.fx = hex(p[4])?;
                    }
                    let period = number(p[5])?;
                    let duration = Table::parse(p[6])?.scaled(scale);
                    if !duration.is_empty() {
                        let stats = if p[8] == "-" {
                            Vec::new()
                        } else {
                            p[8].split('|')
                                .map(|s| {
                                    let (id, table) =
                                        s.split_once('=').ok_or(AbilityError::Catalog)?;
                                    Ok((hex(id)?, Table::parse(table)?))
                                })
                                .collect::<Result<Vec<_>, AbilityError>>()?
                        };
                        definition.applied.push(Applied {
                            side,
                            did: hex(p[1])?,
                            duration: duration.clone(),
                            pulsed: kind == HEALTH_OVER_TIME || kind == VIGOR_OVER_TIME,
                            label: effect_label(kind, &stats, &Table::parse(p[7])?),
                        });
                    }
                    let values = Table::parse(p[7])?;
                    // Do not spend vigor for a skill while silently dropping its
                    // essential status/summon component. Recognized stat effects
                    // occur under several derived native type codes.
                    let has_stats = p[8] != "-";
                    let stat_ids: Vec<u32> = if has_stats {
                        p[8].split('|')
                            .map(|s| {
                                s.split_once('=')
                                    .ok_or(AbilityError::Catalog)
                                    .and_then(|(id, _)| hex(id))
                            })
                            .collect::<Result<_, _>>()?
                    } else {
                        Vec::new()
                    };
                    let known_stats = has_stats
                        && stat_ids.iter().all(|id| {
                            [HEALTH_REGEN, VIGOR_REGEN, NATURAL_ARMOR, ARMOR_LEVEL].contains(id)
                                || ATTACK_RATING.contains(id)
                                || DEFENSE_RATING.contains(id)
                        });
                    let known_vital = match kind {
                        HEALTH | VIGOR => {
                            side == Side::Target && !values.is_empty()
                        }
                        HEALTH_OVER_TIME | VIGOR_OVER_TIME => {
                            !values.is_empty()
                                && !duration.is_empty()
                                && period.is_finite()
                                && period > 0.0
                                && (side == Side::Target
                                    || values.0.iter().all(|(_, v)| *v >= 0.0))
                        }
                        _ => false,
                    };
                    let container = kind == 0
                        && side == Side::Own
                        && duration.is_empty()
                        && !has_stats
                        && values.is_empty();
                    let weapon = kind == WEAPON_ATTACK
                        && !has_stats
                        && (definition.kind == BASIC_ATTACK_KIND || !definition.damage.is_empty());
                    if !(known_vital
                        || known_stats
                        || container
                        || weapon
                        || definition.travel.is_some())
                        || has_stats && !known_stats
                    {
                        definition.unresolved = true;
                    }
                    match kind {
                        HEALTH => {
                            definition.unresolved |= !definition.health.is_empty();
                            definition.health = values;
                        }
                        VIGOR => {
                            definition.unresolved |= !definition.vigor_effect.is_empty();
                            definition.vigor_effect = values
                        }
                        HEALTH_OVER_TIME | VIGOR_OVER_TIME
                            if !values.is_empty() && !duration.is_empty() && period > 0.0 =>
                        {
                            definition.timed.push(Timed {
                                side,
                                vigor: kind == VIGOR_OVER_TIME,
                                values,
                                period_ms: (period * 1000.0) as u64,
                                duration: duration.clone(),
                            });
                        }
                        WEAPON_ATTACK if definition.kind == BASIC_ATTACK_KIND => {
                            definition.weapon = true
                        }
                        _ => {}
                    }
                    if p[8] != "-" && !duration.is_empty()
                        || p[8].contains("40125")
                        || p[8].contains("40262")
                        || p[8].contains("80000")
                    {
                        if definition.buff.is_some()
                            && stat_ids
                                .iter()
                                .any(|id| [HEALTH_REGEN, VIGOR_REGEN].contains(id))
                        {
                            definition.unresolved = true;
                        }
                        let mut health = Table::default();
                        let mut vigor = Table::default();
                        let mut rating = RatingSpec {
                            side,
                            duration: duration.clone(),
                            armor: Vec::new(),
                            attack: [Vec::new(), Vec::new(), Vec::new()],
                            defense: [Vec::new(), Vec::new(), Vec::new()],
                        };
                        if p[8] != "-" {
                            for stat in p[8].split('|') {
                                let (id, table) =
                                    stat.split_once('=').ok_or(AbilityError::Catalog)?;
                                let id = hex(id)?;
                                let parsed = Table::parse(table)?;
                                match id {
                                    HEALTH_REGEN if definition.buff.is_none() => health = parsed,
                                    VIGOR_REGEN if definition.buff.is_none() => vigor = parsed,
                                    NATURAL_ARMOR | ARMOR_LEVEL => rating.armor.push(parsed),
                                    _ => {
                                        if let Some(index) =
                                            ATTACK_RATING.iter().position(|stat| *stat == id)
                                        {
                                            rating.attack[index].push(parsed);
                                        } else if let Some(index) =
                                            DEFENSE_RATING.iter().position(|stat| *stat == id)
                                        {
                                            rating.defense[index].push(parsed);
                                        }
                                    }
                                }
                            }
                        }
                        if definition.buff.is_none()
                            && (!health.is_empty() || !vigor.is_empty())
                            && !duration.is_empty()
                        {
                            definition.buff = Some(Buff {
                                side,
                                health,
                                vigor,
                                duration,
                            });
                        }
                        if rating.useful() {
                            definition.ratings.push(rating);
                        }
                    }
                }
            }
            // The executor currently carries one effect per vital, a shared
            // healing period, and one rating expiry per side. Withhold compound
            // records that would otherwise lose an effect or lengthen its life.
            for (index, a) in definition.timed.iter().enumerate() {
                for b in &definition.timed[index + 1..] {
                    if a.side != b.side
                        || a.vigor == b.vigor
                        || a.period_ms != b.period_ms
                        || a.duration != b.duration
                    {
                        definition.unresolved = true;
                    }
                }
            }
            for (index, a) in definition.ratings.iter().enumerate() {
                if definition.ratings[index + 1..]
                    .iter()
                    .any(|b| a.side == b.side && a.duration != b.duration)
                {
                    definition.unresolved = true;
                }
            }
            if definition.harmful()
                && (definition.buff.is_some()
                    || definition
                        .health
                        .0
                        .iter()
                        .chain(&definition.vigor_effect.0)
                        .any(|(_, v)| *v > 0.0)
                    || definition
                        .timed
                        .iter()
                        .any(|t| t.values.0.iter().any(|(_, v)| *v > 0.0)))
            {
                definition.unresolved = true;
            }
            if definition.usable() && definitions.insert(skill, definition).is_some() {
                return Err(AbilityError::Catalog);
            }
        }
        if definitions.is_empty() {
            return Err(AbilityError::Catalog);
        }
        Ok(Self {
            definitions,
            skills: BTreeMap::new(),
            players: BTreeMap::new(),
            last_now: 0,
        })
    }

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
        let rows: Vec<_> = include_str!("../../../content/skills.tsv")
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

    #[allow(clippy::too_many_arguments)]
    pub fn prepare<'a>(
        &'a mut self,
        player: u64,
        skill: u32,
        aim: Aim,
        hands: (Hand, Hand),
        book: &Book,
        state: &Progression,
        now: u64,
    ) -> Result<PreparedCast<'a>, AbilityError> {
        let definition = self
            .definitions
            .get(&skill)
            .ok_or(AbilityError::Unsupported)?
            .clone();
        if definition.unresolved {
            return Err(AbilityError::Unsupported);
        }
        let aim = self.normalize_aim(skill, aim);
        if player == 0 || player == u64::MAX {
            return Err(AbilityError::InvalidCharacter);
        }
        if !book.usable(skill) {
            return Err(AbilityError::NotLearned);
        }
        let rank = book.get(skill).ok_or(AbilityError::NotLearned)?.rank;
        if !state.valid() {
            return Err(AbilityError::InvalidProgression);
        }
        if !state.alive() {
            return Err(AbilityError::Dead);
        }
        if state.vigor < MIN_VIGOR && definition.vigor > 0 {
            return Err(AbilityError::InsufficientVigor);
        }
        if !holds(definition.right, hands.0) {
            return Err(AbilityError::Hand {
                right: true,
                required: definition.right,
            });
        }
        if !holds(definition.left, hands.1) {
            return Err(AbilityError::Hand {
                right: false,
                required: definition.left,
            });
        }
        let cost = definition.vigor.min(state.vigor);
        let over = |side: Side, vigor: bool, sign: f32| -> Option<Over> {
            definition
                .timed
                .iter()
                .find(|t| t.side == side && t.vigor == vigor && t.values.at(rank) * sign > 0.0)
                .map(|t| {
                    let amount = (t.values.at(rank) * sign).round() as u32;
                    Over {
                        health: if vigor { 0 } else { amount },
                        vigor: if vigor { amount } else { 0 },
                        period_ms: t.period_ms,
                        duration_ms: (t.duration.at(rank).max(0.0) * 1000.0) as u64,
                    }
                })
        };
        let combined_over = |side: Side, sign: f32| -> Option<Over> {
            let health = over(side, false, sign);
            let vigor = over(side, true, sign);
            match (health, vigor) {
                (Some(h), Some(v)) => Some(Over {
                    vigor: v.vigor,
                    ..h
                }),
                (h, v) => h.or(v),
            }
        };
        let regen_for = |side: Side| -> Option<Regen> {
            definition
                .buff
                .as_ref()
                .filter(|b| b.side == side)
                .map(|b| Regen {
                    health: if b.health.is_empty() {
                        1.0
                    } else {
                        b.health.at(rank)
                    },
                    vigor: if b.vigor.is_empty() {
                        1.0
                    } else {
                        b.vigor.at(rank)
                    },
                    duration_ms: (b.duration.at(rank).max(0.0) * 1000.0) as u64,
                })
        };
        let restore = || {
            (
                definition.health.at(rank).max(0.0).round() as u32,
                definition.vigor_effect.at(rank).max(0.0).round() as u32,
            )
        };
        for side in [Side::Own, Side::Target] {
            if regen_for(side).is_some_and(|r| {
                !(0.0..=100.0).contains(&r.health) || !(0.0..=100.0).contains(&r.vigor)
            }) {
                return Err(AbilityError::Unsupported);
            }
        }
        let caster_ratings = {
            let mut total = definition.ratings_for(Side::Own, rank);
            if matches!(aim, Aim::Own) {
                if let Some(targeted) = definition.ratings_for(Side::Target, rank) {
                    total = Some(match total {
                        Some((mut ratings, duration)) => {
                            ratings.add(targeted.0);
                            (ratings, duration.max(targeted.1))
                        }
                        None => targeted,
                    });
                }
            }
            total.filter(|(ratings, _)| !ratings.is_empty())
        };
        let other_ratings = match aim {
            Aim::Player(target, _) | Aim::Creature(target) => definition
                .ratings_for(Side::Target, rank)
                .filter(|(ratings, _)| !ratings.is_empty())
                .map(|(ratings, duration)| (target, ratings, duration)),
            _ => None,
        };
        let applied_for = |side_target: Option<u64>| -> Vec<AppliedEffect> {
            definition
                .applied
                .iter()
                .filter_map(|a| {
                    let entity = match (a.side, side_target) {
                        (Side::Own, _) => player,
                        (Side::Target, Some(target)) => target,
                        (Side::Target, None) => return None,
                    };
                    let seconds = a.duration.at(rank).max(1.0);
                    Some(AppliedEffect {
                        entity,
                        did: a.did,
                        duration_ms: (seconds * 1000.0) as u64,
                        power: f32::from(rank),
                        pulsed: a.pulsed,
                    })
                })
                .collect()
        };
        let applied = match aim {
            // A skill used on oneself puts its target-side effects on the caster too.
            Aim::Own => applied_for(Some(player)),
            Aim::Player(target, _) => applied_for(Some(target)),
            // A creature's effects land when the strike does (`Strike::effects`).
            Aim::Creature(_) => applied_for(None),
            Aim::Other => Vec::new(),
        };
        let outcome = if let Some(travel) = definition.travel.clone() {
            Outcome::Travel(travel)
        } else {
            match aim {
                Aim::Creature(target) if definition.harmful() => {
                    if definition.targets & MONSTER == 0 {
                        return Err(AbilityError::InvalidTarget);
                    }
                    let instant = definition.health.at(rank).min(0.0).abs();
                    let bonus = (definition.damage.at(rank).abs() + instant).round() as u32;
                    Outcome::Strike {
                        target,
                        bonus,
                        vigor_damage: definition.vigor_effect.at(rank).min(0.0).abs().round() as u32,
                        weapon_damage: definition.weapon || !definition.damage.is_empty(),
                        cost,
                        range: definition.range,
                        burn: combined_over(Side::Target, -1.0),
                    }
                }
                _ if definition.harmful() => return Err(AbilityError::TargetCreature),
                Aim::Own => {
                    if definition.targets & SELF == 0 {
                        return Err(AbilityError::InvalidTarget);
                    }
                    let (health, vigor) = restore();
                    Outcome::Restore {
                        health,
                        vigor,
                        mend: combined_over(Side::Target, 1.0).or_else(|| combined_over(Side::Own, 1.0)),
                        regen: regen_for(Side::Target).or_else(|| regen_for(Side::Own)),
                    }
                }
                Aim::Player(target, distance) => {
                    if definition.targets & PLAYER == 0 {
                        return Err(AbilityError::InvalidTarget);
                    }
                    if distance > definition.range.max(3.0) {
                        return Err(AbilityError::OutOfRange);
                    }
                    let (health, vigor) = restore();
                    Outcome::Support {
                        target,
                        health,
                        vigor,
                        mend: combined_over(Side::Target, 1.0),
                        regen: regen_for(Side::Target),
                    }
                }
                Aim::Creature(_) | Aim::Other => return Err(AbilityError::InvalidTarget),
            }
        };
        if let Outcome::Support {
            health: 0,
            vigor: 0,
            mend: None,
            regen: None,
            ..
        } = outcome
        {
            if caster_ratings.is_none() && other_ratings.is_none() {
                return Err(AbilityError::Unsupported);
            }
        }
        self.expire(now)?;
        if let Some(&until) = self.skills.get(&(player, skill)) {
            return Err(AbilityError::Cooldown {
                remaining_ms: until - now,
            });
        }
        if let Some(&until) = self.players.get(&player) {
            return Err(AbilityError::GlobalCooldown {
                remaining_ms: until - now,
            });
        }
        if self.skills.len() >= MAX_SKILL_COOLDOWNS || self.players.len() >= MAX_PLAYER_COOLDOWNS {
            return Err(AbilityError::Capacity);
        }
        let until = now
            .checked_add(definition.reset_ms.max(GLOBAL_COOLDOWN_MS))
            .ok_or(AbilityError::Clock)?;
        let global_until = now
            .checked_add(GLOBAL_COOLDOWN_MS)
            .ok_or(AbilityError::Clock)?;
        let mut commands = Vec::new();
        if !matches!(outcome, Outcome::Strike { .. }) && cost > 0 {
            commands.push(Command::SpendVigor(cost));
        }
        if let Outcome::Restore { health, vigor, .. } = outcome {
            if health > 0 {
                commands.push(Command::Heal(health));
            }
            if vigor > 0 {
                commands.push(Command::RecoverVigor(vigor));
            }
        }
        let next = commands.iter().try_fold(state.clone(), |value, command| {
            value
                .apply(command)
                .map_err(|_| AbilityError::InvalidProgression)
        })?;
        if let Outcome::Restore {
            mend: None,
            regen: None,
            ..
        } = outcome
        {
            if next.health == state.health
                && next.vigor + cost <= state.vigor
                && caster_ratings.is_none()
            {
                return Err(AbilityError::NothingToRestore);
            }
        }
        Ok(PreparedCast {
            runtime: self,
            player,
            skill,
            // Basic weapon attacks keep the validated training swing presentation.
            story: if definition.weapon {
                0
            } else {
                definition.story
            },
            fx: if definition.weapon { 0 } else { definition.fx },
            until,
            global_until,
            outcome,
            commands,
            previous: state.clone(),
            next,
            caster_ratings,
            other_ratings,
            applied,
            creature_effects: match aim {
                Aim::Creature(_) => definition
                    .applied
                    .iter()
                    .filter(|a| a.side == Side::Target)
                    .take(2)
                    .map(|a| (a.did, (a.duration.at(rank).max(1.0) * 1000.0) as u64))
                    .collect(),
                _ => Vec::new(),
            },
        })
    }
}

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
        include_str!("../../../content/creature-skills.tsv"),
        include_str!("../../../content/7230/creature-skills.tsv"),
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
mod tests {
    use super::*;

    fn caster() -> (Progression, Book) {
        let state = Progression::default()
            .apply(&Command::AwardXp(1000))
            .unwrap();
        let book = Book::default()
            .with_starter_skills(&state, 1, 0x1000)
            .unwrap()
            .apply(crate::skills::Command::Train(0x59), &state, 1)
            .unwrap();
        (state, book)
    }

    #[test]
    fn flame_bolt_validates_training_implement_and_target_before_committing() {
        let (state, book) = caster();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let mut runtime = Runtime::load().unwrap();
        assert!(matches!(
            runtime.prepare(
                1,
                0x59,
                Aim::Creature(2),
                hands,
                &Book::default(),
                &state,
                0
            ),
            Err(AbilityError::NotLearned)
        ));
        assert!(matches!(
            runtime.prepare(
                1,
                0x59,
                Aim::Creature(2),
                (Hand::Unknown, Hand::Empty),
                &book,
                &state,
                0
            ),
            Err(AbilityError::Hand { .. })
        ));
        for aim in [Aim::Own, Aim::Player(2, 2.0), Aim::Other] {
            assert!(matches!(
                runtime.prepare(1, 0x59, aim, hands, &book, &state, 0),
                Err(AbilityError::TargetCreature)
            ));
        }
        let cast = runtime
            .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap();
        assert_eq!(cast.presentation(), (0x410000b6, 0x41000241));
        assert_eq!(
            *cast.outcome(),
            Outcome::Strike {
                target: 2,
                bonus: 3,
                vigor_damage: 0,
                weapon_damage: true,
                cost: 32,
                range: 60.0,
                burn: Some(Over {
                    health: 1,
                    vigor: 0,
                    period_ms: 1000,
                    duration_ms: 30000
                })
            }
        );
        assert!(
            cast.commands().is_empty(),
            "a queued strike pays at impact, not twice"
        );
        drop(cast);
        // Queue/persistence failure must not spend the cooldown.
        runtime
            .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap()
            .commit();
        assert!(matches!(
            runtime.prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 999),
            Err(AbilityError::Cooldown { .. })
        ));
        assert!(
            runtime
                .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 1000)
                .is_ok()
        );
        let empty = state.apply(&Command::SpendVigor(state.vigor)).unwrap();
        assert!(matches!(
            runtime.prepare(1, 0x59, Aim::Creature(2), hands, &book, &empty, 1000),
            Err(AbilityError::InsufficientVigor)
        ));
    }

    #[test]
    fn healing_uses_native_cost_and_rank_and_does_not_heal_an_enemy() {
        let (state, book) = caster();
        let book = book
            .apply(crate::skills::Command::Train(0x55), &state, 1)
            .unwrap();
        let state = state.apply(&Command::Damage(50)).unwrap();
        let mut runtime = Runtime::load().unwrap();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let cast = runtime
            .prepare(1, 0x55, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap();
        assert!(matches!(
            cast.outcome(),
            Outcome::Restore { health: 25, .. }
        ));
        assert_eq!(cast.next().health, state.health + 25);
        assert_eq!(cast.next().vigor, state.vigor.saturating_sub(100));
        drop(cast);
        assert!(matches!(
            runtime.prepare(1, 0x55, Aim::Player(2, 41.0), hands, &book, &state, 0),
            Err(AbilityError::OutOfRange)
        ));
        let cast = runtime
            .prepare(1, 0x55, Aim::Player(2, 40.0), hands, &book, &state, 0)
            .unwrap();
        assert!(matches!(
            cast.outcome(),
            Outcome::Support {
                target: 2,
                health: 25,
                ..
            }
        ));
        assert_eq!(cast.next().health, state.health);
    }

    #[test]
    fn trained_native_vigor_attacks_keep_cost_target_and_rank_tables() {
        let state = Progression::default().apply(&Command::AwardXp(1000)).unwrap();
        let mut lugian = Book::default().with_starter_skills(&state, 2, 0x1000).unwrap();
        for skill in [0x11e, 0x110, 0x10f] {
            lugian = lugian.apply(crate::skills::Command::Train(skill), &state, 2).unwrap();
        }
        let mut runtime = Runtime::load().unwrap();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let cast = runtime.prepare(1, 0x10f, Aim::Creature(2), hands, &lugian, &state, 0).unwrap();
        assert!(matches!(cast.outcome(), Outcome::Strike {
            bonus: 7, vigor_damage: 40, weapon_damage: true, cost: 80, burn: None, ..
        }));
        assert!(cast.commands().is_empty(), "cost is paid once at impact");
        drop(cast);
        assert!(matches!(runtime.prepare(1, 0x10f, Aim::Player(2, 1.), hands, &lugian, &state, 0), Err(AbilityError::TargetCreature)));
        assert!(matches!(runtime.prepare(1, 0x10f, Aim::Own, hands, &lugian, &state, 0), Err(AbilityError::TargetCreature)));
        assert!(matches!(runtime.prepare(1, 0x10f, Aim::Creature(2), (Hand::Empty, Hand::Empty), &lugian, &state, 0), Err(AbilityError::Hand {..})));
        assert_eq!(runtime.definitions[&0x10f].vigor_effect.at(50), -400.);

        let mut tumerok = Book::default().with_starter_skills(&state, 4, 0x1000).unwrap();
        for skill in [0x2a1, 0x2a6] {
            tumerok = tumerok.apply(crate::skills::Command::Train(skill), &state, 4).unwrap();
        }
        let cast = runtime.prepare(1, 0x2a6, Aim::Creature(2), (Hand::Implement(8), Hand::Empty), &tumerok, &state, 0).unwrap();
        assert!(matches!(cast.outcome(), Outcome::Strike { vigor_damage: 0, cost: 50,
            burn: Some(Over {health: 0, vigor: 2, period_ms: 1000, duration_ms: 15000}), .. }));
        drop(cast);
        for skill in [0xb8, 0xd0, 0x10f, 0x116, 0x127, 0x16d, 0x29d, 0x2a6, 0x2d0, 0x317, 0x31a] {
            assert!(runtime.supports(skill), "native vigor skill {skill:x}");
        }
        // Enabling vigor alone must not admit a partially decoded compound spell.
        assert!(!runtime.supports(0x52));
    }

    #[test]
    fn effect_tables_reject_nonfinite_and_ambiguous_coordinates() {
        for table in ["1:NaN", "NaN:2", "1:2,1:3", "-1:2", "1:100001"] {
            assert!(Table::parse(table).is_err(), "{table}");
        }
    }

    #[test]
    fn unknown_effect_components_are_not_silently_omitted_from_a_paid_cast() {
        let mut runtime = Runtime::load().unwrap();
        // This native spell combines damage with an unresolved status record.
        assert!(!runtime.supports(0x52));
        assert!(matches!(
            runtime.prepare(
                1,
                0x52,
                Aim::Creature(2),
                (Hand::Implement(0x40000), Hand::Empty),
                &Book::default(),
                &Progression::default(),
                0
            ),
            Err(AbilityError::Unsupported)
        ));
        assert!(runtime.skills.is_empty() && runtime.players.is_empty());
    }

    #[test]
    fn retail_skill_records_drive_heal_and_strike_values() {
        let runtime = Runtime::load().unwrap();
        let ease = &runtime.definitions[&0x55];
        assert_eq!(
            (ease.reset_ms, ease.vigor, ease.range, ease.targets),
            (7000, 100, 40.0, 0x101)
        );
        assert_eq!((ease.health.at(1), ease.health.at(50)), (25.0, 250.0));
        let bolt = &runtime.definitions[&0x59];
        assert_eq!(
            (bolt.reset_ms, bolt.vigor, bolt.range, bolt.right, bolt.left),
            (1000, 32, 60.0, 0x40000, 1)
        );
        assert_eq!((bolt.damage.at(1), bolt.damage.at(50)), (3.0, 75.0));
        let burn = &bolt.timed[0];
        assert_eq!(
            (burn.values.at(1), burn.values.at(50), burn.period_ms),
            (-1.0, -4.0, 1000)
        );
        assert_eq!(burn.duration.at(1), 30.0);
        assert!(bolt.harmful() && !ease.harmful());
        let defense = runtime.definitions[&0x6b]
            .ratings_for(Side::Own, 1)
            .unwrap();
        assert_eq!(defense.0.attack(crate::combat::School::Melee), 7);
        assert_eq!(defense.1, 180_000);
        let aura = runtime.definitions[&0xa1]
            .ratings_for(Side::Target, 50)
            .unwrap();
        assert_eq!(aura.0.defense(crate::combat::School::Magic), 100);
        let calm = &runtime.definitions[&0x67];
        assert_eq!(
            calm.buff.as_ref().map(|b| (b.vigor.at(1), b.vigor.at(50))),
            Some((1.1, 2.0))
        );
        assert!(runtime.definitions[&0xb0].weapon);
        assert_eq!(runtime.definitions[&0x343].travel, Some(Travel::Lifestone));
        assert_eq!(runtime.definitions[&0x366].travel, Some(Travel::Portal));
        assert!(
            matches!(runtime.definitions[&0x39d].travel, Some(Travel::Telepads(ref pads)) if pads[0].cell == 0x6ea70025)
        );
    }

    #[test]
    fn hands_follow_the_implement_masks() {
        assert!(holds(0x40000, Hand::Implement(0x40000)));
        assert!(!holds(0x40000, Hand::Implement(0x2)));
        assert!(!holds(0x40000, Hand::Empty));
        assert!(holds(0x1, Hand::Empty));
        assert!(holds(0x80073, Hand::Empty));
        assert!(holds(0xa0062, Hand::Implement(0x2)));
        assert!(holds(0xffffffff, Hand::Empty));
        assert!(!holds(0xfffffffe, Hand::Empty));
        assert!(!holds(0x40000, Hand::Unknown));
    }

    #[test]
    fn table_interpolates_between_level_points_and_clamps_outside() {
        let table = Table::parse("1:3,50:75,150:500").unwrap();
        assert_eq!(table.at(0), 3.0);
        assert!((table.at(25) - 38.265).abs() < 0.01);
        assert_eq!(table.at(150), 500.0);
        assert_eq!(table.at(400), 500.0);
    }

    #[test]
    fn a_strike_names_the_timed_effects_it_puts_on_the_creature() {
        let (state, book) = caster();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let mut runtime = Runtime::load().unwrap();
        // Flame Bolt's burn is on the creature when the bolt lands, not on the player now.
        let cast = runtime
            .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap();
        assert!(cast.applied().is_empty());
        assert_eq!(cast.creature_effects(), [(0x7000_0776, 30_000), (0, 0)]);
        assert_eq!(
            runtime.effect_label(0x59, 0x7000_0776),
            "Burning (health lost over time)"
        );
    }
}
