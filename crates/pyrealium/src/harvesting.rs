//! Gathering uses recovered mine/map/tool assets. Lost yield bonuses and butcher
//! tables are explicitly reconstructed; all ownership/charges share the craft journal.
use crate::{
    inventory::{Command, Inventory, Item, Location},
    movement::Placement,
    professions::{self, Change, Plan},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};
pub const SURVEY: u32 = 0x6f00264a;
pub const BUTCHERY: u32 = 0x6f002560;
fn bad(s: &str) -> io::Error {
    io::Error::other(s)
}
#[derive(Debug, Deserialize)]
pub struct Mine {
    pub id: u32,
    pub visual: u32,
    pub material: u32,
    pub quantity: u32,
    pub uses: u32,
    pub variance: f32,
}
#[derive(Debug, Deserialize)]
pub struct Map {
    pub id: u32,
    pub region: usize,
    pub mine: u32,
    pub bounds: [i32; 4],
    pub radius: u32,
}
#[derive(Debug, Deserialize)]
pub struct Tool {
    pub id: u32,
    pub skill: u32,
    pub durability: u32,
    pub animation: u32,
}
#[derive(Debug, Deserialize)]
pub struct Butcher {
    pub id: u32,
    pub profile: u32,
    pub material: u32,
}
pub fn map(id: u32) -> Option<&'static Map> {
    professions::catalog().maps.iter().find(|v| v.id == id)
}
pub fn mine(id: u32) -> Option<&'static Mine> {
    professions::catalog().mines.iter().find(|v| v.id == id)
}
pub fn tool(id: u32) -> Option<&'static Tool> {
    professions::catalog().tools.iter().find(|v| v.id == id)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub definition: u32,
    pub target: [i32; 2],
    pub point: Option<[u32; 4]>,
    pub remaining: u32,
    pub expires: u64,
}
impl Job {
    pub fn placement(&self) -> Option<Placement> {
        let p = self.point?;
        Some(Placement {
            cell: p[0],
            coordinates: [
                f32::from_bits(p[1]),
                f32::from_bits(p[2]),
                f32::from_bits(p[3]),
            ],
            heading: 0,
        })
    }
    pub fn distance(&self, p: Placement) -> Option<f32> {
        if !(1..=64).contains(&(p.cell & 65535)) {
            return None;
        }
        let x = (p.cell >> 24) as f32 * 160. + p.coordinates[0];
        let y = ((p.cell >> 16) & 255) as f32 * 160. + p.coordinates[1];
        Some(((x - self.target[0] as f32).powi(2) + (y - self.target[1] as f32).powi(2)).sqrt())
    }
    pub fn guidance(&self) -> String {
        let e = (self.target[0] as f32 - 20400.) / 204.;
        let n = (self.target[1] as f32 - 20400.) / 204.;
        format!(
            "Survey site: {:.1}{}, {:.1}{}. Use the map within 40 meters, then use the appropriate tool on the deposit.",
            n.abs(),
            if n < 0. { "S" } else { "N" },
            e.abs(),
            if e < 0. { "W" } else { "E" }
        )
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Book {
    pub jobs: BTreeMap<u64, Job>,
    pub cooldown: [u64; 3],
    pub tool_uses: BTreeMap<u64, u32>,
    #[serde(with = "crate::db::pairs")]
    pub carcasses: BTreeMap<(u64, u16), u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Activity {
    Survey {
        item: u64,
        definition: u32,
        #[serde(default)]
        target: Option<[i32; 2]>,
    },
    Locate {
        item: u64,
        point: [u32; 4],
    },
    Mine {
        item: u64,
        tool: u64,
        definition: u32,
        #[serde(default)]
        wear: Option<u32>,
    },
    Butcher {
        corpse: u64,
        instance: u16,
        tool: u64,
        definition: u32,
        #[serde(default)]
        wear: Option<u32>,
    },
}
pub fn node_id(item: u64) -> u64 {
    0x8190000000000000 | (item & 0x0000ffffffffffff)
}
pub fn map_id(node: u64) -> u64 {
    crate::inventory::ITEM_NAMESPACE | (node & 0x0000ffffffffffff)
}
impl Book {
    pub fn apply(&mut self, a: &Activity, time: u64) -> io::Result<()> {
        self.carcasses.retain(|_, until| *until > time);
        // Unused maps stay valid; located mines expire after one hour.
        self.jobs.retain(|_, j| j.expires == 0 || j.expires > time);
        match *a {
            Activity::Survey {
                item,
                definition,
                target,
            } => {
                let m = map(definition).ok_or_else(|| bad("Unknown survey map"))?;
                if item >> 48 != 0x8130 || self.jobs.contains_key(&item) || self.jobs.len() >= 12 {
                    return Err(bad("Finish your existing survey maps first."));
                }
                if self.cooldown[m.region] > time {
                    return Err(bad(
                        "Surveying this continent is recovering. Try again after fifteen minutes.",
                    ));
                }
                let hash = item.wrapping_mul(0x9e3779b97f4a7c15);
                let target = target.unwrap_or_else(|| {
                    std::array::from_fn(|i| {
                        m.bounds[i]
                            + ((m.bounds[i + 2] - m.bounds[i]) as i64
                                * ((100 + ((hash >> (i * 32)) % 801)) as i64)
                                / 1000) as i32
                    })
                });
                if (0..2).any(|i| target[i] < m.bounds[i] || target[i] > m.bounds[i + 2]) {
                    return Err(bad("Survey target outside its native region"));
                }
                self.jobs.insert(
                    item,
                    Job {
                        definition,
                        target,
                        point: None,
                        remaining: mine(m.mine).ok_or_else(|| bad("Missing mine"))?.uses,
                        expires: 0,
                    },
                );
                self.cooldown[m.region] = time + 900;
            }
            Activity::Locate { item, point } => {
                let j = self
                    .jobs
                    .get_mut(&item)
                    .ok_or_else(|| bad("This map has no active survey."))?;
                if j.point.is_some() {
                    return Err(bad("This map was already used."));
                }
                let p = Placement {
                    cell: point[0],
                    coordinates: [
                        f32::from_bits(point[1]),
                        f32::from_bits(point[2]),
                        f32::from_bits(point[3]),
                    ],
                    heading: 0,
                };
                let m = map(j.definition).ok_or_else(|| bad("Unknown survey"))?;
                if !p.valid() || !j.distance(p).is_some_and(|d| d <= m.radius as f32) {
                    return Err(bad("Move closer to the survey site."));
                }
                j.point = Some(point);
                j.expires = time + 3600;
                self.cooldown[m.region] = time;
            }
            Activity::Mine {
                item,
                tool: tid,
                definition,
                wear,
            } => {
                let j = self
                    .jobs
                    .get_mut(&item)
                    .filter(|j| j.point.is_some() && j.remaining > 0)
                    .ok_or_else(|| {
                        bad("This deposit is exhausted or belongs to another player.")
                    })?;
                j.remaining -= 1;
                if j.remaining == 0 {
                    self.jobs.remove(&item);
                }
                self.use_tool(tid, definition, wear)?;
            }
            Activity::Butcher {
                corpse,
                instance,
                tool: tid,
                definition,
                wear,
            } => {
                if self.carcasses.contains_key(&(corpse, instance)) {
                    return Err(bad("This carcass has already been butchered."));
                }
                if self.carcasses.len() >= 512 {
                    return Err(bad("Too many recently butchered carcasses."));
                }
                self.carcasses.insert((corpse, instance), time + 3600);
                self.use_tool(tid, definition, wear)?;
            }
        }
        Ok(())
    }
    fn use_tool(&mut self, id: u64, definition: u32, wear: Option<u32>) -> io::Result<()> {
        let t = tool(definition).ok_or_else(|| bad("Unknown gathering tool"))?;
        let used = self.tool_uses.entry(id).or_default();
        // Missing wear preserves historical journal semantics. New records carry
        // per-item wear, including a tool obtained from another character.
        *used = wear.unwrap_or(*used + 1);
        if *used > t.durability {
            return Err(bad("Invalid tool durability"));
        }
        if *used >= t.durability {
            self.tool_uses.remove(&id);
        }
        Ok(())
    }
}
pub fn survey(
    book: &professions::Book,
    bag: &Inventory,
    r: &professions::Recipe,
    id: u64,
    random: u32,
    time: u64,
) -> io::Result<Plan> {
    let output = r
        .outputs
        .iter()
        .find(|o| f64::from(random % 1000) < o.threshold)
        .or(r.outputs.last())
        .ok_or_else(|| bad("Survey map missing"))?;
    let d = professions::catalog()
        .items
        .iter()
        .find(|d| d.id == output.item)
        .ok_or_else(|| bad("Map metadata missing"))?;
    let change = Change {
        skill: SURVEY,
        xp: professions::value(
            &professions::skill(SURVEY).unwrap().use_xp,
            book.get(SURVEY).level,
        ),
        time,
        raise: false,
        activity: Some(Activity::Survey {
            item: id,
            definition: d.id,
            target: None,
        }),
    };
    book.apply(&change)?;
    let item = Item {
        id,
        definition: d.id,
        visual: d.visual,
        quantity: 1,
        stack_limit: 1,
        equipment_mask: 0,
        location: Location::Bag(
            bag.free_slot()
                .ok_or_else(|| bad("Make room for your survey map."))?,
        ),
        roll: None,
    };
    Ok(Plan {
        items: vec![Command::Grant(item)],
        gold: 0,
        change,
        success: true,
        name: r.name.clone(),
    })
}
/// Check native terrain before granting a map. Persist the chosen coordinates in
/// the transaction so a future placement-policy change cannot move saved sites.
pub fn ground_survey(
    book: &professions::Book,
    change: &mut Change,
    geometry: &mut crate::geometry::Geometry,
) -> io::Result<()> {
    let Some(Activity::Survey {
        item,
        definition,
        target,
    }) = &mut change.activity
    else {
        return Ok(());
    };
    let m = map(*definition).ok_or_else(|| bad("Unknown survey map"))?;
    let mut preview = book.1.clone();
    preview.apply(
        &Activity::Survey {
            item: *item,
            definition: *definition,
            target: None,
        },
        change.time,
    )?;
    let center = preview.jobs[item].target;
    for n in 0..48 {
        let radius = if n == 0 { 0. } else { 20. * (n as f32).sqrt() };
        let angle = n as f32 * 2.399963;
        let hash = item.wrapping_add(n as u64).wrapping_mul(0x9e3779b97f4a7c15);
        let xy = if n < 16 {
            [
                center[0] + (radius * angle.cos()).round() as i32,
                center[1] + (radius * angle.sin()).round() as i32,
            ]
        } else {
            std::array::from_fn(|i| {
                let fraction = (100 + ((hash >> (i * 32)) % 801)) as i64;
                m.bounds[i] + (i64::from(m.bounds[i + 2] - m.bounds[i]) * fraction / 1000) as i32
            })
        };
        if (0..2)
            .any(|i| xy[i] < m.bounds[i] || xy[i] > m.bounds[i + 2] || !(0..40960).contains(&xy[i]))
        {
            continue;
        }
        let bx = (xy[0] / 160) as u32;
        let by = (xy[1] / 160) as u32;
        let x = (xy[0] % 160) as f32;
        let y = (xy[1] % 160) as f32;
        let p = Placement {
            cell: (bx << 24) | (by << 16) | ((x as u32 / 20) * 8 + y as u32 / 20 + 1),
            coordinates: [x, y, 0.],
            heading: 0,
        };
        if geometry
            .ground(p, 5000., 5000.)
            .is_some_and(|p| (1..=64).contains(&(p.cell & 65535)) && geometry.dry_ground(p))
        {
            *target = Some(xy);
            return Ok(());
        }
    }
    Err(bad(
        "No dry survey site was found in this region. No map or cooldown was consumed; try again.",
    ))
}
pub fn gather(
    book: &professions::Book,
    bag: &Inventory,
    tid: u64,
    mut activity: Activity,
    material: u32,
    base: u32,
    variance: f32,
    id: u64,
    random: u32,
    time: u64,
) -> io::Result<Plan> {
    let owned = bag
        .items()
        .find(|i| i.id == tid && matches!(i.location, Location::Bag(_)))
        .ok_or_else(|| bad("Use a gathering tool from your backpack."))?;
    let t = tool(owned.definition).ok_or_else(|| bad("That item is not a gathering tool."))?;
    let rank = book.get(t.skill).level;
    let level = owned.roll.map_or(1, |r| u32::from(r.level));
    if level > 1 && rank < level * 3 {
        return Err(bad(
            "Your gathering rank must be at least three times the tool level.",
        ));
    }
    let mut items = vec![];
    let mut next = bag.clone();
    let previous = owned.roll.map_or(0, |r| r.wear);
    let used = wear(book, owned) + (random >> 12 & 1);
    match &mut activity {
        Activity::Mine {
            tool,
            definition,
            wear,
            ..
        }
        | Activity::Butcher {
            tool,
            definition,
            wear,
            ..
        } if *tool == tid && *definition == owned.definition => *wear = Some(used),
        _ => return Err(bad("Gathering action does not match the owned tool.")),
    }
    if used >= t.durability {
        let c = Command::ConsumeDense { item: tid };
        next = next.apply(&c).map_err(io::Error::other)?;
        items.push(c);
    } else if used > u32::from(previous) {
        let c = Command::ToolWear {
            item: tid,
            previous,
            used: used as u16,
        };
        next = next.apply(&c).map_err(io::Error::other)?;
        items.push(c);
    }
    let survey_bonus = if t.skill == BUTCHERY {
        0
    } else {
        book.get(SURVEY).level * 40 / 150
    };
    let tool_bonus = level * 50 / 150;
    let amount = ((base + survey_bonus + tool_bonus) as f32
        * (1. + variance * ((random % 2001) as f32 / 1000. - 1.)))
        .round()
        .clamp(1., 65535.) as u32;
    let d = professions::catalog()
        .items
        .iter()
        .find(|d| d.id == material)
        .ok_or_else(|| bad("Gathered material unavailable"))?;
    // Merge with a compatible stack before asking for another bag slot. Reissue
    // its identity atomically, like crafting remainders, to invalidate stale use.
    let stack = next
        .items()
        .find(|i| {
            i.definition == material
                && i.visual == d.visual
                && i.roll.is_none()
                && i.equipment_mask == 0
                && i.stack_limit == u32::from(u16::MAX)
                && matches!(i.location, Location::Bag(_))
                && i.quantity.checked_add(amount).is_some()
        })
        .cloned();
    let quantity = if let Some(stack) = stack {
        let c = Command::ConsumeStackDense {
            item: stack.id,
            quantity: stack.quantity,
        };
        next = next.apply(&c).map_err(io::Error::other)?;
        items.push(c);
        stack.quantity + amount
    } else {
        amount
    };
    let item = Item {
        id,
        definition: d.id,
        visual: d.visual,
        quantity,
        stack_limit: u32::from(u16::MAX),
        equipment_mask: 0,
        location: Location::Bag(
            next.free_slot()
                .ok_or_else(|| bad("Make room for gathered materials."))?,
        ),
        roll: None,
    };
    next.apply(&Command::Grant(item.clone()))
        .map_err(io::Error::other)?;
    items.push(Command::Grant(item));
    let s = professions::skill(t.skill).unwrap();
    let change = Change {
        skill: t.skill,
        xp: professions::value(&s.use_xp, rank),
        time,
        raise: false,
        activity: Some(activity),
    };
    book.apply(&change)?;
    Ok(Plan {
        items,
        gold: 0,
        change,
        success: true,
        name: format!(
            "{} {amount}",
            if d.name.is_empty() {
                "Material"
            } else {
                &d.name
            }
        ),
    })
}

/// Old journals tracked wear per character. New item rolls carry it through
/// ground pickup/transfers and the next use migrates any historical count.
pub fn wear(book: &professions::Book, item: &Item) -> u32 {
    u32::from(item.roll.map_or(0, |r| r.wear))
        .max(book.1.tool_uses.get(&item.id).copied().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn survey_cooldown_location_charges_and_tool_wear_are_durable() {
        use crate::store::Store;
        let path = crate::db::testdb::fresh();
        let definition = 0x47002ed0;
        let id = crate::inventory::ITEM_NAMESPACE + 1;
        let mut store = Store::open(&path).unwrap();
        let player = store.create("local", "Gather Tester").unwrap().id;
        let change = Change {
            skill: SURVEY,
            xp: 10,
            time: 100,
            raise: false,
            activity: Some(Activity::Survey {
                item: id,
                definition,
                target: None,
            }),
        };
        store
            .profession_transaction("local", player, &[], &[], &change)
            .unwrap();
        assert!(
            store
                .profession_transaction("local", player, &[], &[], &change)
                .is_err()
        );
        let job = &store.characters[&player].professions.1.jobs[&id];
        let xy = job.target;
        let cell = ((xy[0] / 160) as u32) << 24 | ((xy[1] / 160) as u32) << 16 | 1;
        let p = [
            cell,
            ((xy[0] % 160) as f32).to_bits(),
            ((xy[1] % 160) as f32).to_bits(),
            0f32.to_bits(),
        ];
        let locate = Change {
            activity: Some(Activity::Locate { item: id, point: p }),
            time: 101,
            ..change.clone()
        };
        store
            .profession_transaction("local", player, &[], &[], &locate)
            .unwrap();
        assert!(
            store
                .profession_transaction("local", player, &[], &[], &locate)
                .is_err()
        );
        let use_tool = Change {
            skill: 0x6f002579,
            xp: 20,
            time: 102,
            raise: false,
            activity: Some(Activity::Mine {
                item: id,
                tool: id + 1,
                definition: 0x47002deb,
                wear: None,
            }),
        };
        for _ in 0..2 {
            store
                .profession_transaction("local", player, &[], &[], &use_tool)
                .unwrap();
        }
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store.characters[&player].professions.1.jobs[&id].remaining,
            1
        );
        assert_eq!(
            store.characters[&player].professions.1.tool_uses[&(id + 1)],
            2
        );
        store
            .profession_transaction("local", player, &[], &[], &use_tool)
            .unwrap();
        assert!(
            !store.characters[&player]
                .professions
                .1
                .jobs
                .contains_key(&id)
        );
        assert!(
            store
                .profession_transaction("local", player, &[], &[], &use_tool)
                .is_err()
        );
        drop(store);
    }
    #[test]
    fn wrong_location_and_double_butchering_reject_without_changing_book() {
        let b = professions::Book::default();
        let id = crate::inventory::ITEM_NAMESPACE + 1;
        let c = Change {
            skill: SURVEY,
            xp: 1,
            time: 100,
            raise: false,
            activity: Some(Activity::Survey {
                item: id,
                definition: 0x47002ed0,
                target: None,
            }),
        };
        let b = b.apply(&c).unwrap();
        assert!(
            b.apply(&Change {
                activity: Some(Activity::Locate {
                    item: id,
                    point: [0x76b80001, 0, 0, 0]
                }),
                ..c
            })
            .is_err()
        );
        let c = Change {
            skill: BUTCHERY,
            xp: 10,
            time: 100,
            raise: false,
            activity: Some(Activity::Butcher {
                corpse: 123,
                instance: 1,
                tool: id,
                definition: 0x47002de8,
                wear: None,
            }),
        };
        let b = b.apply(&c).unwrap();
        assert!(b.apply(&c).is_err());
    }
    #[test]
    fn gathering_merges_into_a_full_backpack_and_tool_break_is_atomic() {
        use crate::inventory::{BAG_SLOTS, ITEM_NAMESPACE};
        let mut bag = Inventory::default();
        for slot in 0..BAG_SLOTS {
            let definition = if slot == 0 { 0x47002de8 } else { 0x470027a3 };
            let d = professions::catalog()
                .items
                .iter()
                .find(|d| d.id == definition)
                .unwrap();
            bag = bag
                .apply(&Command::Grant(Item {
                    id: ITEM_NAMESPACE + u64::from(slot) + 1,
                    definition,
                    visual: d.visual,
                    quantity: 1,
                    stack_limit: if slot == 0 { 1 } else { u32::from(u16::MAX) },
                    equipment_mask: 0,
                    location: Location::Bag(slot),
                    roll: None,
                }))
                .unwrap();
        }
        assert!(bag.free_slot().is_none());
        let tid = ITEM_NAMESPACE + 1;
        let activity = Activity::Butcher {
            corpse: 123,
            instance: 1,
            tool: tid,
            definition: 0x47002de8,
            wear: None,
        };
        let b = professions::Book::default();
        let p = gather(
            &b,
            &bag,
            tid,
            activity.clone(),
            0x470027a3,
            5,
            0.,
            bag.high_water() + 1,
            0,
            100,
        )
        .unwrap();
        let next = p.items.iter().fold(bag.clone(), |b, c| b.apply(c).unwrap());
        assert_eq!(next.items().count(), usize::from(BAG_SLOTS));
        assert!(next.items().any(|i| i.quantity == 6));
        let b = b.apply(&p.change).unwrap();
        assert!(
            gather(
                &b,
                &next,
                tid,
                activity.clone(),
                0x470027a3,
                5,
                0.,
                next.high_water() + 1,
                0,
                101
            )
            .is_err()
        );
        let mut b = professions::Book::default();
        b.1.tool_uses
            .insert(tid, tool(0x47002de8).unwrap().durability - 1);
        let p = gather(
            &b,
            &bag,
            tid,
            activity,
            0x470027a3,
            5,
            0.,
            bag.high_water() + 1,
            4096,
            102,
        )
        .unwrap();
        let next = p.items.iter().fold(bag.clone(), |b, c| b.apply(c).unwrap());
        assert!(!next.items().any(|i| i.id == tid));
        assert!(!b.apply(&p.change).unwrap().1.tool_uses.contains_key(&tid));
        assert!(bag.items().any(|i| i.id == tid));
    }
    #[test]
    fn tool_wear_survives_item_transfer_and_no_wear_rolls() {
        use crate::inventory::{ITEM_NAMESPACE, Roll};
        let d = professions::catalog()
            .items
            .iter()
            .find(|d| d.id == 0x47002de8)
            .unwrap();
        let tool = Item {
            id: ITEM_NAMESPACE + 1,
            definition: d.id,
            visual: d.visual,
            quantity: 1,
            stack_limit: 1,
            equipment_mask: 0,
            location: Location::Bag(0),
            roll: Some(Roll {
                level: 1,
                crafted: true,
                wear: 6,
                ..Default::default()
            }),
        };
        let command = Command::decode(&Command::Grant(tool.clone()).encode()).unwrap();
        let bag = Inventory::default().apply(&command).unwrap();
        let book = professions::Book::default();
        assert_eq!(wear(&book, bag.items().next().unwrap()), 6);
        let activity = Activity::Butcher {
            corpse: 123,
            instance: 1,
            tool: tool.id,
            definition: d.id,
            wear: None,
        };
        for (random, expected) in [(0, 6), (4096, 7)] {
            let plan = gather(
                &book,
                &bag,
                tool.id,
                activity.clone(),
                0x470027a3,
                5,
                0.,
                tool.id + 1,
                random,
                100,
            )
            .unwrap();
            let next = plan.items.iter().fold(bag.clone(), |b, c| {
                b.apply(&Command::decode(&c.encode()).unwrap()).unwrap()
            });
            let owned = next.items().find(|i| i.id == tool.id).unwrap();
            assert_eq!(wear(&book, owned), expected);
            assert_eq!(wear(&book.apply(&plan.change).unwrap(), owned), expected);
        }
        assert!(
            bag.apply(&Command::ToolWear {
                item: tool.id,
                previous: 5,
                used: 7
            })
            .is_err()
        );
        assert!(
            bag.apply(&Command::ToolWear {
                item: tool.id,
                previous: 6,
                used: 5
            })
            .is_err()
        );
    }
}
