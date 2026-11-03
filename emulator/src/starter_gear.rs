//! Frozen racial starter inventories, grounded in creation/login captures and
//! matching DAT definitions. Never infer fresh grants from item names alone.
//! Historical t1/t2 training grants remain immutable; this profile uses t4.
use crate::{
    dat::Dat,
    inventory::{Command, Inventory, Item, Location},
    inventory_request::Metadata,
    world::CharacterCreationRequest,
};
use std::{collections::BTreeMap, io};

struct Profile {
    race: u32,
    definition: u32,
    visual: u32,
    mask: u16,
    worn: u16,
    qualities_hash: u64,
    physical_hash: u64,
}
include!("starter_gear_data.rs");

fn invalid() -> io::Error {
    io::Error::other("unsupported or incompatible starter gear profile")
}
fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}
pub fn is_apparel(definition: u32) -> bool {
    PROFILES
        .iter()
        .any(|profile| profile.definition == definition && profile.worn != 0)
}
/// Authored buyback eligibility. Preserve the immutable gear profile and reject quest items.
pub fn tradable(item:&Item)->bool {
    item.quantity==1 && item.stack_limit==1 && PROFILES.iter().any(|p|p.definition==item.definition && p.visual==item.visual && p.mask==item.equipment_mask)
}
pub fn apparel_slot(item: &Item, race: u32) -> Option<u16> {
    PROFILES.iter().find(|p| p.race == race && p.definition == item.definition
        && p.visual == item.visual && p.worn != 0 && p.mask == item.equipment_mask
        && item.quantity == 1).map(|p| p.worn)
}
pub fn worn_mask(inventory: &Inventory, race: u32) -> u16 {
    inventory.items().filter_map(|item| apparel_slot(item, race)
        .filter(|slot|item.location == Location::Equipped(*slot))).fold(0, |a,b|a|b)
}
pub fn appearance(character: &crate::store::Character) -> Result<Vec<u8>, crate::world::WorldError> {
    let request=character.creation.as_ref().ok_or(crate::world::WorldError::InvalidValue)?;
    crate::appearance::body(request.race,request.sex).ok_or(crate::world::WorldError::InvalidValue)?
        .visual_desc_equipped(&request.physical_values, if character.starter_gear {worn_mask(&character.inventory,request.race)} else {0x1a},
            &crate::apparel::aspects(&character.inventory,request.race,request.sex))
}
pub fn is_weapon(definition: u32) -> bool {
    PROFILES
        .iter()
        .any(|profile| profile.definition == definition && profile.worn == 0)
}
pub fn is_sword(definition: u32) -> bool {
    matches!(definition, 0x470007ee | 0x470007f5 | 0x47000809)
}
/// DAT-validated racial starter weapons: melee, missile and magic respectively.
/// These are equipment categories, not client-selected damage or animation IDs.
pub fn weapon_category(definition: u32) -> Option<usize> {
    match definition {
        0x470007ee | 0x47000809 | 0x470007f5 => Some(0),
        0x470007ed | 0x47000807 | 0x470007f6 => Some(1),
        0x470007e0 | 0x47000808 | 0x470007f7 => Some(2),
        _ => None,
    }
}
pub fn equipped_category(inventory: &Inventory) -> usize {
    inventory.items().find(|item| item.location == Location::Equipped(0x40))
        .and_then(|item| weapon_category(item.definition)).unwrap_or(0)
}
pub fn combat_mode(inventory: &Inventory) -> u32 {
    let Some(item) = inventory.items().find(|item| item.location == Location::Equipped(0x40)) else {
        return crate::combat_visual::MARTIAL_ARTS;
    };
    match weapon_category(item.definition) {
        Some(1) if item.definition == 0x47000807 => crate::combat_visual::BOULDER,
        Some(1) => crate::combat_visual::MISSILE,
        Some(2) => crate::combat_visual::MAGIC,
        _ => crate::combat_visual::ONE_HAND,
    }
}
pub(crate) fn hand_scale(definition: u32, visual: u32) -> Option<f32> {
    if definition == crate::training_kit::DEFINITION && visual == 0x1f000056 { return Some(1.0); }
    PROFILES.iter().find(|profile| profile.definition == definition && profile.visual == visual && profile.worn == 0)
        .map(|profile| if profile.definition == 0x470007ee { 0.8 } else { 1.0 })
}
pub(crate) fn quality_matches(definition: u32, bytes: &[u8]) -> bool {
    PROFILES.iter().any(|profile| profile.definition == definition && fingerprint(bytes) == profile.qualities_hash)
}
pub(crate) fn physical_matches(definition: u32, bytes: &[u8]) -> bool {
    PROFILES.iter().any(|profile| profile.definition == definition && fingerprint(bytes) == profile.physical_hash)
}
pub fn validate(portal: &mut Dat) -> io::Result<BTreeMap<u32, Metadata>> {
    let mut metadata = BTreeMap::new();
    for profile in PROFILES {
        portal.read(profile.definition)?;
        portal.read(profile.visual)?;
        if fingerprint(&portal.read(0x81000000 | (profile.definition & 0xffffff))?)
            != profile.qualities_hash
        {
            return Err(invalid());
        }
        let info = Metadata::load(portal, profile.definition)?;
        if fingerprint(&portal.read(info.physical)?) != profile.physical_hash {
            return Err(invalid());
        }
        metadata.insert(profile.definition, info);
    }
    Ok(metadata)
}

/// Candidate for an immutable once-only grant. Existing items and occupied
/// equipment slots are retained. Already-owned starter definitions are not
/// duplicated, and an occupied clothing slot leaves the new item in the bag.
pub fn candidate(
    request: &CharacterCreationRequest,
    inventory: &Inventory,
    first: u64,
) -> io::Result<Inventory> {
    if !matches!(request.race, 1 | 2 | 4)
        || !matches!(request.sex, 0x1000 | 0x2000)
        || !inventory.dense_bags()
    {
        return Err(invalid());
    }
    let mut next = inventory.clone();
    let mut id = first;
    // Worn pieces need only a temporary bag slot, so process them before the
    // three permanent bag weapons. A full final kit needs no spare fourth slot.
    let apparel = PROFILES
        .iter()
        .filter(|profile| profile.race == request.race && profile.worn != 0);
    let weapons = PROFILES
        .iter()
        .filter(|profile| profile.race == request.race && profile.worn == 0);
    for profile in apparel.chain(weapons) {
        if next
            .items()
            .any(|item| item.definition == profile.definition)
        {
            continue;
        }
        let slot = next
            .free_slot()
            .ok_or_else(|| io::Error::other("Make room for your racial starter gear."))?;
        let item = Item {roll: None,
            id,
            definition: profile.definition,
            visual: profile.visual,
            quantity: 1,
            stack_limit: 1,
            equipment_mask: profile.mask,
            location: Location::Bag(slot),
        };
        next = next
            .apply(&Command::Grant(item))
            .map_err(io::Error::other)?;
        if profile.worn != 0
            && !next
                .items()
                .any(|item| item.location == Location::Equipped(profile.worn))
        {
            next = next
                .apply(&Command::Wear {
                    item: id,
                    slot: profile.worn,
                })
                .map_err(io::Error::other)?;
        }
        id = id.checked_add(1).ok_or_else(invalid)?;
    }
    Ok(next)
}
