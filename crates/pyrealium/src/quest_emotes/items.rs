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
    let over = super::tables().and_then(|t| t.items.get(&definition).copied()).unwrap_or_default();
    let profile = ITEMS.read().unwrap_or_else(|e| e.into_inner()).get(&definition).copied();
    let (visual, equipment_mask) = match (over.visual, profile) {
        (Some(visual), _) => (visual, over.equipment_mask.unwrap_or(profile.map_or(0, |p| p.1))),
        (None, Some(profile)) => (profile.0, over.equipment_mask.unwrap_or(profile.1)),
        #[cfg(test)]
        (None, None) if definition >> 24 == 0x47 => (TEST_VISUAL, over.equipment_mask.unwrap_or(0)),
        (None, None) => return None,
    };
    Some(Item { roll: None, id, definition, visual, quantity: 1, stack_limit: over.stack_limit.unwrap_or(1), equipment_mask,
        location: Location::Bag(slot) })
}

/// A quest item's profile where the DAT's defaults do not fit (`quest_item`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Override { pub visual: Option<u32>, pub stack_limit: Option<u32>, pub equipment_mask: Option<u16> }

/// The `quest_item` rows (entity, visual, stack_limit, equipment_mask, source).
pub(super) fn overrides(rows: &[Row]) -> io::Result<BTreeMap<u32, Override>> {
    let hex = |v: &str| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok();
    let mut out = BTreeMap::new();
    for (at, f) in rows {
        let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
        let entity = hex(&f[0]).filter(|e| e >> 24 == 0x47).ok_or_else(|| bad("entity must be an EntityDesc DID"))?;
        let visual = if f[1].is_empty() { None } else { Some(hex(&f[1]).ok_or_else(|| bad("visual must be hex"))?) };
        let stack_limit = if f[2].is_empty() { None } else { Some(f[2].parse().map_err(|_| bad("stack_limit must be 1 to 65535"))?) };
        let equipment_mask = if f[3].is_empty() { None } else { Some(f[3].parse().map_err(|_| bad("equipment_mask must be 0 to 2047"))?) };
        out.insert(entity, Override { visual, stack_limit, equipment_mask });
    }
    Ok(out)
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
        let over = tables.items.get(&entity).copied().unwrap_or_default();
        let bad = |why: &str| io::Error::other(format!("emote set {set} names item {entity:08x}: {why}"));
        // As the tradeskills' items (`tradeskills::item`): the class defaults' bag metadata, and
        // the visual its physical record names.
        dat.read(entity).map_err(|e| bad(&e.to_string()))?;
        let info = crate::inventory_request::Metadata::load(dat, entity).map_err(|e| bad(&e.to_string()))?;
        let visual = match over.visual { Some(v) => v, None => crate::tradeskills::visual_of(dat, info.physical).map_err(|e| bad(&e.to_string()))? };
        dat.read(visual).map_err(|e| bad(&e.to_string()))?;
        let mask = over.equipment_mask.unwrap_or_else(|| crate::apparel::recovered_slot(entity, visual).unwrap_or(info.equipment.map_or(0, |e| e.valid as u16 & 0x7ff)));
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

