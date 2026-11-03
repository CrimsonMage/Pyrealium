//! Tradeskills: Surveying, Mining and the crafting skills built on the DAT's recipe records.
//!
//! Retail data traced from the 7230 portal DAT (nothing here is captured server behaviour):
//! * a craft skill is a `0x6f00xxxx` state (`craftskill*.wc`); Surveying is `0x6f00264a`
//!   (string `craftskills.stt` 23695d5a "Surveying", "This skill allows the player to survey for
//!   new raw materials"), Mining Iron/Silver/Stone/Crystal/Wood are `0x6f002579`, `0x6f0025a6`,
//!   `0x6f0025a7`, `0x6f0025a8`, `0x6f0025a9`, Toolmaking is `0x6f002857`;
//! * a recipe is a `0x6f00xxxx` object graph (package 0x1d7): its craft skill DID, its ingredient
//!   slots (packages 0x1d4: ordinal, quantity, allowed item definitions) and its outcomes
//!   (packages 0x1d1 with the roll threshold out of 1000, 0x144 with the item created);
//! * a Surveying recipe (`rcp_surveying_<region>_<material>`) creates one of six surveyor's maps
//!   (`wsl_surveyorsmap_<region>_<material>N`, 0x47002e8e..); using a map in the right place
//!   prospects a deposit (`wsl_mine_<material>_<region>`) that only its surveyor may work, with
//!   the matching tool wielded (chisel, pickaxe, mattock, rock hammer, hatchet), yielding motes.
//!
//! What retail's server decided and the DAT does not state is development policy and is marked
//! "Policy" below.
use crate::dat::Dat;
use std::collections::BTreeMap;

pub const SURVEYING: u32 = 0x6f00264a;
pub const TOOLMAKING: u32 = 0x6f002857;
/// `CraftSkillButchering` (type 11): "butcher ingredients from monster corpses". It has no recipes.
pub const BUTCHERY: u32 = 0x6f002560;
/// The five refining skills (types 19..23). Their descriptions name the pair they refine:
/// "Refining: Dramastic ... refine Iron and Silver into Dramastic" and so on.
pub const REFINING_DRAMASTIC: u32 = 0x6f00283c;
pub const REFINING_MOONGLASS: u32 = 0x6f00283d;
pub const REFINING_RUNEWOOD: u32 = 0x6f00283e;
pub const REFINING_STEEL: u32 = 0x6f00283f;
pub const REFINING_TUKALITE: u32 = 0x6f002840;
/// Craft skills that gather: iron, silver, stone, crystal, wood.
pub const MINING_IRON: u32 = 0x6f002579;
pub const MINING_SILVER: u32 = 0x6f0025a6;
pub const MINING_STONE: u32 = 0x6f0025a7;
pub const MINING_CRYSTAL: u32 = 0x6f0025a8;
pub const MINING_WOOD: u32 = 0x6f0025a9;

/// The materials in the order of the surveyor's map and recipe families (crystal, iron, silver,
/// stone, wood), with the mining skill, the tool and the mote each yields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Material {
    pub name: &'static str,
    pub skill: u32,
    /// `wsl_toolnew_*` (chisel, pickaxe, mattock, rock hammer, hatchet).
    pub tool: u32,
    /// `wsl_toolnew_*_starter`.
    pub starter_tool: u32,
    /// `wsl_mote_*`.
    pub mote: u32,
    /// `wsl_mine_<material>_<region>` for Linvak, Omishan and Osteth.
    pub deposits: [u32; 3],
}
pub const MATERIALS: [Material; 5] = [
    Material { name: "crystal", skill: MINING_CRYSTAL, tool: 0x47002de4, starter_tool: 0x47002f71, mote: 0x470010f2, deposits: [0x47002ef1, 0x47002ef0, 0x47002c68] },
    Material { name: "iron", skill: MINING_IRON, tool: 0x47002deb, starter_tool: 0x47002f75, mote: 0x470010f3, deposits: [0x47002eef, 0x47002eee, 0x47002c69] },
    Material { name: "silver", skill: MINING_SILVER, tool: 0x47002dea, starter_tool: 0x47002f74, mote: 0x470010fe, deposits: [0x47002ef3, 0x47002ef2, 0x47002c6a] },
    Material { name: "stone", skill: MINING_STONE, tool: 0x47002dec, starter_tool: 0x47002f76, mote: 0x470010f4, deposits: [0x47002ef5, 0x47002ef4, 0x47002c6b] },
    Material { name: "wood", skill: MINING_WOOD, tool: 0x47002de7, starter_tool: 0x47002f72, mote: 0x470010f5, deposits: [0x47002ef7, 0x47002ef6, 0x47002c6c] },
];
const FIRST_MAP: u32 = 0x47002e8e;
const MAPS_PER_FAMILY: u32 = 6;

/// The material and region (0 Linvak, 1 Omishan, 2 Osteth) of a surveyor's map definition.
pub fn map_family(definition: u32) -> Option<(&'static Material, usize)> {
    let group = definition.checked_sub(FIRST_MAP)? / MAPS_PER_FAMILY;
    let (region, material) = (group as usize / 5, group as usize % 5);
    (region < 3).then(|| (&MATERIALS[material], region))
}
/// The newer resource family (granite, gypsum, kaolin, oak, slate): its surveyor's maps
/// (`wsl_surveyorsmap_<region>_<name>N`, seven ids a name for Linvak from `0x470035d3`, six for Omishan and Osteth, the
/// second of each seven being the unnamed `it_craft_map_*` template) reveal `wsl_mine_<name>_<region>`
/// deposits (`0x47003645..=0x47003658`), whose entities name the Mining Stone or Wood skill and
/// the mote they yield. The recipes are `rcp_surveying_<region>_<name>` (`0x6f003690..=0x6f00369e`).
pub const NEW_FIRST_MAP: u32 = 0x4700_35d3;
const NEW_DEPOSITS: [[u32; 5]; 3] = [
    [0x4700_3645, 0x4700_3647, 0x4700_3649, 0x4700_364b, 0x4700_364d],
    [0x4700_364f, 0x4700_3650, 0x4700_3651, 0x4700_3652, 0x4700_3653],
    [0x4700_3654, 0x4700_3655, 0x4700_3656, 0x4700_3657, 0x4700_3658],
];
/// The deposit entity a surveyor's map of either family reveals.
pub fn map_deposit(definition: u32) -> Option<u32> {
    if let Some((material, region)) = map_family(definition) {
        return Some(material.deposits[region]);
    }
    // Linvak's five names have seven ids each (the second a template), Omishan's and Osteth's
    // have six (`wsl_surveyorsmap_omishan_granite1` is `0x470035f6`, `osteth_granite1` `0x47003614`).
    if let Some(offset) = definition.checked_sub(NEW_FIRST_MAP).filter(|o| *o < 35) {
        return (offset % 7 != 1).then(|| NEW_DEPOSITS[0][offset as usize / 7]);
    }
    let offset = definition.checked_sub(0x4700_35f6).filter(|o| *o < 60)?;
    let group = offset as usize / 6;
    Some(NEW_DEPOSITS[1 + group / 5][group % 5])
}
pub fn new_family_deposits() -> impl Iterator<Item = u32> {
    NEW_DEPOSITS.into_iter().flatten()
}

/// The material a deposit definition holds, and its region.
pub fn deposit_family(definition: u32) -> Option<(&'static Material, usize)> {
    MATERIALS
        .iter()
        .find_map(|m| m.deposits.iter().position(|d| *d == definition).map(|r| (m, r)))
}
pub fn material_for_tool(definition: u32) -> Option<&'static Material> {
    MATERIALS
        .iter()
        .find(|m| m.tool == definition || m.starter_tool == definition)
}

/// One decoded object of a recipe graph.
#[derive(Debug, Clone)]
pub struct Obj {
    pub pkg: u32,
    pub words: Vec<u32>,
    pub tags: Vec<u8>,
}

/// Walk a `0x6f00xxxx` record: `[did, size, count, ids 0..count]` then the objects. An
/// interpreted object is `[magic, 0, package, n, n words, n tag bytes padded]`; a native one
/// (package `0xffff00xx`) is a hash (`0x01`, `0x08`, `0x11`: header, pairs, 0), an array (`0x0a`,
/// `0x65`: count, items, 0) or a string (`0x6a`: four words); `0x24000100` is a state reference.
pub fn parse(bytes: &[u8]) -> Option<Vec<Obj>> {
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let count = *words.get(2)? as usize;
    if count > 4096 {
        return None;
    }
    let mut pos = 3 + count;
    let mut objects = Vec::new();
    let take = |pos: &mut usize, n: usize| -> Option<Vec<u32>> {
        let slice = words.get(*pos..*pos + n)?.to_vec();
        *pos += n;
        Some(slice)
    };
    while objects.len() < count {
        let magic = *words.get(pos)?;
        pos += 1;
        if magic == 0x24000100 {
            let w = take(&mut pos, 1)?;
            objects.push(Obj { pkg: 0x2400_0000, words: w, tags: Vec::new() });
            continue;
        }
        if magic & 0xffff != 0x0100 {
            return None;
        }
        pos += 1;
        let pkg = *words.get(pos)?;
        pos += 1;
        if pkg >> 16 == 0xffff {
            let w = match pkg & 0xffff {
                0x01 | 0x08 | 0x11 => {
                    let header = *words.get(pos)?;
                    take(&mut pos, 1 + 2 * (header & 0xffffff) as usize + 1)?
                }
                0x0a | 0x65 => {
                    let n = *words.get(pos)? as usize;
                    take(&mut pos, 1 + n + 1)?
                }
                0x6a => take(&mut pos, 4)?,
                _ => return None,
            };
            objects.push(Obj { pkg, words: w, tags: Vec::new() });
        } else {
            let n = *words.get(pos)? as usize;
            pos += 1;
            let w = take(&mut pos, n)?;
            let tag_words = take(&mut pos, n.div_ceil(4))?;
            let tags = tag_words
                .iter()
                .flat_map(|t| t.to_le_bytes())
                .take(n)
                .collect();
            objects.push(Obj { pkg, words: w, tags });
        }
    }
    Some(objects)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The roll (0..1000) below which this outcome is chosen (package 0x1d1).
    pub threshold: f32,
    /// The item definition created (package 0x144).
    pub output: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ingredient {
    pub ordinal: u32,
    pub quantity: u32,
    /// The item definitions the slot accepts (the 0x1d4 object's hash).
    pub allowed: Vec<u32>,
}
#[derive(Clone, Debug)]
pub struct Recipe {
    pub id: u32,
    pub skill: u32,
    /// The craft skill level the recipe needs (the root's field 12: 0 for surveying, 5 for the
    /// pickaxe, 2 for the steel refinings and the potions).
    pub min_level: u32,
    pub outcomes: Vec<Outcome>,
    pub ingredients: Vec<Ingredient>,
}

/// Decode one recipe record; `None` when it is not a package 0x1d7 recipe with a craft skill.
pub fn recipe(id: u32, bytes: &[u8]) -> Option<Recipe> {
    let objects = parse(bytes)?;
    let root = objects.first().filter(|o| o.pkg & 0xffff == 0 && o.pkg >> 16 != 0xffff)?;
    let skill = root.words.iter().copied().find(|w| w >> 24 == 0x6f)?;
    let mut outcomes = Vec::new();
    let mut ingredients = Vec::new();
    let mut threshold = 1000.0f32;
    for object in &objects {
        match object.pkg {
            0x01d1_0000 => threshold = f32::from_bits(*object.words.get(2)?),
            0x0144_0000 => {
                if let Some(output) = object.words.iter().copied().find(|w| w >> 24 == 0x47) {
                    outcomes.push(Outcome { threshold, output });
                }
            }
            0x01d4_0000 => {
                let mut allowed = Vec::new();
                for (value, tag) in object.words.iter().zip(&object.tags) {
                    if *tag != 1 {
                        continue;
                    }
                    if let Some(hash) = objects
                        .get(*value as usize)
                        .filter(|o| o.pkg == 0xffff_0001)
                    {
                        let n = (hash.words[0] & 0xffffff) as usize;
                        allowed.extend((0..n).map(|i| hash.words[1 + 2 * i]));
                    }
                }
                ingredients.push(Ingredient {
                    ordinal: *object.words.first()?,
                    // Field 4 (pickaxe: 20 coal) or field 12 (refining: 100; the potions' 100
                    // Tallow): whichever the record fills.
                    quantity: (*object.words.get(4)?).max(*object.words.get(12)?),
                    allowed,
                });
            }
            _ => {}
        }
    }
    let min_level = *root.words.get(12)?;
    Some(Recipe { id, skill, min_level, outcomes, ingredients })
}

/// Every recipe of the portal DAT, by DID.
#[derive(Default)]
pub struct Catalog {
    recipes: BTreeMap<u32, Recipe>,
}
impl Catalog {
    pub fn load(dat: &mut Dat) -> Self {
        let mut recipes = BTreeMap::new();
        for id in 0x6f00_1000u32..0x6f00_6300 {
            if let Some(r) = dat.read(id).ok().and_then(|bytes| recipe(id, &bytes)) {
                recipes.insert(id, r);
            }
        }
        Self { recipes }
    }
    pub fn get(&self, id: u32) -> Option<&Recipe> {
        self.recipes.get(&id)
    }
    pub fn of_skill(&self, skill: u32) -> impl Iterator<Item = &Recipe> {
        self.recipes.values().filter(move |r| r.skill == skill)
    }
    pub fn len(&self) -> usize {
        self.recipes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

use crate::{
    crafting::{self, SkillEntry},
    inventory::{Item, Location},
    inventory_request::Metadata,
};

/// Every craft skill the 7230 portal DAT defines, in `craftskilltype.emp` order (admin 7148's
/// enum, `CraftSkillHumanArmor` = 2 .. `CraftSkillToolmaking` = 24; 25 is Quest; the 7230 DAT adds
/// 26..29, Empyrean armour and orbs, Drudge armour and boards). Each is a package 0x33af state whose
/// first field is that type number; its second field is the base experience scale (150 for the
/// gathering and refining skills, 500 for the crafting ones). Names are the skills' own strings
/// (string table 25007994).
pub const CRAFT_SKILLS: [u32; 27] = [
    0x6f00_23a8, // 2 Armor: Human
    0x6f00_23aa, // 3 Armor: Lugian
    0x6f00_23a9, // 4 Armor: Tumerok
    0x6f00_2565, // 5 Weapons: Human Martial
    0x6f00_256d, // 6 Weapons: Human Exotic
    0x6f00_256e, // 7 Weapons: Lugian Exotic
    0x6f00_256f, // 8 Weapons: Lugian Martial
    0x6f00_2570, // 9 Weapons: Tumerok Exotic
    0x6f00_2571, // 10 Weapons: Tumerok Martial
    BUTCHERY,    // 11
    MINING_IRON, // 12
    MINING_CRYSTAL, // 13
    MINING_SILVER, // 14
    MINING_STONE, // 15
    MINING_WOOD, // 16
    SURVEYING,   // 17 CraftSkillCartography
    0x6f00_282d, // 18 Spellbinding
    REFINING_DRAMASTIC, // 19
    REFINING_MOONGLASS, // 20
    REFINING_RUNEWOOD, // 21
    REFINING_STEEL, // 22
    REFINING_TUKALITE, // 23
    TOOLMAKING,  // 24
    0x6f00_5242, // 26 Armor: Empyrean
    0x6f00_5276, // 27 Weapons: Empyrean Exotic
    0x6f00_5bb4, // 28 Armor: Drudge
    0x6f00_5bb5, // 29 Weapons: Drudge Martial
];
/// The craft skills the registry lists, in panel order: the quest skill first (its two potion
/// recipes), then every craft skill.
pub const SKILL_ORDER: [u32; 28] = {
    let mut order = [crafting::CRAFT_SKILL_DID; 28];
    let mut i = 0;
    while i < CRAFT_SKILLS.len() {
        order[i + 1] = CRAFT_SKILLS[i];
        i += 1;
    }
    order
};

/// Craft experience and ranks of one character. The skill record the client reads carries the
/// skill's accumulated crafting experience (its second long: with 0 the client's
/// `CanCraftSkillBeRaised` is false, with 50 it is true at rank 1 where the raise costs 42) and its
/// level. The rank thresholds are the Pyreal dump's cumulative curve (`crafting-levels.tsv`) and a
/// craft adds the "crafting XP gained" of its rank. Policy: Mining Silver/Stone/Crystal/Wood use
/// the Mining Iron rows; skills without rows (Toolmaking) start at the DAT's first cost (40) and
/// each rank costs 10 more, a craft gaining 6 + 2 a rank.
///
/// Rank-up is automatic when the experience reaches the next threshold. Retail evidence: the 7230
/// craft page layout has no `RaiseCraftButton` (the client ships `RequestRaiseCraftSkill` but the
/// layout never shows a way to press it), so a player cannot ask for a raise; the explicit request
/// is served too (`raise`), and gives the same result.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    xp: BTreeMap<u32, u64>,
}
pub const MAX_LEVEL: u32 = 150;
impl Progress {
    fn table(skill: u32) -> u32 {
        if (MINING_SILVER..=MINING_WOOD).contains(&skill) {
            MINING_IRON
        } else {
            skill
        }
    }
    pub fn xp(&self, skill: u32) -> u64 {
        self.xp.get(&skill).copied().unwrap_or(0)
    }
    /// The experience a rank starts at.
    fn threshold(skill: u32, level: u32) -> u64 {
        let table = Self::table(skill);
        if crafting::rank_row(table, 1).is_some() {
            return crafting::rank_row(table, level).map_or(u64::MAX, |r| u64::from(r.curve));
        }
        // 40, 50, 60... a rank: the sum of the first level-1 costs.
        let n = u64::from(level.saturating_sub(1));
        n * 40 + 10 * n * n.saturating_sub(1) / 2
    }
    pub fn level_of(skill: u32, xp: u64) -> u32 {
        let mut level = 1;
        while level < MAX_LEVEL && xp >= Self::threshold(skill, level + 1) {
            level += 1;
        }
        level
    }
    pub fn level(&self, skill: u32) -> u32 {
        Self::level_of(skill, self.xp(skill))
    }
    /// One craft or strike of `skill`: add the rank's crafting experience; returns it.
    pub fn gain(&mut self, skill: u32) -> u64 {
        let level = self.level(skill);
        let gained = crafting::rank_row(Self::table(skill), level)
            .map_or(6 + 2 * u64::from(level - 1), |r| u64::from(r.cxp));
        *self.xp.entry(skill).or_insert(0) += gained;
        gained
    }
    /// `RequestRaiseCraftSkill`: refused unless the experience already reaches the next rank
    /// (it always has: ranks are taken as experience arrives).
    pub fn raise(&self, skill: u32) -> Result<u32, &'static str> {
        if !SKILL_ORDER[1..].contains(&skill) {
            return Err("That craft skill cannot be raised.");
        }
        Err("Your craft skill is already at the rank your crafting experience earns.")
    }
    pub fn skills(&self) -> Vec<SkillEntry> {
        SKILL_ORDER
            .iter()
            .map(|&did| SkillEntry { did, level: self.level(did), xp: self.xp(did) as u32 })
            .collect()
    }
    /// The saved line (`tradeskills.tsv`): `skill=xp,...`.
    pub fn encode(&self) -> String {
        let xp: Vec<String> = self.xp.iter().map(|(s, x)| format!("{s:x}={x}")).collect();
        xp.join(",")
    }
    pub fn decode(line: &str) -> Option<Self> {
        let mut parsed = BTreeMap::new();
        for pair in line.split(',').filter(|p| !p.is_empty()) {
            let (skill, xp) = pair.split_once('=')?;
            let skill = u32::from_str_radix(skill, 16).ok()?;
            if !SKILL_ORDER[1..].contains(&skill) {
                return None;
            }
            parsed.insert(skill, xp.parse().ok()?);
        }
        Some(Self { xp: parsed })
    }
    /// Development knob: start with this much experience in every tradeskill.
    pub fn grant_all(&mut self, xp: u64) {
        for &skill in &SKILL_ORDER[1..] {
            *self.xp.entry(skill).or_insert(0) += xp;
        }
    }
    /// Policy: a recipe is known once its skill's level reaches the recipe's own minimum level;
    /// the two potion recipes of the quest skill are always known.
    pub fn known_recipes(&self, catalog: &Catalog) -> Vec<u32> {
        if let Ok(list) = std::env::var("PYREALIUM_DEV_RECIPES") {
            return list.split(',').filter_map(|h| u32::from_str_radix(h, 16).ok()).collect();
        }
        let mut known = vec![crafting::HEALTH_RECIPE, crafting::VIGOR_RECIPE];
        for &skill in &SKILL_ORDER[1..] {
            let level = self.level(skill);
            known.extend(
                catalog
                    .of_skill(skill)
                    .filter(|r| r.min_level <= level)
                    .map(|r| r.id),
            );
        }
        known
    }
}

/// Motes (`wsl_mote_*`) share Tallow's generic physical record and have no appearance file.
pub const MOTE_PHYSICAL: u32 = 0x47000260;
pub const MOTE_VISUAL: u32 = 0x1f000234;
/// Stacks hold at most this many units: the motes' own limit, 1,000,000 (the entity's property
/// `0x41000089` and the weenie description's `0x105` of `wsl_mote_iron`, coal, steel, pyreal, hide,
/// bone, sinew, chitin, tallow and the refined metals; coins hold 100,000,000). A recipe slot takes
/// one item (`AddIngredient` names one item an ordinal) that holds the slot's whole quantity, so the
/// 500 and 2500 refinings are filled by one stack of that many, merged by the player. The wire's
/// quantity words are 32-bit (`item_services::acquired` fields 26 and 48).
pub const STACK: u32 = 1_000_000;
pub fn is_mote(definition: u32) -> bool {
    STACKABLE.contains(&definition)
        || crate::butchery::TRAITS.contains(&definition)
        || definition >> 8 == 0x4700_10
        || (0x4700_3646..=0x4700_365d).contains(&definition)
        || MATERIALS.iter().any(|m| m.mote == definition)
}

/// Item metadata for the tradeskill items: motes by their weenie description's physical record,
/// maps and tools through the generic reader.
pub fn metadata(dat: &mut Dat, definition: u32) -> std::io::Result<Metadata> {
    if is_mote(definition) {
        let desc = dat.read(0x8100_0000 | (definition & 0xff_ffff))?;
        let last = desc.len().checked_sub(4).map(|at| u32::from_le_bytes(desc[at..].try_into().unwrap()));
        if last != Some(MOTE_PHYSICAL) {
            return Err(std::io::Error::other("unsupported mote description"));
        }
        dat.read(definition)?;
        return Ok(Metadata { implement: None, icon: 0, physical: MOTE_PHYSICAL, equipment: None });
    }
    Metadata::load(dat, definition)
}

/// The `0x1f` visual a physical record names (`[size, did, 0x40000001, visual]`).
pub fn visual_of(dat: &mut Dat, physical: u32) -> std::io::Result<u32> {
    let bytes = dat.read(physical)?;
    let word = |i: usize| bytes.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    match (word(2), word(3)) {
        (Some(0x4000_0001), Some(visual)) if visual >> 24 == 0x1f => Ok(visual),
        _ => Err(std::io::Error::other("unsupported physical record")),
    }
}

/// A bag item of a tradeskill definition.
pub fn item(
    dat: &mut Dat,
    definition: u32,
    id: u64,
    slot: u16,
    quantity: u32,
) -> std::io::Result<(Item, Metadata)> {
    let info = metadata(dat, definition)?;
    let visual = visual_of(dat, info.physical)?;
    let hand = info.equipment.map_or(0, |e| e.valid as u16 & 0x7ff);
    Ok((
        Item {
            roll: None,
            id,
            definition,
            visual,
            quantity,
            stack_limit: if is_mote(definition) { STACK } else { 1 },
            equipment_mask: hand,
            location: Location::Bag(slot),
        },
        info,
    ))
}

/// What a deposit entity carries (`0x410002c4`: the mining skill, `0x410002c5`: the mote it
/// yields, `0x410002c8`: its charges), read from its entity record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepositInfo {
    pub skill: u32,
    pub mote: u32,
    pub charges: u32,
    pub visual: u32,
}
pub fn deposit_info(dat: &mut Dat, entity: u32) -> std::io::Result<DepositInfo> {
    let bytes = dat.read(entity)?;
    let words: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let prop = |key: u32| words.windows(2).find(|w| w[0] == key).map(|w| w[1]);
    let bad = || std::io::Error::other("unsupported deposit entity");
    let physical = prop(0x4100_020b).ok_or_else(bad)?;
    Ok(DepositInfo {
        skill: prop(0x4100_02c4).ok_or_else(bad)?,
        mote: prop(0x4100_02c5).ok_or_else(bad)?,
        charges: prop(0x4100_02c8).ok_or_else(bad)?,
        visual: visual_of(dat, physical)?,
    })
}

/// The place a surveyor's map names: its description says "You should begin your search near
/// 14.9S, 40.0W", real map coordinates (`Position::FromRealMapCoords`: latitude, longitude in
/// percent of the half-world). Returned as land cells (`LandDefs::game_map_coords_to_gid`:
/// `w / 2 * (1 + v / 100)` with a world 2040 cells wide), x east and y north.
pub fn map_coordinates(text: &str) -> Option<[f64; 2]> {
    let rest = &text[text.find("near ")? + 5..];
    let mut parts = rest.split(',');
    let part = |p: &str, plus: char, minus: char| -> Option<f64> {
        let p = p.trim().trim_end_matches('.');
        let letter = p.chars().last()?;
        let value: f64 = p[..p.len() - 1].trim().parse().ok()?;
        match letter.to_ascii_uppercase() {
            c if c == plus => Some(value),
            c if c == minus => Some(-value),
            _ => None,
        }
    };
    let latitude = part(parts.next()?, 'N', 'S')?;
    let longitude_text = parts.next()?;
    let end = longitude_text.find('.').and_then(|d| longitude_text[d + 1..].find(|c: char| c.is_ascii_alphabetic()).map(|e| d + 1 + e + 1))?;
    let longitude = part(&longitude_text[..end], 'E', 'W')?;
    Some([1020.0 * (1.0 + longitude / 100.0), 1020.0 * (1.0 + latitude / 100.0)])
}
/// Policy: "use the map in nearby locations" works within this many land cells of the place
/// (about 1.2 degrees; a cell is 20 m, so a quarter kilometre).
pub const NEAR_CELLS: f64 = 12.0;
/// Where a placement is, in land cells (the unit of `@maptravel`).
pub fn land_of(placement: crate::movement::Placement) -> [f64; 2] {
    let (bx, by) = (f64::from(placement.cell >> 24), f64::from((placement.cell >> 16) & 0xff));
    [bx * 8.0 + f64::from(placement.coordinates[0]) / 20.0, by * 8.0 + f64::from(placement.coordinates[1]) / 20.0]
}

/// Pick the outcome a roll in 0..1000 lands on (the first whose threshold exceeds it; the last
/// when none does).
pub fn pick_outcome(outcomes: &[Outcome], roll: f32) -> Option<&Outcome> {
    let mut sorted: Vec<&Outcome> = outcomes.iter().collect();
    sorted.sort_by(|a, b| a.threshold.total_cmp(&b.threshold));
    sorted
        .iter()
        .copied()
        .find(|o| roll < o.threshold)
        .or(sorted.last().copied())
}

/// Refining (the five skills 0x6f00283c..40). The recipe records hold two ingredient slots of the
/// same quantity, the outcome roll, a "crafted" state (`6f000052`) and the Refining Workshop
/// effect (`6f002ee6`, package 0x14d: name "Refining", "Your proximity to this Refining Workshop
/// allows you to refine advanced ingredients for use in high-quality items", carried by the
/// workshop entities `4700082d`, `4700086e`, `47000895` as their property `410002c1`): a
/// requirement of standing at a workshop ("You must be near a Refining Workshop to craft this
/// item"), not a result. No record names the refined item or its quantity. What names them:
/// * the item: the skill's description ("refine Iron and Silver into Dramastic") and the recipe
///   names ("Refine Dramastic (100)"), agreeing with the skill's ingredient pair; the item is the
///   material of that name (`wsl_*`, string tables 2500797d, 25007aeb, ...);
/// * the quantity: the recipe's own description, "The amount of Dramastic this recipe yields is
///   dependent on your Dramastic Refining skill, including any bonuses from a wielded Crucible".
///   The formula is not in any record; see [`refine_yield`] (policy).
/// Returns the refined material a classic refining skill makes.
pub fn refined_product(skill: u32) -> Option<u32> {
    match skill {
        REFINING_DRAMASTIC => Some(0x4700_2ad5),
        REFINING_MOONGLASS => Some(0x4700_27a4),
        REFINING_RUNEWOOD => Some(0x4700_27a5),
        REFINING_STEEL => Some(0x4700_27a6),
        REFINING_TUKALITE => Some(0x4700_27a8),
        _ => None,
    }
}
/// The refined materials, coal (`wsl_coal`) and the other stackable crafting materials that share
/// the motes' generic physical record.
pub const STACKABLE: [u32; 6] = [0x4700_2ad5, 0x4700_27a4, 0x4700_27a5, 0x4700_27a6, 0x4700_27a8, 0x4700_2ad4];
/// The percentage of its ingredients a refining recipe returns at a rank of its refining skill.
/// AC2Hero, `refining.html` ("The efficiency of this operation has been said to be between 34%
/// and 100%, meaning 100 Wood + 100 Stone will result in 34 - 100 Runewood. We can get better
/// results when we increase our Refining Skill for Runewood, Stand near a Refiner and use a
/// Crucible"): 34 at rank 1 up to 100 at the top rank, in even steps (the page gives the range and
/// the direction, not the curve: policy). The workshop is a requirement of the recipe in the
/// DAT (`classic_refining`'s doc), the Crucible's bonus has no amount in the DAT or the page and
/// is not modelled.
pub fn refine_percent(level: u32) -> u32 {
    let level = level.clamp(1, MAX_LEVEL);
    34 + 66 * (level - 1) / (MAX_LEVEL - 1)
}
/// The units a refining recipe returns for `quantity` ingredient units (`refine_percent`), at
/// least one.
pub fn refine_yield(quantity: u32, level: u32) -> u32 {
    (quantity * refine_percent(level) / 100).max(1)
}
/// Units a mining strike adds to a deposit's base yield for the character's Surveying rank.
/// AC2Hero, `survey.html`: "a bonus for a high Surveying skill, which maxes out at 40 extra yield
/// units per swing at 150 Surveying" (even steps up to that: policy; rank 1 gets none).
pub fn survey_bonus(level: u32) -> u32 {
    40 * level.min(MAX_LEVEL) / MAX_LEVEL
}
/// The `rcp_*_study` recipes (the DAT marks them "OBSOLETE - DNT" and describes them as crafting
/// and discarding samples "to improve your skills" without profit).
pub fn is_study(recipe: u32) -> bool {
    matches!(recipe, 0x6f00_3085 | 0x6f00_308a | 0x6f00_308b | 0x6f00_308d | 0x6f00_308f | 0x6f00_3091 | 0x6f00_3093 | 0x6f00_3095 | 0x6f00_3097 | 0x6f00_3099 | 0x6f00_309b)
}
/// Whether a refining recipe takes only the classic motes (iron, silver, crystal, stone, wood):
/// the newer resources' refinings (`Create Rough Boards (100)`, `Create Tiles (100)`) make other
/// items whose definitions no record names, so they are not served.
pub fn classic_refining(recipe: &Recipe) -> bool {
    refined_product(recipe.skill).is_some()
        && recipe.outcomes.is_empty()
        && !recipe.ingredients.is_empty()
        && recipe
            .ingredients
            .iter()
            .all(|i| !i.allowed.is_empty() && i.allowed.iter().all(|a| MATERIALS.iter().any(|m| m.mote == *a)))
}

/// The DAT-backed tradeskill data a running server holds: the recipes, the item definitions
/// the skills create or use (with their visual and metadata) and the deposit entities.
pub struct Assets {
    pub catalog: Catalog,
    items: BTreeMap<u32, (u32, Metadata)>,
    deposits: BTreeMap<u32, DepositInfo>,
    /// The traders' stores by the trader's entity (see [`StoreLine`]).
    traders: BTreeMap<u32, (u32, Vec<StoreLine>)>,
    /// Each creature entity's butchery profile (`butchery.rs`).
    butchery: BTreeMap<u32, u32>,
}

/// One sale line of a trader's store record (`store_*.wc`, the raw-material traders
/// `0x6f003570..=0x6f003575`). The record's lines are package `0x380f` objects of eight words:
/// the line's ordinal, the item definition, a reference to the line's name object, the price per
/// unit as a float, a zero and the currency (`wsl_coin`, `0x47001108`). The trader entity names
/// its store in property `0x410002de` (`wsl_trader_cragstone` -> `store_cragstone`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoreLine {
    pub definition: u32,
    /// Coins per unit (coal costs 0.5, the starter tools and flawless pyreal 50).
    pub price: f32,
    /// The line's texture (the item's weenie description, word 7).
    pub icon: u32,
}
/// The store property of an entity record.
const STORE_PROPERTY: u32 = 0x4100_02de;
const COIN: u32 = 0x4700_1108;

/// The sale lines of a store record, in the record's order.
pub fn store_lines(bytes: &[u8]) -> Vec<(u32, f32)> {
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let mut lines = Vec::new();
    for (at, word) in words.iter().enumerate() {
        if *word != 0x380f_0000 || words.get(at + 1) != Some(&8) {
            continue;
        }
        let Some(fields) = words.get(at + 2..at + 10) else { continue };
        if fields[1] >> 24 == 0x47 && fields[5] == COIN {
            lines.push((fields[1], f32::from_bits(fields[3])));
        }
    }
    lines
}
impl Assets {
    pub fn load(dat: &mut Dat) -> std::io::Result<Self> {
        let catalog = Catalog::load(dat);
        if catalog.is_empty() {
            return Err(std::io::Error::other("the DAT holds no craft recipes"));
        }
        let mut wanted: Vec<u32> = (FIRST_MAP..FIRST_MAP + 90).collect();
        wanted.extend((NEW_FIRST_MAP..NEW_FIRST_MAP + 95).filter(|d| map_deposit(*d).is_some()));
        for m in &MATERIALS {
            wanted.extend([m.tool, m.starter_tool, m.mote]);
        }
        wanted.extend(STACKABLE);
        wanted.extend(crate::butchery::TRAITS);
        for &skill in &SKILL_ORDER[1..] {
            for r in catalog.of_skill(skill) {
                wanted.extend(r.outcomes.iter().map(|o| o.output));
                wanted.extend(r.ingredients.iter().flat_map(|i| i.allowed.iter().copied()));
            }
        }
        wanted.sort_unstable();
        wanted.dedup();
        let mut items = BTreeMap::new();
        for d in wanted {
            if let Ok(info) = metadata(dat, d) {
                if let Ok(visual) = visual_of(dat, info.physical) {
                    items.insert(d, (visual, info));
                }
            }
        }
        let mut deposits = BTreeMap::new();
        // The names these items carry when created (`item_names`).
        for &definition in items.keys() {
            if let Some(name) = crate::item_names::name_of(dat, definition) {
                crate::item_names::register(definition, name);
            }
        }
        for d in MATERIALS.iter().flat_map(|m| m.deposits) {
            deposits.insert(d, deposit_info(dat, d)?);
        }
        for d in new_family_deposits() {
            if let Ok(info) = deposit_info(dat, d) {
                if let Ok(mote) = metadata(dat, info.mote).and_then(|m| visual_of(dat, m.physical).map(|v| (v, m))) {
                    items.insert(info.mote, mote);
                }
                deposits.insert(d, info);
            }
        }
        // The raw-material traders: entities that name a store record in property 0x410002de.
        let mut traders = BTreeMap::new();
        for entity in 0x4700_2f00u32..0x4700_3100 {
            let Ok(bytes) = dat.read(entity) else { continue };
            let words: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
            let Some(store) = words.windows(2).find(|w| w[0] == STORE_PROPERTY && w[1] >> 24 == 0x6f).map(|w| w[1]) else { continue };
            let Ok(record) = dat.read(store) else { continue };
            let mut lines = Vec::new();
            for (definition, price) in store_lines(&record) {
                if !items.contains_key(&definition) {
                    let Ok(info) = metadata(dat, definition) else { continue };
                    let Ok(visual) = visual_of(dat, info.physical) else { continue };
                    items.insert(definition, (visual, info));
                }
                let desc = dat.read(0x8100_0000 | (definition & 0xff_ffff)).unwrap_or_default();
                let icon = desc.get(28..32).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
                if icon >> 24 == 0x41 {
                    lines.push(StoreLine { definition, price, icon });
                }
            }
            if !lines.is_empty() {
                traders.insert(entity, (store, lines));
            }
        }
        for &definition in items.keys() {
            if crate::item_names::lookup(definition).is_none() {
                if let Some(name) = crate::item_names::name_of(dat, definition) {
                    crate::item_names::register(definition, name);
                }
            }
        }
        let butchery = crate::butchery::profiles(dat);
        Ok(Self { catalog, items, deposits, traders, butchery })
    }
    /// The butchery profile of the creature entity `entity`, when it has one.
    pub fn butchery_profile(&self, entity: u32) -> Option<u32> {
        self.butchery.get(&entity).copied()
    }
    /// The store record and sale lines of the trader entity `entity`, when it names one.
    pub fn trader_store(&self, entity: u32) -> Option<(u32, &[StoreLine])> {
        self.traders.get(&entity).map(|(store, lines)| (*store, lines.as_slice()))
    }
    /// Every item definition with a loaded profile and its metadata.
    pub fn metadata(&self) -> impl Iterator<Item = (u32, Metadata)> + '_ {
        self.items.iter().map(|(d, (_, m))| (*d, *m))
    }
    /// A bag item of `definition`, when its profile loaded.
    pub fn make(&self, definition: u32, id: u64, slot: u16, quantity: u32) -> Option<Item> {
        let (visual, info) = self.items.get(&definition)?;
        Some(Item {
            roll: None,
            id,
            definition,
            visual: *visual,
            quantity,
            stack_limit: if is_mote(definition) { STACK } else { 1 },
            equipment_mask: info.equipment.map_or(0, |e| e.valid as u16 & 0x7ff),
            location: Location::Bag(slot),
        })
    }
    pub fn deposit(&self, entity: u32) -> Option<&DepositInfo> {
        self.deposits.get(&entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portal() -> Option<Dat> {
        let dir = std::env::var_os("AC2_TEST_7230_PORTAL")?;
        Some(Dat::open(dir).expect("AC2_TEST_7230_PORTAL opens"))
    }

    /// AC2Hero `resource.html` lists the raw-material traders' bundles: Coal 200 = 100 gp, 1000 =
    /// 500, 5000 = 2500; Flawless Pyreal, Gathered Deruwood and Imbued Obsidian 2 = 100, 10 =
    /// 500, 50 = 2500. The DAT's store records (client and DAT outrank the page) price a unit:
    /// the same prices, so any bundle the page names costs what it says, and the purchase
    /// dialog sells any count at the unit price.
    #[test]
    fn the_traders_unit_prices_make_the_bundles_ac2hero_lists() {
        let Some(mut dat) = portal() else { return };
        let mut coal = 0;
        let mut dear = 0;
        for store in 0x6f00_3570u32..=0x6f00_3575 {
            for (definition, price) in store_lines(&dat.read(store).unwrap()) {
                if definition == 0x4700_2ad4 {
                    assert_eq!(price, 0.5, "coal is half a coin");
                    for (units, gold) in [(200.0, 100.0), (1000.0, 500.0), (5000.0, 2500.0)] {
                        assert_eq!(units * f64::from(price), gold);
                    }
                    coal += 1;
                } else {
                    assert_eq!(price, 50.0, "{definition:08x}: a flawless/imbued/gathered unit");
                    for (units, gold) in [(2.0, 100.0), (10.0, 500.0), (50.0, 2500.0)] {
                        assert_eq!(units * f64::from(price), gold);
                    }
                    dear += 1;
                }
            }
        }
        assert!(coal >= 3, "Cragstone, Ikeras and Linvak Tukal sell coal ({coal})");
        assert!(dear >= 3, "Cavendo, Hakata and Ondekodo sell their commodity ({dear})");
    }

    #[test]
    fn map_families_follow_the_entity_order() {
        let (m, r) = map_family(0x47002e99).unwrap();
        assert_eq!((m.name, r), ("iron", 0));
        let (m, r) = map_family(0x47002eac).unwrap();
        assert_eq!((m.name, r), ("crystal", 1));
        assert!(map_family(0x47002e8d).is_none());
    }

    #[test]
    fn the_surveying_recipes_decode() {
        let Some(mut dat) = portal() else { return };
        let catalog = Catalog::load(&mut dat);
        let r = catalog.get(0x6f002af0).expect("linvak iron surveying");
        assert_eq!(r.skill, SURVEYING);
        assert_eq!(r.outcomes.len(), 6, "{:?}", r.outcomes);
        assert!(r.ingredients.is_empty());
        let pick = catalog.get(0x6f002854).expect("pickaxe");
        assert_eq!(pick.skill, TOOLMAKING);
        assert_eq!(pick.outcomes[0].output, 0x47002deb);
        assert_eq!(pick.ingredients.len(), 5, "{:?}", pick.ingredients);
        assert!(pick.ingredients[1].allowed.contains(&0x470010f3));
    }

    #[test]
    fn every_craft_skill_is_registered_once() {
        assert_eq!(SKILL_ORDER.len(), 1 + CRAFT_SKILLS.len());
        assert_eq!(SKILL_ORDER[0], crafting::CRAFT_SKILL_DID);
        let mut sorted = SKILL_ORDER.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SKILL_ORDER.len());
        let Some(mut dat) = portal() else { return };
        // Each is a package 0x33af state whose first field is its `craftskilltype.emp` number
        // (2..=24 and 26..=29; 25, the quest skill, is the registry's first entry).
        let mut types = Vec::new();
        for skill in CRAFT_SKILLS {
            let objects = parse(&dat.read(skill).unwrap()).expect("a skill record parses");
            assert_eq!(objects[0].pkg, 0x33af_0000, "{skill:08x}");
            types.push(objects[0].words[0]);
        }
        types.sort_unstable();
        assert_eq!(types, (2..=24).chain(26..=29).collect::<Vec<_>>());
    }

    #[test]
    fn refining_recipes_name_no_item_and_the_skill_names_it() {
        // AC2Hero refining.html: 34 to 100 percent, better with the skill.
        assert_eq!(refine_yield(100, 1), 34);
        assert_eq!(refine_yield(100, 150), 100);
        assert!(refine_yield(100, 40) > refine_yield(100, 2));
        assert_eq!(refine_yield(1, 2), 1);
        // AC2Hero survey.html: up to 40 extra units a swing at 150 Surveying.
        assert_eq!(survey_bonus(1), 0);
        assert_eq!(survey_bonus(150), 40);
        assert!(survey_bonus(75) > 0 && survey_bonus(75) < 40);
        let Some(mut dat) = portal() else { return };
        let catalog = Catalog::load(&mut dat);
        // Refine Steel (100): Iron and Wood, 100 of each, no item outcome.
        let steel = catalog.get(0x6f002a34).expect("refine steel (100)");
        assert_eq!(steel.skill, REFINING_STEEL);
        assert!(steel.outcomes.is_empty());
        assert_eq!(steel.ingredients.len(), 2);
        assert!(steel.ingredients.iter().all(|i| i.quantity == 100));
        assert!(classic_refining(steel));
        assert_eq!(refined_product(steel.skill), Some(0x4700_27a6));
        // The newer resources' refinings make other items and are not served.
        let boards = catalog.get(0x6f00369f).expect("create rough boards (100)");
        assert_eq!(boards.skill, REFINING_RUNEWOOD);
        assert!(!classic_refining(boards));
        // The record's root class is the skill's own (Refining: Steel is 0x36a8).
        let objects = parse(&dat.read(0x6f002a34).unwrap()).unwrap();
        assert_eq!(objects[0].pkg, 0x36a8_0000);
        // Every skill with recipes parses them, from the whole recipe range.
        for skill in CRAFT_SKILLS {
            let n = catalog.of_skill(skill).count();
            assert!(n > 0 || [BUTCHERY, 0x6f00_5bb4, 0x6f00_5bb5].contains(&skill), "{skill:08x}");
        }
    }

    #[test]
    fn map_text_names_a_place() {
        let text = "This map of Linvak Massif is marked with directions to a likely patch of resources. You should begin your search near 14.9S, 40.0W. Then use the map in nearby locations to prospect for the resources.";
        let [x, y] = map_coordinates(text).unwrap();
        assert!((x - 612.0).abs() < 0.01 && (y - 868.02).abs() < 0.01, "{x} {y}");
        let [x, y] = map_coordinates("near 39.6N, 24.7E. Then").unwrap();
        assert!((x - 1271.94).abs() < 0.01 && (y - 1423.92).abs() < 0.01, "{x} {y}");
    }

    #[test]
    fn every_tradeskill_levels_with_use_and_unlocks_recipes() {
        // Mining Iron: 2 a strike, rank 2 at 20 (the Pyreal rows); the other mining skills
        // share them. Toolmaking has no row: 40, then 50 more for rank 3, 6 a craft at rank 1.
        for skill in [MINING_IRON, MINING_SILVER, MINING_STONE, MINING_CRYSTAL, MINING_WOOD] {
            let mut progress = Progress::default();
            for _ in 0..9 {
                progress.gain(skill);
            }
            assert_eq!(progress.level(skill), 1);
            progress.gain(skill);
            assert_eq!(progress.level(skill), 2, "{skill:08x}");
        }
        let mut progress = Progress::default();
        for _ in 0..6 {
            progress.gain(TOOLMAKING);
        }
        assert_eq!(progress.level(TOOLMAKING), 1);
        progress.gain(TOOLMAKING);
        assert_eq!(progress.level(TOOLMAKING), 2);
        assert_eq!(Progress::decode(&progress.encode()), Some(progress.clone()));
        let Some(mut dat) = portal() else { return };
        let catalog = Catalog::load(&mut dat);
        let count = |p: &Progress| p.known_recipes(&catalog).iter().filter(|r| catalog.get(**r).is_some_and(|r| r.skill == TOOLMAKING)).count();
        assert_eq!(count(&Progress::default()), 6);
        assert_eq!(count(&progress), 7, "rank 2 adds 6f0036ad");
        progress.grant_all(1000);
        assert!(count(&progress) > 30, "high ranks know the tool recipes");
    }
}
