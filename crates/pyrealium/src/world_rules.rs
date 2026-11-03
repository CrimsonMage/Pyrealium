//! Operator-authored world rules: custom creature statistics, behaviour, loot,
//! patrol routes and spawn groups. This is custom server content written by the
//! content editor next to a world overlay, never recovered retail data.
//!
//! A rule targets either a creature type (8 hex digits, `47xxxxxx`) or one
//! placed object (16 hex digits, `8140xxxxxxxxxxxx`). Placement rules win over
//! type rules, which win over the bundled tables. `-` keeps the inherited value.
use std::{collections::BTreeMap, fs::File, io::{self, Read}, path::Path};

pub const HEADER: &str = "pyrealium-world-rules-v1";
const MAX_BYTES: u64 = 4 * 1024 * 1024;

fn invalid(text: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, text.into()) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target { Entity(u32), Actor(u64) }
impl Target {
    fn parse(text: &str) -> io::Result<Self> {
        let value = u64::from_str_radix(text, 16).map_err(|_| invalid("rule target is not hexadecimal"))?;
        match text.len() {
            8 if value >> 24 == 0x47 => Ok(Self::Entity(value as u32)),
            16 if value >> 48 == 0x8140 && value != 0x8140_0000_0000_0000 => Ok(Self::Actor(value)),
            _ => Err(invalid("rule target must be a 47xxxxxx entity or an 8140… placement id")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub health: Option<u32>, pub damage: Option<u32>, pub armor: Option<u32>, pub level: Option<u32>,
    pub xp: Option<u64>, pub gold: Option<u32>, pub interval_ms: Option<u64>, pub range: Option<f32>,
    pub leash: Option<f32>, pub respawn_ms: Option<u64>,
}
impl Stats {
    /// Later (more specific) values replace earlier ones field by field.
    pub fn merged(self, other: Self) -> Self {
        Self { health: other.health.or(self.health), damage: other.damage.or(self.damage),
            armor: other.armor.or(self.armor), level: other.level.or(self.level), xp: other.xp.or(self.xp),
            gold: other.gold.or(self.gold), interval_ms: other.interval_ms.or(self.interval_ms),
            range: other.range.or(self.range), leash: other.leash.or(self.leash),
            respawn_ms: other.respawn_ms.or(self.respawn_ms) }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Behaviour {
    pub perception: Option<f32>, pub homesick: Option<f32>, pub wandering: Option<bool>,
    pub wander_range: Option<f32>, pub wander_probability: Option<f32>, pub aggressive: Option<bool>,
    pub idle_only: Option<bool>, pub loot_proof: Option<bool>,
}
impl Behaviour {
    pub fn merged(self, other: Self) -> Self {
        Self { perception: other.perception.or(self.perception), homesick: other.homesick.or(self.homesick),
            wandering: other.wandering.or(self.wandering), wander_range: other.wander_range.or(self.wander_range),
            wander_probability: other.wander_probability.or(self.wander_probability),
            aggressive: other.aggressive.or(self.aggressive), idle_only: other.idle_only.or(self.idle_only),
            loot_proof: other.loot_proof.or(self.loot_proof) }
    }
    pub fn apply(self, mut profile: crate::creature_ai::Profile) -> crate::creature_ai::Profile {
        if let Some(v) = self.perception { profile.perception = v; }
        if let Some(v) = self.homesick { profile.homesick = Some(v); }
        if let Some(v) = self.wandering { profile.wandering = Some(v); }
        if let Some(v) = self.wander_range { profile.wander_range = v; }
        if let Some(v) = self.wander_probability { profile.probability = Some(v); }
        if let Some(v) = self.aggressive {
            profile.free_attack = Some(v);
            // An explicit choice also overrides the friend-table heuristic.
            profile.custom_aggressive = Some(v);
        }
        if let Some(v) = self.idle_only { profile.idle_only = v; }
        if let Some(v) = self.loot_proof { profile.loot_proof = v; }
        profile
    }
}

/// One weighted outcome. An empty item list is an explicit "no drop" outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct LootOutcome { pub weight: u32, pub items: Vec<u32> }

#[derive(Clone, Debug, PartialEq)]
pub struct Patrol {
    pub pause_ms: u64,
    /// Waypoint offsets in metres from the creature's spawn point.
    pub waypoints: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group { pub id: u64, pub capacity: usize, pub regeneration_ms: Option<u64>, pub members: Vec<u64> }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub stats: BTreeMap<Target, Stats>,
    pub behaviour: BTreeMap<Target, Behaviour>,
    pub loot: BTreeMap<Target, Vec<LootOutcome>>,
    pub patrols: BTreeMap<u64, Patrol>,
    pub groups: Vec<Group>,
}

impl Rules {
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES { return Err(invalid("world rules exceed 4 MiB")); }
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid("world rules are not UTF-8"))?;
        Self::parse(text).map_err(|error| invalid(format!("{}: {error}", path.display())))
    }

    pub fn parse(text: &str) -> io::Result<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) { return Err(invalid("world rules header")); }
        let mut rules = Self::default();
        for (index, line) in lines.enumerate() {
            if line.is_empty() || line.starts_with('#') { continue; }
            rules.row(line).map_err(|error| invalid(format!("line {}: {error}", index + 2)))?;
        }
        if rules.stats.len() + rules.behaviour.len() + rules.loot.len() + rules.patrols.len() + rules.groups.len() > 65536 {
            return Err(invalid("world rule count limit"));
        }
        Ok(rules)
    }

    /// Every type or placement the operator has given custom statistics.
    pub fn stats_for(&self, entity: u32, actor: u64) -> Option<Stats> {
        let base = self.stats.get(&Target::Entity(entity)).copied();
        let own = self.stats.get(&Target::Actor(actor)).copied();
        match (base, own) { (None, None) => None, (a, b) => Some(a.unwrap_or_default().merged(b.unwrap_or_default())) }
    }
    pub fn behaviour_for(&self, entity: u32, actor: u64) -> Behaviour {
        self.behaviour.get(&Target::Entity(entity)).copied().unwrap_or_default()
            .merged(self.behaviour.get(&Target::Actor(actor)).copied().unwrap_or_default())
    }
    pub fn loot_for(&self, entity: u32, actor: u64) -> Option<&[LootOutcome]> {
        self.loot.get(&Target::Actor(actor)).or_else(|| self.loot.get(&Target::Entity(entity))).map(Vec::as_slice)
    }
    pub fn loot_definitions(&self) -> impl Iterator<Item = u32> + '_ {
        self.loot.values().flatten().flat_map(|outcome| outcome.items.iter().copied())
    }

    fn row(&mut self, line: &str) -> io::Result<()> {
        if line.len() > 4096 { return Err(invalid("row exceeds 4096 bytes")); }
        let f: Vec<_> = line.split('\t').collect();
        let keep = |i: usize| f[i] == "-" || f[i].is_empty();
        let int = |i: usize, max: u64| -> io::Result<Option<u64>> {
            if keep(i) { return Ok(None); }
            let value = f[i].parse::<u64>().map_err(|_| invalid(format!("field {} must be a whole number", i + 1)))?;
            if value > max { return Err(invalid(format!("field {} exceeds {max}", i + 1))); }
            Ok(Some(value))
        };
        let real = |i: usize, low: f32, high: f32| -> io::Result<Option<f32>> {
            if keep(i) { return Ok(None); }
            let value = f[i].parse::<f32>().map_err(|_| invalid(format!("field {} must be a number", i + 1)))?;
            if !value.is_finite() || !(low..=high).contains(&value) {
                return Err(invalid(format!("field {} must be within {low}..{high}", i + 1)));
            }
            Ok(Some(value))
        };
        let flag = |i: usize| -> io::Result<Option<bool>> {
            match f[i] { "-" | "" => Ok(None), "0" => Ok(Some(false)), "1" => Ok(Some(true)),
                _ => Err(invalid(format!("field {} must be 0, 1 or -", i + 1))) }
        };
        match f.as_slice() {
            ["stats", target, ..] if f.len() == 12 => {
                let stats = Stats {
                    health: int(2, 5_000_000)?.map(|v| v as u32), damage: int(3, 5_000_000)?.map(|v| v as u32),
                    armor: int(4, 1_000_000)?.map(|v| v as u32), level: int(5, 999)?.map(|v| v as u32),
                    xp: int(6, i64::MAX as u64)?, gold: int(7, i32::MAX as u64)?.map(|v| v as u32),
                    interval_ms: int(8, 60_000)?, range: real(9, 0.5, 30.)?, leash: real(10, 1., 500.)?,
                    respawn_ms: int(11, 86_400_000)?,
                };
                if stats.health == Some(0) || stats.damage == Some(0) { return Err(invalid("health and damage must be above zero")); }
                if stats.interval_ms.is_some_and(|v| v < 250) { return Err(invalid("attack interval must be at least 250 ms")); }
                if stats.respawn_ms.is_some_and(|v| v < 1000) { return Err(invalid("respawn must be at least 1000 ms")); }
                if self.stats.insert(Target::parse(target)?, stats).is_some() { return Err(invalid("duplicate stats target")); }
            }
            ["ai", target, ..] if f.len() == 10 => {
                let behaviour = Behaviour {
                    perception: real(2, 0., 500.)?, homesick: real(3, 0., 500.)?, wandering: flag(4)?,
                    wander_range: real(5, 0., 500.)?, wander_probability: real(6, 0., 1.)?,
                    aggressive: flag(7)?, idle_only: flag(8)?, loot_proof: flag(9)?,
                };
                if self.behaviour.insert(Target::parse(target)?, behaviour).is_some() { return Err(invalid("duplicate ai target")); }
            }
            ["loot", target, weight, items] => {
                let weight = weight.parse::<u32>().ok().filter(|w| (1..=1_000_000).contains(w))
                    .ok_or_else(|| invalid("loot weight must be 1..1000000"))?;
                let items = if *items == "-" { Vec::new() } else {
                    items.split(',').map(|id| u32::from_str_radix(id, 16).ok().filter(|id| id >> 24 == 0x47)
                        .ok_or_else(|| invalid("loot items must be 47xxxxxx definitions"))).collect::<io::Result<Vec<_>>>()?
                };
                if items.len() > 32 { return Err(invalid("a loot outcome holds at most 32 items")); }
                let table = self.loot.entry(Target::parse(target)?).or_default();
                if table.len() >= 256 { return Err(invalid("a loot table holds at most 256 outcomes")); }
                table.push(LootOutcome { weight, items });
            }
            ["patrol", actor, pause, points] => {
                let Target::Actor(actor) = Target::parse(actor)? else { return Err(invalid("a patrol belongs to one placed creature")); };
                let pause_ms = pause.parse::<u64>().ok().filter(|v| *v <= 600_000).ok_or_else(|| invalid("patrol pause must be 0..600000 ms"))?;
                let waypoints = points.split(';').map(|point| {
                    let (x, y) = point.split_once(',').ok_or_else(|| invalid("waypoints are dx,dy pairs"))?;
                    let value = |s: &str| s.parse::<f32>().ok().filter(|v| v.is_finite() && v.abs() <= 500.)
                        .ok_or_else(|| invalid("waypoint offsets must be within 500 m of the spawn"));
                    Ok([value(x)?, value(y)?])
                }).collect::<io::Result<Vec<_>>>()?;
                if waypoints.is_empty() || waypoints.len() > 64 { return Err(invalid("a patrol has 1..64 waypoints")); }
                if self.patrols.insert(actor, Patrol { pause_ms, waypoints }).is_some() { return Err(invalid("duplicate patrol")); }
            }
            ["group", id, capacity, regeneration, members] => {
                let id = u64::from_str_radix(id, 16).ok().filter(|id| *id != 0 && *id < (1u64 << 63) - 1).ok_or_else(|| invalid("group id"))?;
                let capacity = capacity.parse::<usize>().ok().filter(|c| (1..=64).contains(c)).ok_or_else(|| invalid("group capacity must be 1..64"))?;
                let regeneration_ms = if *regeneration == "-" { None } else {
                    Some(regeneration.parse::<u64>().ok().filter(|v| (1000..=86_400_000).contains(v)).ok_or_else(|| invalid("group respawn must be 1000..86400000 ms"))?)
                };
                let members = members.split(',').map(|m| match Target::parse(m)? {
                    Target::Actor(id) => Ok(id), Target::Entity(_) => Err(invalid("group members are placement ids")),
                }).collect::<io::Result<Vec<_>>>()?;
                if members.is_empty() || members.len() > 64 { return Err(invalid("a group has 1..64 members")); }
                if members.iter().collect::<std::collections::BTreeSet<_>>().len()!=members.len() || self.groups.iter().any(|g| g.id == id || g.members.iter().any(|m| members.contains(m))) {
                    return Err(invalid("duplicate group or a member in two groups"));
                }
                self.groups.push(Group { id, capacity: capacity.min(members.len()), regeneration_ms, members });
            }
            _ => return Err(invalid(format!("unrecognized rule row `{}`", f[0]))),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = "pyrealium-world-rules-v1\n# comment\n\
stats\t470007d1\t500\t40\t-\t20\t-\t-\t-\t-\t-\t-\n\
stats\t8140000000020001\t-\t60\t-\t-\t900\t-\t-\t-\t40\t5000\n\
ai\t8140000000020001\t25\t-\t1\t15\t0.5\t1\t-\t-\n\
loot\t8140000000020001\t70\t47000ed7,47000ee1\n\
loot\t8140000000020001\t30\t-\n\
patrol\t8140000000020001\t2000\t10,0;10,10;0,10\n\
group\t1\t2\t60000\t8140000000020001,8140000000020002,8140000000020003\n";

    #[test]
    fn placement_rules_override_type_rules_field_by_field() {
        let rules = Rules::parse(SAMPLE).unwrap();
        let stats = rules.stats_for(0x470007d1, 0x8140000000020001).unwrap();
        assert_eq!((stats.health, stats.damage, stats.level, stats.xp, stats.leash), (Some(500), Some(60), Some(20), Some(900), Some(40.)));
        assert_eq!(rules.stats_for(0x470007d1, 0x8140000000020009).unwrap().damage, Some(40));
        assert!(rules.stats_for(0x47000001, 0x8140000000020009).is_none());
        let table = rules.loot_for(0x470007d1, 0x8140000000020001).unwrap();
        assert_eq!(table.len(), 2);
        assert!(table[1].items.is_empty());
        assert!(rules.loot_for(0x470007d1, 0x8140000000020002).is_none());
        assert_eq!(rules.patrols[&0x8140000000020001].waypoints, vec![[10., 0.], [10., 10.], [0., 10.]]);
        assert_eq!(rules.groups[0].capacity, 2);
        let ai = rules.behaviour_for(0x470007d1, 0x8140000000020001).apply(Default::default());
        assert!(ai.wanders() && ai.aggressive());
    }

    #[test]
    fn malformed_rows_are_rejected() {
        for row in ["stats\t470007d1\t0\t1\t-\t-\t-\t-\t-\t-\t-\t-", "ai\t12345678\t1\t-\t-\t-\t-\t-\t-\t-",
            "loot\t470007d1\t0\t47000ed7", "loot\t470007d1\t5\t1f000001", "patrol\t470007d1\t0\t1,1",
            "patrol\t8140000000020001\t0\t1;1", "group\t1\t1\t-\t470007d1", "bogus\tx",
            "group\t8000000000000001\t1\t-\t8140000000020001",
            "group\t1\t1\t-\t8140000000020001,8140000000020001",
            "stats\t470007d1\t1\t1\t-\t-\t-\t-\t10\t-\t-\t-"] {
            assert!(Rules::parse(&format!("{HEADER}\n{row}\n")).is_err(), "{row}");
        }
        assert!(Rules::parse(&format!("{HEADER}\nai\t470007d1\t-\t-\t-\t-\t-\t-\t-\t-\nai\t470007d1\t-\t-\t-\t-\t-\t-\t-\t-\n")).is_err());
    }
}
