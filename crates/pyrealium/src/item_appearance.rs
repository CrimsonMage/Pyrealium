//! Item appearance: the global appearance keys an item's CreateObject VisualDesc carries.
//!
//! Retail servers sent many items with global part group mods (VisualDesc bit 0x100: one or
//! more appearance tables, each with keys and scalars, mostly key 0x41000021), and the client
//! draws them on the model and takes the inventory icon's atoms from them
//! (`VisualDesc::GetTotalIconDesc`, Legions 0x005F3920, reads `this->m_global_app_mods`). The
//! captures (`content/7230/item-appearance-observed.tsv`, `tools/audit_item_appearance.py`) show
//! a scalar that differs between instances of one item type and stays with an instance across
//! sightings, so retail set it per item when the item was made. How it chose the value is not
//! known.
//!
//! Pyrealium policy: an item of an observed type (definition and visual) takes one of the sets
//! retail sent that type, weighted by how many distinct instances showed it (parent-only counts
//! as a set), chosen by a fixed hash of the item's id and definition, so the item keeps it for
//! its life and across restarts without a stored column. A set is used only if each of its
//! appearance tables is one the 7230 DAT visual or a parent of it names in its global
//! appearance: the 2012-2017 servers in the captures ran renumbered DATs, and most of their sets
//! name tables the 7230 visual does not (about 70 item types are confirmed). A pre-2012 set
//! naming another table is left out too, as this server cannot check that table holds the key. An item of an unobserved type
//! stays parent-only, as most retail items were.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use ac2_assets::Ac2VisualDesc;
use ac2_core::{DataId, QualifiedDataId};
use ac2_db::ResourcePackedDecode;

use crate::dat::Dat;

/// Appearance table DID -> its appearance keys and scalars.
pub type GlobalMods = Vec<(u32, Vec<(u32, f32)>)>;

/// One observed set and the instances that showed it.
#[derive(Clone, Debug, PartialEq)]
struct Observed {
    weight: u32,
    mods: GlobalMods,
}

static CATALOG: OnceLock<BTreeMap<(u32, u32), Vec<Observed>>> = OnceLock::new();

fn word(text: &str) -> Option<u32> {
    u32::from_str_radix(text, 16).ok()
}

/// The observed table: `(definition, visual) -> sets`. Malformed lines are an error.
fn parse(text: &str) -> Result<BTreeMap<(u32, u32), Vec<Observed>>, String> {
    let mut table: BTreeMap<(u32, u32), Vec<Observed>> = BTreeMap::new();
    for (number, line) in text.lines().enumerate().filter(|(_, l)| !l.starts_with('#') && !l.is_empty()) {
        let bad = || format!("item-appearance-observed.tsv line {}", number + 1);
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 5 {
            return Err(bad());
        }
        let (Some(entity), Some(visual)) = (word(fields[0]), word(fields[1])) else {
            return Err(bad());
        };
        let count = |f: &str| f.parse::<u32>().map_err(|_| bad());
        let weight = count(fields[2])? + count(fields[3])?;
        let mut mods = GlobalMods::new();
        for part in fields[4].split(';').filter(|p| !p.is_empty()) {
            let (table, keys) = part.split_once(':').ok_or_else(bad)?;
            let mut values = Vec::new();
            for pair in keys.split(',') {
                let (key, value) = pair.split_once('=').ok_or_else(bad)?;
                let value: f32 = value.parse().map_err(|_| bad())?;
                if !value.is_finite() {
                    return Err(bad());
                }
                values.push((word(key).ok_or_else(bad)?, value));
            }
            mods.push((word(table).ok_or_else(bad)?, values));
        }
        if weight == 0 || entity >> 24 != 0x47 || visual >> 24 != 0x1f {
            return Err(bad());
        }
        table.entry((entity, visual)).or_default().push(Observed { weight, mods });
    }
    Ok(table)
}

/// Keeps the sets whose tables the visual (or a parent of it) names in its global appearance;
/// `tables_of` answers `None` for a visual that does not load.
fn validate(
    table: BTreeMap<(u32, u32), Vec<Observed>>,
    mut tables_of: impl FnMut(u32) -> Option<Vec<u32>>,
) -> (BTreeMap<(u32, u32), Vec<Observed>>, usize) {
    let mut dropped = 0;
    let mut kept = BTreeMap::new();
    for ((entity, visual), sets) in table {
        let Some(own) = tables_of(visual) else {
            dropped += sets.len();
            continue;
        };
        let valid: Vec<Observed> = sets
            .into_iter()
            .filter(|set| {
                let ok = set.mods.iter().all(|(t, _)| own.contains(t));
                dropped += usize::from(!ok);
                ok
            })
            .collect();
        if valid.iter().any(|set| !set.mods.is_empty()) {
            kept.insert((entity, visual), valid);
        }
    }
    (kept, dropped)
}

/// Loads the observed table and keeps what the 7230 portal DAT confirms. Called once by
/// `Bootstrap::load` for the 7230 data; without it every item stays parent-only.
pub fn install(portal: &mut Dat) -> std::io::Result<()> {
    let table = parse(include_str!("../../../content/7230/item-appearance-observed.tsv"))
        .map_err(std::io::Error::other)?;
    let (kept, dropped) = validate(table, |visual| {
        // The tables the visual and its parents name in their global appearance.
        let (mut tables, mut current, mut depth) = (Vec::new(), visual, 0);
        while current != 0 && depth < 16 {
            let bytes = portal.read(current).ok()?;
            let desc = Ac2VisualDesc::decode_packed(QualifiedDataId::new(DataId::new(current), 0), 0, &bytes).ok()?;
            tables.extend(desc.global_appearance.iter().map(|(t, _)| t.raw()));
            current = desc.parent.map_or(0, |p| p.raw());
            depth += 1;
        }
        Some(tables)
    });
    eprintln!(
        "item appearance: {} item types with observed sets, {dropped} sets not confirmed by the DATs",
        kept.len()
    );
    let _ = CATALOG.set(kept);
    Ok(())
}

/// SplitMix64: a fixed hash, the same on every build and platform.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn choose(sets: &[Observed], id: u64, definition: u32) -> GlobalMods {
    let total: u64 = sets.iter().map(|s| u64::from(s.weight)).sum();
    if total == 0 {
        return GlobalMods::new();
    }
    let mut pick = mix(id ^ (u64::from(definition) << 32)) % total;
    for set in sets {
        if pick < u64::from(set.weight) {
            return set.mods.clone();
        }
        pick -= u64::from(set.weight);
    }
    GlobalMods::new()
}

/// The global mods item `id` of `definition` and `visual` is sent with (empty: parent-only).
pub fn for_item(id: u64, definition: u32, visual: u32) -> GlobalMods {
    CATALOG
        .get()
        .and_then(|catalog| catalog.get(&(definition, visual)))
        .map_or_else(GlobalMods::new, |sets| choose(sets, id, definition))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_table_parses_and_an_item_keeps_its_set() {
        let table = parse(include_str!("../../../content/7230/item-appearance-observed.tsv")).unwrap();
        assert!(table.len() > 400, "{} types", table.len());
        let sets = table.values().find(|s| s.len() > 3).unwrap();
        let a = choose(sets, 0x1234, 0x47000001);
        assert_eq!(a, choose(sets, 0x1234, 0x47000001), "the same item keeps its set");
        let distinct: std::collections::BTreeSet<String> =
            (0..64u64).map(|id| format!("{:?}", choose(sets, id, 0x47000001))).collect();
        assert!(distinct.len() > 1, "instances of one type differ");
    }

    #[test]
    fn a_set_naming_a_table_the_visual_lacks_is_dropped() {
        let table = parse(
            "47000001\t1F000001\t1\t0\t\n47000001\t1F000001\t0\t2\t20000010:41000021=0.92\n\
             47000001\t1F000001\t0\t1\t20000011:41000021=0.5\n47000002\t1F000002\t0\t1\t20000012:00000002=0.1\n",
        )
        .unwrap();
        let (kept, dropped) = validate(table, |visual| (visual == 0x1f000001).then(|| vec![0x2000_0010]));
        assert_eq!(dropped, 2, "the foreign table and the unloadable visual");
        let sets = &kept[&(0x47000001, 0x1f000001)];
        assert_eq!(sets.len(), 2, "parent-only and the confirmed set");
        assert!(!kept.contains_key(&(0x47000002, 0x1f000002)));
    }

    #[test]
    fn weights_follow_the_instances() {
        let sets = vec![
            Observed { weight: 1, mods: GlobalMods::new() },
            Observed { weight: 3, mods: vec![(0x2000_0010, vec![(0x4100_0021, 0.92)])] },
        ];
        let with = (0..4000u64).filter(|&id| !choose(&sets, id, 0x47000001).is_empty()).count();
        assert!((2800..3200).contains(&with), "{with} of 4000");
    }

    /// On the 7230 DATs: most observed types are confirmed, and an item with mods is described
    /// with them in its pack and in hand, and its hand spawn still parses.
    #[test]
    fn confirmed_sets_reach_pack_and_hand_items() {
        let Ok(dir) = std::env::var("AC2_TEST_GAME_DATA") else { return };
        let mut portal = Dat::open(std::path::Path::new(&dir).join("portal.dat")).unwrap();
        install(&mut portal).unwrap();
        let catalog = CATALOG.get().unwrap();
        assert!(catalog.len() > 60, "{} confirmed types", catalog.len());
        let (&(definition, visual), _) = catalog.iter().next().unwrap();
        let id = (1..10_000u64).find(|&id| !for_item(id, definition, visual).is_empty()).unwrap();
        let mut item = crate::inventory::Item {
            roll: None, id, definition, visual, quantity: 1, stack_limit: 1, equipment_mask: 0,
            location: crate::inventory::Location::Bag(0),
        };
        let pack = crate::world::create_inventory_item(&item, 0x5000_0000_0000_0001, 3).unwrap();
        assert_eq!(&pack[12..16], &0x102u32.to_le_bytes());
        item.location = crate::inventory::Location::Equipped(0x40);
        let hand = crate::world::create_hand_item(&item, 0x5000_0000_0000_0001, 3, 7, 1).unwrap();
        assert_eq!(crate::world::hand_parent(&hand).unwrap(), (0x5000_0000_0000_0001, 7));
    }
}
