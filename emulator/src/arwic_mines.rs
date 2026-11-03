//! Version-one Arwic Mines lesson. Native assets/text and guide evidence are
//! pinned in content/arwic-mines-v1.json. `m1` transactions replay this policy.
use crate::{
    inventory::{Command, Inventory, Item, Location},
    quests::Book,
};
use serde::Deserialize;
use std::{collections::BTreeMap, io, sync::OnceLock};

pub const QUEST: u32 = 0x400000c1;
pub const NPC: u32 = 0x470015ad;
pub const OUTCAST: u32 = 0x470015ac;
pub const HAMMER: u32 = 0x470015a9;
pub const SWORD: u32 = 0x470015aa;
pub const IRON: u32 = 0x470010f3;
pub const GLYPH: u32 = 0x4700109f;
pub const GUIDE: u32 = 0x47002917;
pub const RECIPE: u32 = 0x6f001f0e;
pub const REPEAT: u64 = 20 * 60 * 60;
fn bad(s: &str) -> io::Error {
    io::Error::other(s)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    // 0 absent, 1 find hammer, 2 return hammer, 3 craft, 4 return sword, 5 done.
    pub phase: u32,
    pub started: u64,
    pub completed: u64,
    pub learned: bool,
}
#[derive(Deserialize)]
pub struct Definition {
    pub id: u32,
    pub name: String,
    pub visual: u32,
    physical: u32,
    quality_hash: String,
}
#[derive(Deserialize)]
pub struct Content {
    pub title: String,
    pub summary: String,
    pub phases: Vec<String>,
    pub items: Vec<Definition>,
    pub guide_pages: Vec<String>,
}
pub fn content() -> &'static Content {
    static C: OnceLock<Content> = OnceLock::new();
    C.get_or_init(|| {
        serde_json::from_str(include_str!("../content/arwic-mines-v1.json"))
            .expect("checked Arwic lesson")
    })
}
pub fn name(id: u32) -> Option<&'static str> {
    content()
        .items
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.name.as_str())
}
pub fn read_page(page: usize) -> Result<Vec<u8>, crate::world::WorldError> {
    let pages = &content().guide_pages;
    let page = page % pages.len();
    crate::interaction::dialogue(
        "The Shaper's Guide",
        &format!(
            "Page {}/{}. {} Double-click the book again for the next page.",
            page + 1,
            pages.len(),
            pages[page]
        ),
    )
}
pub fn item(definition: u32, id: u64, slot: u16, quantity: u32) -> io::Result<Item> {
    let d = content()
        .items
        .iter()
        .find(|d| d.id == definition)
        .ok_or_else(|| bad("Unknown lesson item"))?;
    Ok(Item {
        id,
        definition,
        visual: d.visual,
        quantity,
        stack_limit: if definition == IRON { u32::from(u16::MAX) } else { 1 },
        equipment_mask: if definition == SWORD || definition == HAMMER {
            0x40
        } else {
            0
        },
        location: Location::Bag(slot),
        roll: None,
    })
}
pub fn validate(
    dat: &mut crate::dat::Dat,
) -> io::Result<BTreeMap<u32, crate::inventory_request::Metadata>> {
    let mut result = BTreeMap::new();
    for d in &content().items {
        let raw = dat.read(0x81000000 | (d.id & 0xffffff))?;
        let hash = raw.iter().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        if format!("{hash:016x}") != d.quality_hash {
            return Err(bad("Arwic lesson DAT profile changed"));
        }
        dat.read(d.id)?;
        dat.read(d.visual)?;
        dat.read(d.physical)?;
        // Glyph names use native StringInfo substitutions. This fingerprinted
        // profile supplies their bag transport without guessing prefix offsets.
        let info = if d.id == GLYPH {
            crate::inventory_request::Metadata {
                implement: None,
                icon: 0,
                physical: d.physical,
                equipment: None,
            }
        } else {
            crate::inventory_request::Metadata::load(dat, d.id)?
        };
        result.insert(d.id, info);
    }
    Ok(result)
}
impl State {
    pub fn guidance(&self) -> String {
        if (1..=4).contains(&self.phase) {
            content().phases[(self.phase - 1) as usize].clone()
        } else if self.phase == 5 {
            "Lesson complete. You received 150 experience, an Arwic Outlands Glyph and The Shaper's Guide. You may repeat this lesson after 20 hours.".into()
        } else {
            "Speak to the Arwic Mines Overseer in Arwic (44.0N, 6.1W).".into()
        }
    }
    pub fn allowed(&self, recipe: u32) -> bool {
        recipe != RECIPE || self.learned
    }
    pub fn can_loot(&self, bag: &Inventory) -> bool {
        self.phase == 1 && !bag.items().any(|i| i.definition == HAMMER)
    }
}
pub fn owned(bag: &Inventory, definition: u32) -> Option<u64> {
    bag.items()
        .find(|i| i.definition == definition && matches!(i.location, Location::Bag(_)))
        .map(|i| i.id)
}
/// The server verifies corpse ownership, range and its pending hammer before
/// authorizing Hammer. Talk is only dispatched by the observed Overseer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Talk,
    Hammer,
    Craft(u64),
}
impl Action {
    pub fn encode(self) -> String {
        match self {
            Self::Talk => "talk".into(),
            Self::Hammer => "hammer".into(),
            Self::Craft(id) => format!("craft,{id}"),
        }
    }
    pub fn decode(s: &str) -> io::Result<Self> {
        match s {
            "talk" => Ok(Self::Talk),
            "hammer" => Ok(Self::Hammer),
            _ => s
                .strip_prefix("craft,")
                .and_then(|s| s.parse().ok())
                .map(Self::Craft)
                .ok_or_else(|| bad("Invalid Arwic action")),
        }
    }
}
pub struct Plan {
    pub items: Vec<Command>,
    pub rewards: Vec<crate::progression::Command>,
    pub quests: Book,
}
pub fn plan(
    book: &Book,
    bag: &Inventory,
    action: Action,
    first: u64,
    time: u64,
) -> io::Result<Plan> {
    if time == 0 || time < book.arwic.started || time < book.arwic.completed || !bag.dense_bags() {
        return Err(bad("Invalid lesson state or clock"));
    }
    let mut next = book.clone();
    let mut items = Vec::new();
    let mut rewards = Vec::new();
    let mut bag = bag.clone();
    let mut id = first;
    let mut grant =
        |definition, quantity, bag: &mut Inventory, items: &mut Vec<Command>| -> io::Result<()> {
            let slot = bag
                .free_slot()
                .ok_or_else(|| bad("Make room in your backpack for the lesson reward."))?;
            let command = Command::Grant(item(definition, id, slot, quantity)?);
            *bag = bag.apply(&command).map_err(io::Error::other)?;
            items.push(command);
            id = id
                .checked_add(1)
                .ok_or_else(|| bad("Item identity overflow"))?;
            Ok(())
        };
    match action {
        Action::Talk => {
            let referral = crate::exploration_quests::catalog().referral.id;
            if next
                .exploration
                .0
                .get(&referral)
                .is_some_and(|s| s.completed == 0)
            {
                next.exploration = next.exploration.converse(NPC, 1, time)?.book;
            }
            match next.arwic.phase {
                0 | 5
                    if next.arwic.phase == 0
                        || time.saturating_sub(next.arwic.completed) >= REPEAT =>
                {
                    next.arwic.phase = if owned(&bag, HAMMER).is_some() { 2 } else { 1 };
                    next.arwic.started = time;
                }
                1 if owned(&bag, HAMMER).is_some() => next.arwic.phase = 2,
                2 => {
                    if owned(&bag, HAMMER).is_none() {
                        return Err(bad("Bring the Crude Flat Hammer in your backpack."));
                    }
                    // Keep the hammer. Merge compatible Iron before reserving a
                    // slot; replay uses precisely the same inventory commands.
                    let mut amount = 20;
                    let merge = bag
                        .items()
                        .find(|i| {
                            i.definition == IRON
                                && i.roll.is_none()
                                && i.quantity <= u32::from(u16::MAX) - 20
                                && matches!(i.location, Location::Bag(_))
                        })
                        .cloned();
                    if let Some(old) = merge {
                        amount += old.quantity;
                        let c = Command::ConsumeStackDense {
                            item: old.id,
                            quantity: old.quantity,
                        };
                        bag = bag.apply(&c).map_err(io::Error::other)?;
                        items.push(c);
                    }
                    grant(IRON, amount, &mut bag, &mut items)?;
                    next.arwic.phase = 3;
                    next.arwic.learned = true;
                }
                4 => {
                    let sword = owned(&bag, SWORD).ok_or_else(|| {
                        bad("Bring the Arwic Mines Short Sword in your backpack.")
                    })?;
                    let c = Command::ConsumeDense { item: sword };
                    bag = bag.apply(&c).map_err(io::Error::other)?;
                    items.push(c);
                    grant(GLYPH, 1, &mut bag, &mut items)?;
                    grant(GUIDE, 1, &mut bag, &mut items)?;
                    rewards.push(crate::progression::Command::AwardXp(150));
                    next.arwic.phase = 5;
                    next.arwic.completed = time;
                }
                _ => {}
            }
        }
        Action::Hammer => {
            if !next.arwic.can_loot(&bag) {
                return Err(bad(
                    "Accept the Arwic Mines lesson before collecting its hammer; you only need one.",
                ));
            }
            grant(HAMMER, 1, &mut bag, &mut items)?;
            next.arwic.phase = 2;
        }
        Action::Craft(primary) => {
            if !next.arwic.learned {
                return Err(bad(
                    "The Arwic Mines Overseer must teach you this recipe first.",
                ));
            }
            if !bag.items().any(|i| {
                i.id == primary && i.definition == IRON && matches!(i.location, Location::Bag(_))
            }) {
                return Err(bad("Select Iron from your backpack."));
            }
            let mut stacks = bag
                .items()
                .filter(|i| i.definition == IRON && matches!(i.location, Location::Bag(_)))
                .cloned()
                .collect::<Vec<_>>();
            stacks.sort_by_key(|i| (i.id != primary, i.id));
            let mut need = 20;
            let mut remainder = 0;
            for stack in stacks {
                if need == 0 {
                    break;
                }
                let used = need.min(stack.quantity);
                need -= used;
                remainder = stack.quantity - used;
                let c = Command::ConsumeStackDense {
                    item: stack.id,
                    quantity: stack.quantity,
                };
                bag = bag.apply(&c).map_err(io::Error::other)?;
                items.push(c);
            }
            if need > 0 {
                return Err(bad("The sword requires 20 Iron."));
            }
            if remainder > 0 {
                grant(IRON, remainder, &mut bag, &mut items)?;
            }
            grant(SWORD, 1, &mut bag, &mut items)?;
            if next.arwic.phase == 3 {
                next.arwic.phase = 4;
            }
        }
    }
    Ok(Plan {
        items,
        rewards,
        quests: next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn first(b: &Inventory) -> u64 {
        b.high_water().max(crate::inventory::ITEM_NAMESPACE) + 1
    }
    fn apply(
        q: &mut Book,
        b: &mut Inventory,
        a: Action,
        t: u64,
    ) -> Vec<crate::progression::Command> {
        let p = plan(q, b, a, first(b), t).unwrap();
        for c in p.items {
            *b = b.apply(&c).unwrap();
        }
        *q = p.quests;
        p.rewards
    }
    fn quantity(b: &Inventory, d: u32) -> u32 {
        b.items()
            .filter(|i| i.definition == d)
            .map(|i| u32::from(i.quantity))
            .sum()
    }
    fn learned() -> (Book, Inventory) {
        let (mut q, mut b) = (Book::default(), Inventory::default());
        apply(&mut q, &mut b, Action::Talk, 100);
        apply(&mut q, &mut b, Action::Hammer, 101);
        apply(&mut q, &mut b, Action::Talk, 102);
        (q, b)
    }
    #[test]
    fn lesson_requires_acceptance_then_retains_hammer_and_awards_once() {
        let (mut q, mut b) = (Book::default(), Inventory::default());
        assert!(!q.arwic.allowed(RECIPE));
        assert!(plan(&q, &b, Action::Hammer, first(&b), 100).is_err());
        assert!(plan(&q, &b, Action::Craft(1), first(&b), 100).is_err());
        apply(&mut q, &mut b, Action::Talk, 100);
        assert_eq!(q.arwic.phase, 1);
        apply(&mut q, &mut b, Action::Hammer, 101);
        assert_eq!(q.arwic.phase, 2);
        assert!(plan(&q, &b, Action::Hammer, first(&b), 102).is_err());
        apply(&mut q, &mut b, Action::Talk, 102);
        assert_eq!(q.arwic.phase, 3);
        apply(&mut q, &mut b, Action::Talk, 103);
        assert_eq!((quantity(&b, HAMMER), quantity(&b, IRON)), (1, 20));
        assert!(q.arwic.allowed(RECIPE));
        let iron = owned(&b, IRON).unwrap();
        apply(&mut q, &mut b, Action::Craft(iron), 104);
        assert_eq!(q.arwic.phase, 4);
        assert_eq!(quantity(&b, IRON), 0);
        let rewards = apply(&mut q, &mut b, Action::Talk, 105);
        assert!(matches!(
            rewards.as_slice(),
            [crate::progression::Command::AwardXp(150)]
        ));
        assert_eq!(
            (
                quantity(&b, SWORD),
                quantity(&b, GLYPH),
                quantity(&b, GUIDE)
            ),
            (0, 1, 1)
        );
        assert_eq!(q.arwic.phase, 5);
        let old = b.clone();
        assert!(apply(&mut q, &mut b, Action::Talk, 105 + REPEAT - 1).is_empty());
        assert_eq!(b, old);
        apply(&mut q, &mut b, Action::Talk, 105 + REPEAT);
        assert_eq!(q.arwic.phase, 2);
        assert!(q.arwic.learned, "learning survives repeat acceptance");
        assert!(plan(&q, &b, Action::Talk, first(&b), 104).is_err());
    }
    #[test]
    fn craft_consumes_across_stacks_and_retains_exact_remainder() {
        let (mut q, mut b) = learned();
        let id = owned(&b, IRON).unwrap();
        b = b
            .apply(&Command::ConsumeStackDense {
                item: id,
                quantity: 20,
            })
            .unwrap();
        for qty in [7, 9, 12] {
            b = b
                .apply(&Command::Grant(
                    item(IRON, first(&b), b.free_slot().unwrap(), qty).unwrap(),
                ))
                .unwrap();
        }
        let primary = owned(&b, IRON).unwrap();
        apply(&mut q, &mut b, Action::Craft(primary), 103);
        assert_eq!(quantity(&b, IRON), 8);
        assert_eq!(quantity(&b, SWORD), 1);
        assert!(b.dense_bags());
        let primary = owned(&b, IRON).unwrap();
        let old = b.clone();
        assert!(plan(&q, &b, Action::Craft(primary), first(&b), 104).is_err());
        assert_eq!(b, old);
    }
    #[test]
    fn full_backpack_cannot_lose_sword_or_gain_partial_rewards() {
        let (mut q, mut b) = learned();
        let iron = owned(&b, IRON).unwrap();
        apply(&mut q, &mut b, Action::Craft(iron), 103);
        while let Some(slot) = b.free_slot() {
            b = b
                .apply(&Command::Grant(item(GUIDE, first(&b), slot, 1).unwrap()))
                .unwrap();
        }
        let (oldq, oldb) = (q.clone(), b.clone());
        assert!(plan(&q, &b, Action::Talk, first(&b), 104).is_err());
        assert_eq!(q, oldq);
        assert_eq!(b, oldb);
        let remove = owned(&b, GUIDE).unwrap();
        b = b.apply(&Command::ConsumeDense { item: remove }).unwrap();
        assert_eq!(apply(&mut q, &mut b, Action::Talk, 105).len(), 1);
        assert!(b.free_slot().is_none());
    }
    #[test]
    fn installed_lesson_assets_and_packets_are_valid() {
        let Ok(path) = std::env::var("AC2_TEST_GAME_DATA") else {
            return;
        };
        let mut dat =
            crate::dat::Dat::open(std::path::Path::new(&path).join("portal.dat")).unwrap();
        let metadata = validate(&mut dat).unwrap();
        for (i, d) in content().items.iter().enumerate() {
            let item = item(
                d.id,
                crate::inventory::ITEM_NAMESPACE + i as u64 + 1,
                i as u16,
                1,
            )
            .unwrap();
            crate::item_services::acquired(1, &item, metadata[&d.id]).unwrap();
        }
        for phase in 1..=5 {
            let q = State {
                phase,
                ..Default::default()
            };
            let text = q
                .guidance()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            crate::interaction::dialogue("Arwic Mines and Crafting Quest", &text).unwrap();
        }
        assert_eq!(content().guide_pages.len(), 15);
        for page in 0..=15 {
            read_page(page).unwrap();
        }
    }
}
