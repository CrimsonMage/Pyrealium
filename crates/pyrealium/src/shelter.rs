//! Rescue the Shelter Hold's item profiles. The quest itself is data (`quest_emotes`, the
//! `content/quest-*.tsv` and `emote-*.tsv` tables); what stays here is what the tables cannot
//! say: the five quest items' DAT profiles (checked against the DATs at start, so a grant makes
//! the right visual) and the development kit's Vault arrival recovery.
pub const TOOL: u32 = 0x47001587;
pub const GLYPH: u32 = 0x47001582;
pub const DIGGER: u32 = 0x4700158b;
pub const SENTINEL: u32 = 0x4700158c;
// Clear arrival beside the scout, outside the authored seven-meter Menhir Shard.
// Verified against the native room, shard collision, and the largest avatar capsule.
pub const VAULT: crate::movement::Placement = crate::movement::Placement {
    cell:0x03f40154, coordinates:[176.,62.,15.32], heading:0 };
pub fn recover_vault_arrival(point:crate::movement::Placement) -> crate::movement::Placement {
    let [x,y,z]=point.coordinates;
    if point.cell==VAULT.cell && (178.3..180.5).contains(&x) && (62.2..63.7).contains(&y) && (14.5..17.).contains(&z) {
        VAULT
    } else {point}
}
struct ItemProfile { definition:u32, visual:u32, physical:u32, appearance:u32, hash:u64 }
const ITEMS: &[ItemProfile] = &[
    ItemProfile{definition:TOOL,visual:0x1f000771,physical:0x47000779,appearance:0x20000021,hash:0xb158fe7e464bd1aa},
    ItemProfile{definition:GLYPH,visual:0x1f0001cf,physical:0x4700026a,appearance:0,hash:0x5a95a5ca45673770},
    ItemProfile{definition:0x47001583,visual:0x1f0018de,physical:0x470034fe,appearance:0x20000268,hash:0xc5d340d20e895fbd},
    ItemProfile{definition:0x47001584,visual:0x1f0018df,physical:0x470034ff,appearance:0x20000268,hash:0x3957ba01e0a605fe},
    ItemProfile{definition:0x47001585,visual:0x1f0018e0,physical:0x47003500,appearance:0x20000268,hash:0xcfe0c57a8c55276b},
];
pub fn validate(dat:&mut crate::dat::Dat)->std::io::Result<std::collections::BTreeMap<u32,crate::inventory_request::Metadata>> {
    let mut result=std::collections::BTreeMap::new();
    for p in ITEMS {
        let b=dat.read(0x81000000|(p.definition&0xffffff))?;
        let hash=b.iter().fold(0xcbf29ce484222325u64,|h,b|(h^u64::from(*b)).wrapping_mul(0x100000001b3));
        // The Legions records are pinned by hash; another release (7230) must at least name the
        // same visual and physical descriptions in the item's class defaults.
        let names=|w:u32|b.windows(4).any(|x|x==w.to_le_bytes());
        if hash!=p.hash && !(names(p.visual) && names(p.physical)) {return Err(std::io::Error::other("Shelter item DAT profile changed"));}
        dat.read(p.definition)?;dat.read(p.visual)?;dat.read(p.physical)?;
        if p.appearance!=0 {dat.read(p.appearance)?;}
        result.insert(p.definition,crate::inventory_request::Metadata{implement:None,icon:p.appearance,physical:p.physical,equipment:None});
    }
    Ok(result)
}
pub fn item(definition:u32,id:u64,slot:u16)->Option<crate::inventory::Item> {
    let p=ITEMS.iter().find(|p|p.definition==definition)?;
    Some(crate::inventory::Item{roll:None,id,definition,visual:p.visual,quantity:1,stack_limit:1,equipment_mask:0,location:crate::inventory::Location::Bag(slot)})
}
pub fn owned(inventory:&crate::inventory::Inventory,definition:u32)->Option<u64> {
    inventory.items().find(|i|matches!(i.location,crate::inventory::Location::Bag(s) if item(definition,i.id,s).as_ref()==Some(i))).map(|i|i.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vault_recovery_only_moves_the_old_obstructed_arrival() {
        let mut p=VAULT;p.coordinates=[178.730484,62.94236,15.121846];
        assert_eq!(recover_vault_arrival(p),VAULT);
        assert_eq!(recover_vault_arrival(VAULT),VAULT);
        p.coordinates=[178.,64.,15.25];assert_eq!(recover_vault_arrival(p),p);
        p.cell=0x02990109;p.coordinates=[178.730484,62.94236,15.121846];assert_eq!(recover_vault_arrival(p),p);
    }
}
