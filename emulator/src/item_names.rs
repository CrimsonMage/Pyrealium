//! The name a created inventory item carries in its `gmWeenieDesc` (`CreateObject`, data-mask
//! bit 4, `Name_StringInfoStat`).
//!
//! The client reads an item's name through `gmEntity` 0xAD (`GetRawStringInfo` over the
//! `CBaseQualities`: the weenie description first, then the class defaults, the DAT qualities at
//! `0x81000000 | definition`). An item the server creates after the client's login reaches it
//! before the client has read those defaults, and a pack slot keeps the "Error!
//! Name_StringInfoStat missing" text it was given then; an item that is in the pack at login does
//! not. A server that puts the name in the description (as retail's `gmWeenieDesc::StreamPack`
//! allows and Pyrealium does for the world's named objects) shows it at once.
//!
//! Where the name comes from (the DAT, not a table of Pyrealium's): the entity description's
//! `Name` property `0x41000143` (a `StringInfo`: the string, then its table), else the name the
//! item's qualities record embeds in its own `gmWeenieDesc` (table, string).
use crate::dat::Dat;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The entity property that holds an item's name (`store_script_7160`'s `NAME_PROPERTY`).
pub const NAME_PROPERTY: u32 = 0x4100_0143;

/// A name's table and string ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    pub table: u32,
    pub string: u32,
}

impl Name {
    /// `StringInfo::Pack` without variables or literal: the string, the table, a zero count and a
    /// zero literal flag.
    pub fn wire(self) -> [u8; 12] {
        let mut bytes = [0; 12];
        bytes[..4].copy_from_slice(&self.string.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.table.to_le_bytes());
        bytes
    }
}

static NAMES: Mutex<BTreeMap<u32, Name>> = Mutex::new(BTreeMap::new());

/// Records the name items of `definition` carry when created.
pub fn register(definition: u32, name: Name) {
    if let Ok(mut names) = NAMES.lock() {
        names.insert(definition, name);
    }
}

/// The recorded name of `definition`.
pub fn lookup(definition: u32) -> Option<Name> {
    NAMES.lock().ok()?.get(&definition).copied()
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// A string-table id (`0x25......`).
fn is_table(id: u32) -> bool {
    id >> 24 == 0x25
}

/// The name in the entity description's `Name` property: `[property, StringInfo type
/// 0x4000000a, string, table, 0]`.
fn entity_name(entity: &[u32]) -> Option<Name> {
    entity.windows(5).find_map(|w| {
        (w[0] == NAME_PROPERTY && w[1] == 0x4000_000a && is_table(w[3]))
            .then_some(Name { table: w[3], string: w[2] })
    })
}

/// The name the qualities record's embedded description holds (`gmWeenieDesc::StreamPack`: the
/// data mask, the DID, the description mask, the optional package / entity / bitfield words, then
/// the name's table, string and a zero).
fn record_name(record: &[u32]) -> Option<Name> {
    let desc = *record.get(2)?;
    if desc & 4 == 0 {
        return None;
    }
    let mut at = 3;
    for bit in [1, 0x200_0000, 2] {
        if desc & bit != 0 {
            at += 1;
        }
    }
    let (a, b, flags) = (*record.get(at)?, *record.get(at + 1)?, *record.get(at + 2)?);
    if flags != 0 {
        return None;
    }
    if is_table(a) {
        Some(Name { table: a, string: b })
    } else if is_table(b) {
        Some(Name { table: b, string: a })
    } else {
        None
    }
}

/// The name of item `definition`, from the DAT.
pub fn name_of(dat: &mut Dat, definition: u32) -> Option<Name> {
    if let Ok(entity) = dat.read(definition) {
        if let Some(name) = entity_name(&words(&entity)) {
            return Some(name);
        }
    }
    let record = dat.read(0x8100_0000 | (definition & 0xff_ffff)).ok()?;
    record_name(&words(&record))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entity_property_names_the_item() {
        let entity = [7, NAME_PROPERTY, 0x4000_000a, 0xa860_7cec, 0x2500_80b9, 0, 9];
        assert_eq!(entity_name(&entity), Some(Name { table: 0x2500_80b9, string: 0xa860_7cec }));
        assert_eq!(entity_name(&[NAME_PROPERTY, 5, 6, 7, 8]), None);
    }

    #[test]
    fn a_record_embeds_the_name_in_either_order() {
        let table_first = [0x21, 0x8100_07ee, 0x10 | 4 | 0x200_0000, 0x4700_07ee, 0x2500_1476, 0xa860_7cec, 0];
        let want = Name { table: 0x2500_1476, string: 0xa860_7cec };
        assert_eq!(record_name(&table_first), Some(want));
        let string_first = [0x21, 0x8100_07ee, 0x10 | 4, 0xa860_7cec, 0x2500_1476, 0];
        assert_eq!(record_name(&string_first), Some(want));
        assert_eq!(record_name(&[0x21, 1, 0x10]), None);
    }

    #[test]
    fn the_wire_is_the_string_the_table_and_two_zero_shorts() {
        let wire = Name { table: 0x2500_1476, string: 0xa860_7cec }.wire();
        assert_eq!(wire, [0xec, 0x7c, 0x60, 0xa8, 0x76, 0x14, 0x00, 0x25, 0, 0, 0, 0]);
    }
}
