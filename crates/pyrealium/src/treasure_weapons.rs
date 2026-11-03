//! Observed weapon identities; generated damage is explicitly Pyrealium policy.
use crate::{dat::Dat, inventory::{Item, Location, Roll}, inventory_request::Metadata};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, io, sync::OnceLock};

#[derive(Deserialize)]
pub struct Profile {
    pub definition: u32,
    pub visual: u32,
    pub mask: u16,
    pub implement: u32,
    pub minimum_level: u16,
    damage: Option<u16>,
    sources: BTreeMap<String, String>,
}
fn profiles() -> &'static BTreeMap<u32, Profile> {
    static PROFILES: OnceLock<BTreeMap<u32, Profile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        #[derive(Deserialize)]
        struct File { schema: u32, weapons: Vec<Profile> }
        let file: File = serde_json::from_str(include_str!("../../../content/treasure-weapons.json")).expect("checked weapon catalog");
        assert_eq!(file.schema, 1);
        let mut rows = BTreeMap::new();
        for p in file.weapons {
            assert!(p.definition >> 24 == 0x47 && p.visual >> 24 == 0x1f && matches!(p.mask, 0x40 | 0xc0)
                && p.implement.count_ones() == 1 && (1..=150).contains(&p.minimum_level)
                && p.damage.is_none_or(|n| (1..=1000).contains(&n)) && p.sources.len() == 4);
            assert!(rows.insert(p.definition, p).is_none());
        }
        rows
    })
}
pub fn profile(item: &Item) -> Option<&'static Profile> {
    profiles().get(&item.definition).filter(|p| p.visual == item.visual && p.mask == item.equipment_mask)
}
pub fn compatible(dat: &mut Dat, metadata: &BTreeMap<u32, Option<Metadata>>) -> BTreeSet<u32> {
    let mut hashes = BTreeMap::new();
    let rows: BTreeSet<_> = profiles().values().filter(|p| {
        metadata.get(&p.definition).and_then(|m| *m).is_some_and(|m|
            m.implement == Some(p.implement) && m.equipment.is_some_and(|e| e.valid == u32::from(p.mask) && e.preferred == 0x40))
        && p.sources.iter().all(|(key, expected)| {
            let Ok(did) = u32::from_str_radix(key, 16) else { return false };
            let actual = hashes.entry(did).or_insert_with(|| dat.read(did).ok().map(|b| format!("{:x}", Sha256::digest(b))));
            actual.as_ref() == Some(expected)
        })
    }).map(|p| p.definition).collect();
    println!("Treasure weapons: {} compatible observed profiles; {} withheld.", rows.len(), profiles().len() - rows.len());
    rows
}
pub fn roll(item: &mut Item, level: u16, random: &mut u64) {
    let Some(p) = profile(item) else { return };
    let level = level.max(p.minimum_level).clamp(1, 150);
    let base = p.damage.unwrap_or(6 + level * 2);
    let damage = (f32::from(base) * (0.85 + crate::creature_ai::random(random) * 0.30)).round().clamp(1., 1000.) as u16;
    item.roll = Some(Roll { damage, level, ..Default::default() });
}
/// Local journal validation is independent of the currently mounted DAT build.
pub fn valid_finish(item: &Item) -> bool {
    profile(item).is_some_and(|p| item.roll.is_some_and(|r|
        !r.crafted && r.armor == 0 && r.damage > 0 && r.tint == 0 && r.wear == 0 && r.level >= p.minimum_level))
}
pub fn check_level(item: &Item, level: u16) -> io::Result<()> {
    if let Some(p) = profile(item) {
        if level < p.minimum_level {
            return Err(io::Error::other(format!("This weapon requires level {}.", p.minimum_level)));
        }
    }
    Ok(())
}
pub fn equipped_damage(inventory: &crate::inventory::Inventory) -> Option<u32> {
    inventory.items().find(|i| i.location == Location::Equipped(0x40))
        .filter(|i| valid_finish(i)).and_then(|i| i.roll).map(|r| u32::from(r.damage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Command, Inventory, ITEM_NAMESPACE};
    fn weapon(definition: u32) -> Item {
        let p = &profiles()[&definition];
        Item { id: ITEM_NAMESPACE + 1, definition, visual: p.visual, equipment_mask: p.mask,
            quantity: 1, stack_limit: 1, location: Location::Bag(0), roll: None }
    }
    #[test]
    fn loot_weapons_round_trip_without_becoming_crafted_or_armor() {
        assert_eq!(profiles().len(), 69);
        assert!(!profiles().contains_key(&0x470026bc), "Unimplemented usage restriction stays withheld");
        let mut random = 7;
        for &did in profiles().keys() {
            let mut item = weapon(did); roll(&mut item, 50, &mut random);
            let grant = Command::Grant(item.clone());
            let inventory = Inventory::default().apply(&Command::decode(&grant.encode()).unwrap()).unwrap();
            assert_eq!(inventory.items().next(), Some(&item));
            let equipped = inventory.apply(&Command::Wear {item:item.id, slot:0x40}).unwrap();
            assert_eq!(equipped_damage(&equipped), Some(u32::from(item.roll.unwrap().damage)));
            assert!(crate::merchants::sale_value(&item).is_some());
            for bad in [Item {visual:0x1f000001,..item.clone()}, Item {equipment_mask:0x20,..item.clone()},
                Item {roll:Some(Roll {armor:1,..item.roll.unwrap()}),..item.clone()}] {
                assert!(Inventory::default().apply(&Command::Grant(bad)).is_err());
            }
        }
    }
    #[test]
    fn native_level_gate_is_separate_from_generated_treasure_level() {
        let mut item = weapon(0x47000f3b); let mut random = 42; roll(&mut item, 50, &mut random);
        assert!(check_level(&item, 14).is_err()); assert!(check_level(&item, 15).is_ok());
        assert_eq!(item.roll.unwrap().level, 50);
        assert_eq!(profile(&item).unwrap().minimum_level, 15);
    }
}
