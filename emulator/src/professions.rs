//! DAT-backed profession recipes and durable advancement. Reconstruction policy
//! (manufacture XP, refining yield and output rolls) is documented in content/README.md.
use crate::{
    inventory::{Command, Inventory, Item, Location},
    inventory_request::Metadata,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::OnceLock,
};
pub const RAISE: u32 = 0x8206002e;
pub const WINDOW: u64 = 22 * 60 * 60;
fn bad(s: &str) -> io::Error {
    io::Error::other(s)
}
#[derive(Debug, Deserialize)]
pub struct Skill {
    pub id: u32,
    pub name: String,
    pub cap: u32,
    pub xp: Vec<u64>,
    pub limit: Vec<u64>,
    pub use_xp: Vec<u64>,
}
#[derive(Debug, Deserialize)]
pub struct Ingredient {
    pub ordinal: u32,
    pub name: String,
    pub choices: Vec<u32>,
    pub minimum: u32,
    pub quantities: Vec<u64>,
}
#[derive(Debug, Deserialize)]
pub struct Output {
    pub threshold: f64,
    pub item: u32,
}
#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub id: u32,
    pub skill: u32,
    pub name: String,
    pub kind: String,
    pub workshop: u32,
    pub minimum: u32,
    pub maximum: u32,
    pub difficulty: Vec<u64>,
    pub cost: Vec<u64>,
    pub ingredients: Vec<Ingredient>,
    pub outputs: Vec<Output>,
}
#[derive(Debug, Deserialize)]
pub struct Definition {
    pub id: u32,
    pub visual: u32,
    pub name: String,
    pub damage: u32,
    pub armor: u32,
    pub stack: u16,
    pub dye: Option<f32>,
}
#[derive(Debug, Deserialize)]
pub struct Workshop {
    pub id: u32,
    pub effect: u32,
    pub radius: f32,
}
#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub skills: Vec<Skill>,
    pub recipes: Vec<Recipe>,
    pub items: Vec<Definition>,
    pub workshops: Vec<Workshop>,
    pub success: Vec<f32>,
    pub mines: Vec<crate::harvesting::Mine>,
    pub maps: Vec<crate::harvesting::Map>,
    pub tools: Vec<crate::harvesting::Tool>,
    pub butcher: Vec<crate::harvesting::Butcher>,
}
pub fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| {
        serde_json::from_str(include_str!("../content/professions-v1.json"))
            .expect("checked profession catalog")
    })
}
pub fn skill(id: u32) -> Option<&'static Skill> {
    catalog().skills.iter().find(|s| s.id == id)
}
pub fn recipe(id: u32) -> Option<&'static Recipe> {
    catalog().recipes.iter().find(|s| s.id == id)
}
/// Installed creation-tool identities; discipline relationships also survive in
/// the retail Toolmaking documentation. Bonus magnitude is reconstruction policy.
pub fn creation_tool(definition: u32) -> Option<&'static [u32]> {
    match definition {
        0x47002de3 => Some(&[0x6f00282d]),
        0x47002de5 => Some(&[0x6f00283c, 0x6f00283d, 0x6f00283e, 0x6f00283f, 0x6f002840]),
        0x47002de6 => Some(&[
            0x6f002565, 0x6f00256d, 0x6f00256e, 0x6f00256f, 0x6f002570, 0x6f002571, 0x6f005276,
        ]),
        0x47002de9 => Some(&[0x6f002857]),
        0x47002ded => Some(&[0x6f0023a8, 0x6f0023aa, 0x6f0023a9, 0x6f005242]),
        _ => None,
    }
}
pub fn tool_durability(definition: u32) -> Option<u32> {
    crate::harvesting::tool(definition)
        .map(|t| t.durability)
        .or_else(|| creation_tool(definition).map(|_| 20))
}
pub fn creation_tool_usable(book: &Book, item: &Item) -> bool {
    creation_tool(item.definition).is_some_and(|skills| {
        skills
            .iter()
            .any(|s| book.get(*s).level >= u32::from(item.roll.map_or(1, |r| r.level)) * 10)
    }) && item
        .roll
        .is_none_or(|r| u32::from(r.wear) < tool_durability(item.definition).unwrap())
}
pub fn creation_bonus<'a>(
    book: &Book,
    inventory: &'a Inventory,
    skill: u32,
) -> Option<(&'a Item, u32)> {
    inventory
        .items()
        .find(|i| {
            i.location == Location::Equipped(0x40)
                && creation_tool(i.definition).is_some_and(|s| s.contains(&skill))
                && book.get(skill).level >= u32::from(i.roll.map_or(1, |r| r.level)) * 10
                && creation_tool_usable(book, i)
        })
        .map(|i| (i, u32::from(i.roll.map_or(1, |r| r.level))))
}
pub fn valid_finish(item: &Item) -> bool {
    let Some(d) = catalog()
        .items
        .iter()
        .find(|d| d.id == item.definition && d.visual == item.visual)
    else {
        return false;
    };
    let r = item.roll.unwrap();
    if let Some(tool) = crate::harvesting::tool(d.id) {
        return r.armor == 0 && r.damage == 0 && u32::from(r.wear) < tool.durability;
    }
    if creation_tool(d.id).is_some() {
        return item.equipment_mask == 0x40
            && r.armor == 0
            && r.damage == 0
            && u32::from(r.wear) <= tool_durability(d.id).unwrap();
    }
    if r.wear != 0 {
        return false;
    }
    if let Some(slot) = crate::apparel::recovered_slot(d.id, d.visual) {
        return item.equipment_mask == slot && r.armor > 0 && r.damage == 0;
    }
    item.equipment_mask & 0xc0 != 0
        && r.armor == 0
        && ((d.damage > 0 && r.damage > 0) || (d.damage == 0 && r.damage == 0))
}
pub fn weapon_damage(inventory: &Inventory) -> Option<u32> {
    crate::treasure_weapons::equipped_damage(inventory).or_else(|| inventory
        .items()
        .find(|i| i.location == Location::Equipped(0x40))
        .and_then(|i| i.roll)
        .filter(|r| r.crafted && r.damage > 0)
        .map(|r| u32::from(r.damage)))
}
pub fn value(curve: &[u64], level: u32) -> u64 {
    curve
        .get(if curve.len() == 1 { 0 } else { level as usize })
        .copied()
        .unwrap_or(0)
}
impl Recipe {
    pub fn inputs(&self, level: u32) -> Vec<&Ingredient> {
        self.ingredients
            .iter()
            .filter(|i| level >= i.minimum)
            .collect()
    }
    pub fn check_level(&self, level: u32) -> io::Result<()> {
        if level < self.minimum || level > self.maximum {
            Err(bad("Choose a level within the recipe's range."))
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub level: u32,
    pub available: u64,
    pub earned: u64,
    pub reset: u64,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            level: 1,
            available: 0,
            earned: 0,
            reset: 0,
        }
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Book(pub BTreeMap<u32, Progress>, pub crate::harvesting::Book);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub skill: u32,
    pub xp: u64,
    pub time: u64,
    pub raise: bool,
    #[serde(default)]
    pub activity: Option<crate::harvesting::Activity>,
}
impl Book {
    pub fn get(&self, id: u32) -> Progress {
        self.0.get(&id).cloned().unwrap_or_default()
    }
    pub fn apply(&self, c: &Change) -> io::Result<Self> {
        let s = skill(c.skill).ok_or_else(|| bad("Unknown crafting profession."))?;
        if c.time == 0 || c.xp > 1_000_000 || c.raise && c.xp != 0 {
            return Err(bad("Invalid crafting advancement."));
        }
        let mut next = self.clone();
        let p = next.0.entry(c.skill).or_default();
        if p.reset == 0 || c.time >= p.reset {
            p.earned = 0;
            p.reset = c
                .time
                .checked_add(WINDOW)
                .ok_or_else(|| bad("Crafting clock overflow"))?;
        }
        if c.raise {
            if p.level >= s.cap {
                return Err(bad("This profession is at its maximum rank."));
            }
            let cost = value(&s.xp, p.level + 1)
                .checked_sub(value(&s.xp, p.level))
                .filter(|v| *v > 0)
                .ok_or_else(|| bad("Invalid profession XP curve"))?;
            p.available = p
                .available
                .checked_sub(cost)
                .ok_or_else(|| bad("Not enough crafting experience to raise this profession."))?;
            p.level += 1;
        } else {
            let gained = c.xp.min(value(&s.limit, p.level).saturating_sub(p.earned));
            p.available = p
                .available
                .checked_add(gained)
                .ok_or_else(|| bad("Crafting experience overflow"))?;
            p.earned = p
                .earned
                .checked_add(gained)
                .ok_or_else(|| bad("Crafting learning limit overflow"))?;
        }
        if let Some(activity) = &c.activity {
            if c.raise {
                return Err(bad("Invalid gathering advancement"));
            }
            next.1.apply(activity, c.time)?;
        }
        Ok(next)
    }
}
/// Installed ItemCreationRecipeDifficultyTable, indexed by skill - difficulty.
pub fn chance(rank: u32, difficulty: u64) -> u32 {
    let d = (i64::from(rank) - difficulty.min(i64::MAX as u64) as i64).clamp(-500, 500);
    catalog().success[(d + 500) as usize].clamp(0., 100.) as u32
}
pub fn require_workshop(scene: &crate::scene::Scene, player: u64, recipe: u32) -> io::Result<()> {
    let r = self::recipe(recipe).ok_or_else(|| bad("Unknown crafting recipe."))?;
    if r.workshop == 0 {
        return Ok(());
    }
    if scene.content_objects().any(|o| {
        catalog().workshops.iter().any(|w| {
            w.id == o.entity
                && w.effect == r.workshop
                && scene
                    .combat_distance(player, o.id)
                    .is_some_and(|d| d <= w.radius)
        })
    }) {
        return Ok(());
    }
    Err(bad(match r.workshop {
        0x6f002ee6 => "Move within eight meters of a refining workshop.",
        0x6f002ee7 => "Move within eight meters of a spellbinding workshop.",
        _ => "Move within eight meters of a crafting workshop.",
    }))
}
fn quantity(inventory: &Inventory, definition: u32) -> u64 {
    inventory
        .items()
        .filter(|i| i.definition == definition && matches!(i.location, Location::Bag(_)))
        .map(|i| u64::from(i.quantity))
        .sum()
}
pub fn allowed(request: &crate::crafting::ValidateRequest, inventory: &Inventory) -> bool {
    let Some(r) = recipe(request.recipe) else {
        return false;
    };
    if r.check_level(request.spinner).is_err() {
        return false;
    }
    let Some(i) = r
        .inputs(request.spinner)
        .into_iter()
        .find(|i| i.ordinal == request.ordinal)
    else {
        return false;
    };
    if r.kind == "dye"
        && i.ordinal == 1
        && !inventory.items().any(|v| {
            v.id == request.item && crate::apparel::recovered_slot(v.definition, v.visual).is_some()
        })
    {
        return false;
    }
    inventory.dense_bags()
        && inventory.items().any(|v| {
            v.id == request.item
                && matches!(v.location, Location::Bag(_))
                && i.choices.contains(&v.definition)
                && quantity(inventory, v.definition) >= value(&i.quantities, request.spinner)
        })
}
pub struct Plan {
    pub items: Vec<Command>,
    pub gold: u32,
    pub change: Change,
    pub success: bool,
    pub name: String,
}
pub fn plan(
    book: &Book,
    inventory: &Inventory,
    request: &crate::crafting::ExecuteRequest,
    mut id: u64,
    random: u32,
    time: u64,
    metadata: &BTreeMap<u32, Option<Metadata>>,
) -> io::Result<Plan> {
    let r = recipe(request.recipe).ok_or_else(|| {
        bad("This recipe requires a specialized crafting handler or quest unlock.")
    })?;
    r.check_level(request.spinner)?;
    if r.kind == "survey" {
        if !request.ingredients.is_empty() {
            return Err(bad("Surveying does not consume ingredients."));
        }
        return crate::harvesting::survey(book, inventory, r, id, random, time);
    }
    let inputs = r.inputs(request.spinner);
    if !inventory.dense_bags()
        || inputs.is_empty()
        || inputs.len() != request.ingredients.len()
        || !request.ingredients.values().any(|i| *i == request.primary)
    {
        return Err(bad("Fill every required ingredient slot."));
    }
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    let mut remainder = Vec::new();
    for input in inputs {
        let iid = *request
            .ingredients
            .get(&input.ordinal)
            .ok_or_else(|| bad("Missing recipe ingredient."))?;
        let owned = inventory
            .items()
            .find(|i| i.id == iid && matches!(i.location, Location::Bag(_)))
            .ok_or_else(|| bad("Ingredients must be in your backpack."))?;
        let qty = value(&input.quantities, request.spinner);
        if qty == 0 || !input.choices.contains(&owned.definition) {
            return Err(bad(
                "An ingredient has the wrong type or insufficient quantity.",
            ));
        }
        // Preserve the old whole-stack journal operation. Reissue any remainder
        // atomically with a fresh IID, using the existing native acquire path.
        let mut needed = qty;
        let mut stacks: Vec<_> = inventory
            .items()
            .filter(|i| i.definition == owned.definition && matches!(i.location, Location::Bag(_)))
            .collect();
        stacks.sort_by_key(|i| (i.id != iid, i.id));
        for stack in stacks {
            if needed == 0 {
                break;
            }
            if !seen.insert(stack.id) {
                continue;
            }
            let used = needed.min(u64::from(stack.quantity));
            needed -= used;
            items.push(Command::ConsumeStackDense {
                item: stack.id,
                quantity: stack.quantity,
            });
            if used < u64::from(stack.quantity) {
                let mut left = stack.clone();
                left.quantity -= used as u32;
                remainder.push(left);
            }
        }
        if needed > 0 {
            return Err(bad(
                "An ingredient has insufficient quantity across your backpack stacks.",
            ));
        }
    }
    let difficulty = value(&r.difficulty, request.spinner);
    let rank = book.get(r.skill).level;
    let creation_tool = creation_bonus(book, inventory, r.skill)
        .filter(|_| r.kind == "produce" || r.kind == "refine");
    let effective_rank = rank.saturating_add(creation_tool.map_or(0, |(_, bonus)| bonus));
    let probability =
        if r.skill == crate::crafting::CRAFT_SKILL_DID || r.kind == "refine" || r.kind == "dye" {
            100
        } else {
            chance(effective_rank, difficulty)
        };
    if probability == 0 {
        return Err(bad(
            "That recipe level is beyond your current crafting ability.",
        ));
    }
    let success = random % 100 < probability;
    let mut bag = inventory.clone();
    for c in &items {
        bag = bag.apply(c).map_err(io::Error::other)?;
    }
    // Success and failure both wear an equipped creation tool with a 50% roll.
    // Exhausted tools remain inspectable/discardable but provide no bonus.
    if let Some((tool, _)) = creation_tool.filter(|_| random >> 12 & 1 != 0) {
        let previous = tool.roll.map_or(0, |r| r.wear);
        let c = Command::ToolWear {
            item: tool.id,
            previous,
            used: previous + 1,
        };
        bag = bag.apply(&c).map_err(io::Error::other)?;
        items.push(c);
    }
    for mut left in remainder {
        left.id = id;
        id = id
            .checked_add(1)
            .ok_or_else(|| bad("Item identity exhausted"))?;
        left.location = Location::Bag(bag.free_slot().ok_or_else(|| bad("No backpack space."))?);
        let c = Command::Grant(left);
        bag = bag.apply(&c).map_err(io::Error::other)?;
        items.push(c);
    }
    if r.kind == "dye" {
        let original = inventory
            .items()
            .find(|i| Some(&i.id) == request.ingredients.get(&1))
            .ok_or_else(|| bad("Choose the armor to dye."))?;
        let dye = inventory
            .items()
            .find(|i| Some(&i.id) == request.ingredients.get(&2))
            .and_then(|i| catalog().items.iter().find(|d| d.id == i.definition))
            .and_then(|d| d.dye)
            .ok_or_else(|| bad("Choose a dye plant."))?;
        if crate::apparel::recovered_slot(original.definition, original.visual).is_none() {
            return Err(bad("This armor has no recovered dyeable appearance."));
        }
        let mut product = original.clone();
        let mut finish = product.roll.unwrap_or(crate::inventory::Roll {
            armor: crate::apparel::item_armor(product.definition)
                .unwrap_or(1)
                .min(1000) as u16,
            level: 1,
            ..Default::default()
        });
        finish.tint = (dye.clamp(0., 1.) * 1000.).round() as u16;
        product.roll = Some(finish);
        product.id = id;
        product.location = Location::Bag(bag.free_slot().ok_or_else(|| bad("No backpack space."))?);
        let c = Command::Grant(product);
        bag.apply(&c).map_err(io::Error::other)?;
        items.push(c);
    } else if success {
        let roll = f64::from(random / 100 % 1000);
        let output = r
            .outputs
            .iter()
            .find(|o| roll < o.threshold)
            .or(r.outputs.last())
            .ok_or_else(|| bad("Recipe has no product."))?;
        let definition = catalog()
            .items
            .iter()
            .find(|i| i.id == output.item)
            .ok_or_else(|| bad("Product definition missing."))?;
        let info = metadata
            .get(&output.item)
            .and_then(|v| *v)
            .ok_or_else(|| bad("Product metadata unavailable; no ingredients were consumed."))?;
        let apparel = crate::apparel::recovered_slot(definition.id, definition.visual);
        let level = request.spinner.max(1).min(150) as u16;
        let mask = apparel.unwrap_or(info.equipment.map_or(0, |e| e.valid as u16 & 0x7ff));
        // Native item bases are retained; the lost server mutation scaling is
        // reconstructed here and recorded per instance, never supplied by clients.
        let rolled = if crate::harvesting::tool(definition.id).is_some() {
            Some(crate::inventory::Roll {
                level,
                crafted: true,
                ..Default::default()
            })
        } else if apparel.is_some() {
            Some(crate::inventory::Roll {
                armor: (crate::apparel::item_armor(definition.id).unwrap_or(10)
                    + u32::from(level.saturating_sub(1)) * 2)
                    .min(1000) as u16,
                tint: (random / 100000 % 1001) as u16,
                level,
                crafted: true,
                damage: 0,
                wear: 0,
            })
        } else if mask & 0xc0 != 0 {
            Some(crate::inventory::Roll {
                armor: 0,
                tint: 0,
                level,
                damage: if definition.damage > 0 {
                    (definition.damage + u32::from(level.saturating_sub(1)) * 2).min(1000) as u16
                } else {
                    0
                },
                crafted: true,
                wear: 0,
            })
        } else {
            None
        };
        // Refining always succeeds. The shipped handler has no server formula;
        // interpolate the documented 33% novice / 83% master endpoints.
        let mut quantity = if r.kind == "refine" {
            let raw = r
                .inputs(request.spinner)
                .iter()
                .map(|i| value(&i.quantities, request.spinner))
                .min()
                .unwrap_or(0);
            (raw * (3300 + u64::from(effective_rank.min(150).saturating_sub(1)) * 5000 / 149)
                / 10000)
                .max(1)
                .min(65535) as u32
        } else {
            1
        };
        // Refinery output is material, not a distinct crafted equipment roll.
        // Merge before asking for a slot. Only remove pre-transaction IIDs: the
        // native service publishes removals against the original inventory.
        if r.kind == "refine" {
            let stack = bag
                .items()
                .find(|i| {
                    i.definition == definition.id
                        && i.visual == definition.visual
                        && i.roll.is_none()
                        && i.equipment_mask == 0
                        && i.stack_limit == u32::from(u16::MAX)
                        && matches!(i.location, Location::Bag(_))
                        && i.quantity.checked_add(quantity).is_some()
                        && inventory.items().any(|old| old.id == i.id)
                })
                .cloned();
            if let Some(stack) = stack {
                let consume = Command::ConsumeStackDense {
                    item: stack.id,
                    quantity: stack.quantity,
                };
                bag = bag.apply(&consume).map_err(io::Error::other)?;
                quantity += stack.quantity;
                items.push(consume);
            }
        }
        let product = Item {
            id,
            definition: definition.id,
            visual: definition.visual,
            quantity,
            stack_limit: if r.kind == "refine" {
                u32::from(u16::MAX)
            } else {
                u32::from(definition.stack.max(1))
            },
            equipment_mask: mask,
            location: Location::Bag(
                bag.free_slot()
                    .ok_or_else(|| bad("Make space in your backpack for the finished item."))?,
            ),
            roll: rolled,
        };
        let c = Command::Grant(product);
        bag.apply(&c).map_err(io::Error::other)?;
        items.push(c);
    }
    let s = skill(r.skill).ok_or_else(|| bad("Missing profession"))?;
    // Native per-use curves when provided; manufacture award is reconstructed.
    // No XP for trivial recipes, failures, or quest recipes.
    let xp = if success
        && s.cap > 1
        && r.kind != "dye"
        && (r.kind == "refine" || difficulty + 30 >= u64::from(rank))
    {
        if s.use_xp.is_empty() {
            (difficulty + 1) * 2
        } else {
            value(&s.use_xp, rank)
        }
    } else {
        0
    };
    let change = Change {
        skill: r.skill,
        xp: xp.min(1_000_000),
        time,
        raise: false,
        activity: None,
    };
    book.apply(&change)?;
    Ok(Plan {
        items,
        gold: value(&r.cost, request.spinner)
            .try_into()
            .map_err(|_| bad("Craft cost overflow"))?,
        change,
        success,
        name: r.name.clone(),
    })
}
pub fn validate_assets(
    dat: &mut crate::dat::Dat,
    metadata: &mut BTreeMap<u32, Option<Metadata>>,
) -> io::Result<()> {
    let mut valid = 0;
    for item in &catalog().items {
        if metadata.get(&item.id).is_some_and(|i| i.is_some()) {
            continue;
        }
        if dat.read(item.visual).is_err() {
            continue;
        }
        if let Ok(info) = Metadata::load(dat, item.id) {
            metadata.insert(item.id, Some(info));
            valid += 1;
        }
    }
    println!(
        "Professions: {} disciplines, {} recovered production recipes; {valid} additional item profiles.",
        catalog().skills.len(),
        catalog().recipes.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_curves_and_recipes_are_bounded() {
        assert_eq!(catalog().skills.len(), 26);
        assert!(catalog().recipes.len() > 250);
        for s in &catalog().skills {
            assert!(s.xp.len() > s.cap as usize);
            assert!(s.xp.windows(2).all(|v| v[0] <= v[1]));
        }
        for r in &catalog().recipes {
            assert!(skill(r.skill).is_some());
            assert!(r.minimum <= r.maximum);
            for i in r.inputs(r.minimum) {
                assert!(value(&i.quantities, r.minimum) > 0, "{} {}", r.name, i.name);
            }
        }
    }
    #[test]
    fn advancement_is_separate_capped_and_resets_after_twenty_two_hours() {
        let id = 0x6f0023a8;
        let c = Change {
            skill: id,
            xp: 5000,
            time: 100,
            raise: false,
            activity: None,
        };
        let b = Book::default().apply(&c).unwrap();
        assert_eq!(b.get(id).available, 1728);
        let raised = b
            .apply(&Change {
                skill: id,
                xp: 0,
                time: 101,
                raise: true,
                activity: None,
            })
            .unwrap();
        assert_eq!(raised.get(id).level, 2);
        assert_eq!(raised.get(id).available, 1688);
        let capped = raised
            .apply(&Change {
                time: 102,
                ..c.clone()
            })
            .unwrap();
        assert_eq!(capped.get(id).available, 1688);
        let reset = capped
            .apply(&Change {
                time: 100 + WINDOW,
                ..c
            })
            .unwrap();
        assert_eq!(reset.get(id).available, 3416);
        assert!(
            Book::default()
                .apply(&Change {
                    skill: id,
                    xp: 0,
                    time: 100,
                    raise: true,
                    activity: None
                })
                .is_err()
        );
        assert_eq!(reset.get(0x6f0023a9).level, 1);
    }
    #[test]
    fn chance_anchors() {
        for (d, c) in [(50, 10), (30, 30), (10, 70), (0, 80)] {
            assert_eq!(chance(50, 50 + d), c)
        }
        assert_eq!(chance(80, 50), 100);
        assert_eq!(chance(1, 80), 0);
    }
    fn stock(bag: &Inventory, definition: u32, qty: u32) -> Item {
        let d = catalog().items.iter().find(|i| i.id == definition).unwrap();
        Item {
            id: bag.high_water() + 1,
            definition,
            visual: d.visual,
            quantity: qty,
            stack_limit: u32::from(u16::MAX),
            equipment_mask: 0,
            location: Location::Bag(bag.free_slot().unwrap()),
            roll: None,
        }
    }
    fn inputs(r: &Recipe, level: u32) -> (Inventory, crate::crafting::ExecuteRequest) {
        let mut bag = Inventory::default();
        let mut ingredients = BTreeMap::new();
        for i in r.inputs(level) {
            let item = stock(&bag, i.choices[0], value(&i.quantities, level) as u32);
            ingredients.insert(i.ordinal, item.id);
            bag = bag.apply(&Command::Grant(item)).unwrap();
        }
        let primary = ingredients.values().next().copied().unwrap_or(1);
        (
            bag,
            crate::crafting::ExecuteRequest {
                recipe: r.id,
                spinner: level,
                primary,
                ingredients,
            },
        )
    }
    fn profiles() -> BTreeMap<u32, Option<Metadata>> {
        catalog()
            .items
            .iter()
            .map(|d| {
                (
                    d.id,
                    Some(Metadata {
                        implement: None,
                        icon: 0,
                        physical: d.id,
                        equipment: None,
                    }),
                )
            })
            .collect()
    }
    #[test]
    fn creation_tool_gates_bonus_and_wears_atomically_on_failed_attempts() {
        use crate::inventory::Roll;
        let skill = 0x6f0023a8;
        let mut book = Book::default();
        let (mut bag, request) = inputs(recipe(0x6f00261c).unwrap(), 1);
        let mut tool = stock(&bag, 0x47002ded, 1);
        tool.equipment_mask = 0x40;
        tool.stack_limit = 1;
        tool.roll = Some(Roll {
            level: 1,
            crafted: true,
            ..Default::default()
        });
        bag = bag.apply(&Command::Grant(tool.clone())).unwrap();
        assert!(!creation_tool_usable(&book, &tool));
        book.0.insert(
            skill,
            Progress {
                level: 10,
                ..Default::default()
            },
        );
        assert!(creation_tool_usable(&book, &tool));
        assert!(
            creation_bonus(&book, &bag, skill).is_none(),
            "A tool in the bag is not equipped"
        );
        bag = bag
            .apply(&Command::Wear {
                item: tool.id,
                slot: 0x40,
            })
            .unwrap();
        assert_eq!(creation_bonus(&book, &bag, skill).unwrap().1, 1);
        assert!(creation_bonus(&book, &bag, 0x6f002857).is_none());
        let plan = plan(
            &book,
            &bag,
            &request,
            bag.high_water() + 1,
            4199,
            100,
            &profiles(),
        )
        .unwrap();
        assert!(!plan.success);
        let next = plan.items.iter().fold(bag.clone(), |b, c| {
            b.apply(&Command::decode(&c.encode()).unwrap()).unwrap()
        });
        assert_eq!(
            next.items()
                .find(|i| i.id == tool.id)
                .unwrap()
                .roll
                .unwrap()
                .wear,
            1
        );
        assert_eq!(
            bag.items()
                .find(|i| i.id == tool.id)
                .unwrap()
                .roll
                .unwrap()
                .wear,
            0
        );
        let exhausted = next
            .apply(&Command::ToolWear {
                item: tool.id,
                previous: 1,
                used: 20,
            })
            .unwrap();
        assert!(creation_bonus(&book, &exhausted, skill).is_none());
        let worn = exhausted.items().find(|i| i.id == tool.id).unwrap();
        assert!(!creation_tool_usable(&book, worn));
        let unequipped = exhausted
            .apply(&Command::Unequip { item: tool.id })
            .unwrap();
        let worn = unequipped.items().find(|i| i.id == tool.id).unwrap();
        assert!(
            Inventory::default()
                .apply(
                    &Command::decode(
                        &Command::Grant(Item {
                            location: Location::Bag(0),
                            ..worn.clone()
                        })
                        .encode()
                    )
                    .unwrap()
                )
                .is_ok()
        );
    }
    #[test]
    fn crafting_is_atomic_across_partial_stacks_failure_gold_replay_and_restart() {
        use crate::{inventory::ITEM_NAMESPACE, progression::Command as P, store::Store};
        let dir = std::env::temp_dir().join(format!(
            "ac2-profession-{}-{}",
            std::process::id(),
            crate::recovered_quests::now()
        ));
        let skill = 0x6f0023a8;
        let metadata = profiles();
        let expected;
        {
            let mut store = Store::open(&dir).unwrap();
            let id = store.create("local", "Artisan").unwrap().id;
            for qty in [5, 95] {
                let item = stock(&store.characters[&id].inventory, 0x470010fe, qty);
                store
                    .inventory_command("local", id, Command::Grant(item))
                    .unwrap();
            }
            let request = crate::crafting::ExecuteRequest {
                recipe: 0x6f00261c,
                spinner: 1,
                primary: ITEM_NAMESPACE + 1,
                ingredients: BTreeMap::from([(2, ITEM_NAMESPACE + 1)]),
            };
            assert!(allowed(
                &crate::crafting::ValidateRequest {
                    recipe: request.recipe,
                    spinner: 1,
                    ordinal: 2,
                    item: request.primary
                },
                &store.characters[&id].inventory
            ));
            let make = |s: &Store, random| {
                let c = &s.characters[&id];
                plan(
                    &c.professions,
                    &c.inventory,
                    &request,
                    s.next_item_id().unwrap(),
                    random,
                    100,
                    &metadata,
                )
                .unwrap()
            };
            let failure = make(&store, 99);
            assert!(!failure.success);
            assert_eq!(failure.change.xp, 0);
            let crafted = make(&store, 0);
            assert!(crafted.success);
            assert_eq!(crafted.change.xp, 14);
            let initial = store.characters[&id].inventory.clone();
            let bytes = std::fs::metadata(dir.join("characters.journal"))
                .unwrap()
                .len();
            assert!(
                store
                    .profession_transaction(
                        "local",
                        id,
                        &crafted.items,
                        &[P::SpendGold(1)],
                        &crafted.change
                    )
                    .is_err()
            );
            assert!(
                store
                    .profession_transaction("other", id, &crafted.items, &[], &crafted.change)
                    .is_err()
            );
            assert_eq!(initial, store.characters[&id].inventory);
            assert_eq!(
                bytes,
                std::fs::metadata(dir.join("characters.journal"))
                    .unwrap()
                    .len()
            );
            store
                .profession_transaction("local", id, &crafted.items, &[], &crafted.change)
                .unwrap();
            assert!(
                store
                    .profession_transaction("local", id, &crafted.items, &[], &crafted.change)
                    .is_err()
            );
            let c = &store.characters[&id];
            assert_eq!(quantity(&c.inventory, 0x470010fe), 87);
            assert_eq!(c.professions.get(skill).available, 14);
            assert_eq!(
                c.inventory
                    .items()
                    .find(|i| i.definition == 0x47002aed)
                    .unwrap()
                    .roll
                    .unwrap()
                    .armor,
                10
            );
            assert!(
                plan(
                    &c.professions,
                    &c.inventory,
                    &request,
                    store.next_item_id().unwrap(),
                    0,
                    101,
                    &metadata
                )
                .is_err()
            );
            for _ in 0..2 {
                let c = &store.characters[&id];
                let material = c
                    .inventory
                    .items()
                    .find(|i| i.definition == 0x470010fe)
                    .unwrap()
                    .id;
                let req = crate::crafting::ExecuteRequest {
                    primary: material,
                    ingredients: BTreeMap::from([(2, material)]),
                    ..request.clone()
                };
                let p = plan(
                    &c.professions,
                    &c.inventory,
                    &req,
                    store.next_item_id().unwrap(),
                    0,
                    102,
                    &metadata,
                )
                .unwrap();
                store
                    .profession_transaction("local", id, &p.items, &[], &p.change)
                    .unwrap();
            }
            store
                .profession_transaction(
                    "local",
                    id,
                    &[],
                    &[],
                    &Change {
                        skill,
                        xp: 0,
                        time: 103,
                        raise: true,
                        activity: None,
                    },
                )
                .unwrap();
            assert_eq!(store.characters[&id].professions.get(skill).level, 2);
            assert_eq!(store.characters[&id].professions.get(skill).available, 2);
            expected = store.characters[&id].clone();
            store.compact_journal().unwrap();
        }
        {
            let restored = Store::open(&dir).unwrap();
            assert_eq!(
                restored.characters[&expected.id].inventory,
                expected.inventory
            );
            assert_eq!(
                restored.characters[&expected.id].professions,
                expected.professions
            );
        }
        for name in ["characters.journal", "positions.snapshot"] {
            let f = dir.join(name);
            if f.exists() {
                std::fs::remove_file(f).unwrap();
            }
        }
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn refine_batches_give_material_quantities_not_single_items() {
        let r = recipe(0x6f002a34).unwrap();
        let (bag, request) = inputs(r, 0);
        let meta = profiles();
        for (rank, expected) in [(1, 33), (150, 83)] {
            let mut book = Book::default();
            book.0.insert(
                r.skill,
                Progress {
                    level: rank,
                    ..Default::default()
                },
            );
            let p = plan(&book, &bag, &request, bag.high_water() + 1, 99, 100, &meta).unwrap();
            assert!(p.success);
            let result = p.items.iter().fold(bag.clone(), |b, c| b.apply(c).unwrap());
            assert_eq!(quantity(&result, 0x470027a6), expected);
            assert_eq!(quantity(&result, 0x470010f3), 0);
        }
    }
    #[test]
    fn refinery_merges_output_when_remainders_fill_the_backpack() {
        let r = recipe(0x6f003315).unwrap();
        let mut bag = Inventory::default();
        let mut ingredients = BTreeMap::new();
        for i in r.inputs(1) {
            let item = stock(&bag, i.choices[0], value(&i.quantities, 1) as u32 + 1);
            ingredients.insert(i.ordinal, item.id);
            bag = bag.apply(&Command::Grant(item)).unwrap();
        }
        let steel = stock(&bag, 0x470027a6, 1);
        bag = bag.apply(&Command::Grant(steel.clone())).unwrap();
        while bag.free_slot().is_some() {
            let item = stock(&bag, crate::merchants::MUCOR_DEFINITION, 1);
            bag = bag.apply(&Command::Grant(item)).unwrap();
        }
        let request = crate::crafting::ExecuteRequest {
            recipe: r.id,
            spinner: 1,
            primary: *ingredients.values().next().unwrap(),
            ingredients,
        };
        let p = plan(
            &Book::default(),
            &bag,
            &request,
            bag.high_water() + 1,
            0,
            100,
            &profiles(),
        )
        .unwrap();
        let (next, _) = crate::player_transaction::candidate(
            &bag,
            &Default::default(),
            &p.items,
            &[],
            Some(bag.high_water() + 1),
        )
        .unwrap();
        assert_eq!(quantity(&next, steel.definition), 67);
        assert_eq!(quantity(&next, 0x470010f3), 1);
        assert_eq!(quantity(&next, 0x470010f5), 1);
        assert_eq!(next.items().count(), crate::inventory::BAG_SLOTS as usize);
        assert!(!next.items().any(|i| i.id == steel.id));
        assert_eq!(quantity(&bag, steel.definition), 1);
        assert!(
            crate::player_transaction::candidate(
                &next,
                &Default::default(),
                &p.items,
                &[],
                Some(next.high_water() + 1)
            )
            .is_err()
        );
    }
    #[test]
    fn dye_preserves_the_selected_items_stats_and_other_equipment() {
        let definition = 0x47002aed;
        let r = catalog()
            .recipes
            .iter()
            .find(|r| {
                r.kind == "dye"
                    && r.ingredients
                        .iter()
                        .any(|i| i.ordinal == 1 && i.choices.contains(&definition))
            })
            .unwrap();
        let mut bag = Inventory::default();
        let mut original = stock(&bag, definition, 1);
        original.stack_limit = 1;
        original.equipment_mask =
            crate::apparel::recovered_slot(definition, original.visual).unwrap();
        original.roll = Some(crate::inventory::Roll {
            armor: 147,
            tint: 123,
            level: 12,
            crafted: true,
            damage: 0,
            wear: 0,
        });
        bag = bag.apply(&Command::Grant(original.clone())).unwrap();
        let mut other = original.clone();
        other.id += 1;
        other.location = Location::Bag(1);
        bag = bag.apply(&Command::Grant(other.clone())).unwrap();
        let plant = stock(
            &bag,
            r.ingredients
                .iter()
                .find(|i| i.ordinal == 2)
                .unwrap()
                .choices[0],
            1,
        );
        bag = bag.apply(&Command::Grant(plant.clone())).unwrap();
        let request = crate::crafting::ExecuteRequest {
            recipe: r.id,
            spinner: 0,
            primary: original.id,
            ingredients: BTreeMap::from([(1, original.id), (2, plant.id)]),
        };
        let plan = plan(
            &Book::default(),
            &bag,
            &request,
            bag.high_water() + 1,
            99,
            100,
            &profiles(),
        )
        .unwrap();
        assert!(plan.success);
        assert_eq!(plan.gold, 750);
        assert_eq!(plan.change.xp, 0);
        let dyed = plan.items.iter().fold(bag, |b, c| b.apply(c).unwrap());
        let retained = dyed.items().find(|i| i.id == other.id).unwrap();
        assert_eq!(retained.roll, other.roll);
        assert_eq!(retained.definition, other.definition);
        assert!(
            !dyed
                .items()
                .any(|i| i.id == original.id || i.id == plant.id)
        );
        let result = dyed.items().find(|i| i.id != other.id).unwrap();
        let roll = result.roll.unwrap();
        assert_eq!(
            (roll.armor, roll.level, roll.damage, roll.crafted),
            (147, 12, 0, true)
        );
        let tint = catalog()
            .items
            .iter()
            .find(|d| d.id == plant.definition)
            .unwrap()
            .dye
            .unwrap();
        assert_eq!(roll.tint, (tint.clamp(0., 1.) * 1000.).round() as u16);
    }
    #[test]
    fn every_enabled_recipe_product_has_runtime_metadata_in_installed_dat() {
        let Ok(path) = std::env::var("AC2_TEST_GAME_DATA") else {
            return;
        };
        let mut dat =
            crate::dat::Dat::open(std::path::Path::new(&path).join("portal.dat")).unwrap();
        let mut meta = BTreeMap::new();
        validate_assets(&mut dat, &mut meta).unwrap();
        let mut errors = Vec::new();
        for r in &catalog().recipes {
            if r.kind == "dye" {
                continue;
            }
            let (bag, request) = inputs(r, r.minimum);
            for output in &r.outputs {
                let random = ((output.threshold - 1.).max(0.) as u32) * 100;
                match plan(
                    &Book::default(),
                    &bag,
                    &request,
                    bag.high_water() + 1,
                    random,
                    100,
                    &meta,
                ) {
                    Ok(_) => {}
                    Err(e) => {
                        if chance(1, value(&r.difficulty, r.minimum)) > 0 || r.kind == "refine" {
                            errors.push(format!("{:08x} {}: {e}", r.id, r.name));
                        }
                    }
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }
}
