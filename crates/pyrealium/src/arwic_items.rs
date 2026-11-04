//! The Arwic Mines items (the hammer, the sword, Iron, the glyph and The Shaper's Guide): their
//! pinned DAT profiles, names and the guide's pages, from content/arwic-mines-v1.json. The lesson
//! itself is quest rows (`arwic_mines_and_crafting`); nothing here runs it.
use crate::inventory::{Item, Location};
use serde::Deserialize;
use std::{collections::BTreeMap, io, sync::OnceLock};

pub const OUTCAST: u32 = 0x470015ac;
pub const HAMMER: u32 = 0x470015a9;
pub const SWORD: u32 = 0x470015aa;
pub const IRON: u32 = 0x470010f3;
pub const GLYPH: u32 = 0x4700109f;
pub const GUIDE: u32 = 0x47002917;
fn bad(s: &str) -> io::Error {
    io::Error::other(s)
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
        serde_json::from_str(include_str!("../../../content/arwic-mines-v1.json"))
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
