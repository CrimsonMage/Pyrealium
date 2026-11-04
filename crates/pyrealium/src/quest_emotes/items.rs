//! The items quest sets grant: their profiles, read from the DATs at start or by a live edit.
use super::*;

/// The item profiles a quest may grant beyond the Shelter's pinned five: entity DID to its
/// visual description and equipment slots, read from the DATs at start (`install_items`).
static ITEMS: RwLock<BTreeMap<u32, (u32, u16)>> = RwLock::new(BTreeMap::new());

/// A quest item `definition` as a new bag item: the Shelter's pinned profiles, else a profile
/// `install_items` read. (Unit tests, which have no DATs, give any other entity a placeholder
/// visual.)
pub fn item(definition: u32, id: u64, slot: u16) -> Option<Item> {
    if let Some(item) = crate::shelter::item(definition, id, slot) { return Some(item); }
    let profile = ITEMS.read().unwrap_or_else(|e| e.into_inner()).get(&definition).copied();
    let (visual, equipment_mask) = match profile {
        Some(profile) => profile,
        #[cfg(test)]
        None if definition >> 24 == 0x47 => (TEST_VISUAL, 0),
        None => return None,
    };
    Some(Item { roll: None, id, definition, visual, quantity: 1, stack_limit: 1, equipment_mask, location: Location::Bag(slot) })
}
#[cfg(test)]
pub const TEST_VISUAL: u32 = 0x1f00_fffe;

/// Reads every item the tables grant from the DATs and checks it as the loot catalog checks its
/// items (`loot::Catalog::load_with_rules`): the class defaults give the bag metadata, the visual
/// exists, and the acquisition notice encodes. Returns each item's profile and the metadata to
/// register (`register_items`); the admin thread reads a live edit's this way on its own DATs.
pub fn read_items(tables: &Tables, dat: &mut crate::dat::Dat) -> io::Result<Vec<ItemProfile>> {
    let mut out: Vec<ItemProfile> = Vec::new();
    for (set, entity) in tables.gives() {
        if crate::shelter::item(entity, crate::inventory::ITEM_NAMESPACE + 1, 0).is_some() || out.iter().any(|p| p.entity == entity) { continue; }
        let bad = |why: &str| io::Error::other(format!("emote set {set} names item {entity:08x}: {why}"));
        // As the tradeskills' items (`tradeskills::item`): the class defaults' bag metadata, and
        // the visual its physical record names.
        dat.read(entity).map_err(|e| bad(&e.to_string()))?;
        let info = crate::inventory_request::Metadata::load(dat, entity).map_err(|e| bad(&e.to_string()))?;
        let visual = crate::tradeskills::visual_of(dat, info.physical).map_err(|e| bad(&e.to_string()))?;
        dat.read(visual).map_err(|e| bad(&e.to_string()))?;
        let mask = crate::apparel::recovered_slot(entity, visual).unwrap_or(info.equipment.map_or(0, |e| e.valid as u16 & 0x7ff));
        let probe = Item { roll: None, id: crate::inventory::ITEM_NAMESPACE + 1, definition: entity, visual, quantity: 1, stack_limit: 1,
            equipment_mask: mask, location: Location::Bag(0) };
        crate::item_services::acquired(7, &probe, info).map_err(|_| bad("its acquisition notice does not encode"))?;
        out.push(ItemProfile { entity, visual, mask, metadata: info });
    }
    Ok(out)
}

/// A quest item's profile read from the DATs (`read_items`).
#[derive(Clone, Copy, Debug)]
pub struct ItemProfile { pub entity: u32, pub visual: u32, pub mask: u16, pub metadata: crate::inventory_request::Metadata }

/// Makes the profiles grantable (`item`). Profiles are only added: an item a live edit no longer
/// grants keeps its profile, since players may hold it.
pub fn register_items(profiles: &[ItemProfile]) {
    let mut items = ITEMS.write().unwrap_or_else(|e| e.into_inner());
    for p in profiles { items.insert(p.entity, (p.visual, p.mask)); }
}

/// `read_items` then `register_items`, at start. Returns the metadata to register.
pub fn install_items(tables: &Tables, dat: &mut crate::dat::Dat)
    -> io::Result<Vec<(u32, crate::inventory_request::Metadata)>> {
    let profiles = read_items(tables, dat)?;
    register_items(&profiles);
    Ok(profiles.iter().map(|p| (p.entity, p.metadata)).collect())
}

