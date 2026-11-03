//! Additional DAT armor profiles. Race/sex selectors are native entity properties;
//! deterministic training drops are Pyrealium policy, not recovered retail loot tables.
use crate::{inventory::{Inventory,Item,Location}, inventory_request::Metadata};
struct Profile {definition:u32,visual:u32,physical:u32,appearance:u32,race:u32,slot:u16,male:u32,female:u32,armor:Option<u32>,hash:u64,entity_hash:u64}
include!("apparel_data.rs");
include!("recovered_apparel_data.rs");
fn profiles()->impl Iterator<Item=&'static Profile>{PROFILES.iter().chain(RECOVERED_PROFILES.iter())}
pub fn validate(dat:&mut crate::dat::Dat)->std::io::Result<std::collections::BTreeMap<u32,Metadata>> {
    let mut result=std::collections::BTreeMap::new();
    let hash=|b:&[u8]|b.iter().fold(0xcbf29ce484222325u64,|h,b|(h^u64::from(*b)).wrapping_mul(0x100000001b3));
    for p in profiles() {
        if hash(&dat.read(p.definition)?)!=p.entity_hash || hash(&dat.read(0x81000000|(p.definition&0xffffff))?)!=p.hash {
            return Err(std::io::Error::other("Armor DAT profile changed"));
        }
        for id in [p.male,p.female,p.appearance,p.visual] {dat.read(id)?;}
        let expected:Vec<u8>=[0xbu32,p.physical,0x40000001,p.visual].into_iter().flat_map(u32::to_le_bytes).collect();
        if dat.read(p.physical)?!=expected {return Err(std::io::Error::other("Armor physics profile changed"));}
        result.insert(p.definition,Metadata{implement:None,icon:p.appearance,physical:p.physical,equipment:Some(crate::inventory_request::Equipment{valid:u32::from(p.slot),preferred:u32::from(p.slot),primary:0,secondary:0})});
    }
    Ok(result)
}
fn profile(item:&Item,race:u32)->Option<&'static Profile> {
    profiles().find(|p|p.race==race && p.definition==item.definition && p.visual==item.visual
        && p.slot==item.equipment_mask && item.quantity==1 && item.stack_limit==1)
}
pub fn required_race(definition:u32)->Option<u32>{profiles().find(|p|p.definition==definition).map(|p|p.race)}
pub fn recovered_slot(definition:u32,visual:u32)->Option<u16>{profiles().find(|p|p.definition==definition && p.visual==visual).map(|p|p.slot)}
pub fn slot(item:&Item,race:u32)->Option<u16> {profile(item,race).map(|p|p.slot)}
pub fn tradable(item:&Item)->bool {
    (PROFILES.iter().any(|p|p.definition==item.definition)||RECOVERED_TRADABLE.contains(&item.definition))
        && [1,2,4,8,16].iter().any(|race|profile(item,*race).is_some())
}
pub fn aspects(inventory:&Inventory,race:u32,sex:u32)->Vec<(u32,u32,f32)> {
    inventory.items().filter_map(|i|profile(i,race).filter(|p|i.location==Location::Equipped(p.slot)).map(|p|(i,p)))
        .flat_map(|(i,p)|[(if sex==0x1000 {p.male}else{p.female},0x10,1.0),(if sex==0x1000 {p.male}else{p.female},2,i.roll.map_or(0.,|r|f32::from(r.tint)/1000.))]).collect()
}
pub fn armor(inventory:&Inventory,race:u32)->u32 {
    inventory.items().filter_map(|i|profile(i,race).filter(|p|i.location==Location::Equipped(p.slot)).map(|p|i.roll.map_or(p.armor.unwrap_or(0),|r|u32::from(r.armor)))).sum()
}
pub fn item_armor(definition:u32)->Option<u32>{profiles().find(|p|p.definition==definition).and_then(|p|p.armor)}
pub fn drop(inventory:&Inventory,race:u32,id:u64,slot:u16)->Option<Item> {
    let p=PROFILES.iter().find(|p|p.race==race && !inventory.items().any(|i|i.definition==p.definition))?;
    Some(Item{roll: None, id,definition:p.definition,visual:p.visual,quantity:1,stack_limit:1,equipment_mask:p.slot,location:Location::Bag(slot)})
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn dye_eligibility_does_not_make_quest_armor_saleable() {
        let p=RECOVERED_PROFILES.iter().find(|p|!RECOVERED_TRADABLE.contains(&p.definition)&&!PROFILES.iter().any(|o|o.definition==p.definition)).unwrap();
        let item=Item{roll:None,id:crate::inventory::ITEM_NAMESPACE+1,definition:p.definition,visual:p.visual,quantity:1,stack_limit:1,equipment_mask:p.slot,location:Location::Bag(0)};
        assert!(slot(&item,p.race).is_some());
        assert!(!tradable(&item));
        assert!(crate::merchants::sale_value(&item).is_none());
    }
    #[test] fn recovered_profiles_match_installed_dat() {
        let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let mut dat=crate::dat::Dat::open(std::path::Path::new(&path).join("portal.dat")).unwrap();
        let profiles=validate(&mut dat).unwrap();
        assert_eq!(profiles[&0x47000fdc].equipment.unwrap().valid,16);
        assert_eq!(required_race(0x47001009),Some(4));
        assert_eq!(item_armor(0x47000fdc),None,"Appearance alone cannot recover rolled armor");
    }
    #[test] fn historical_loot_metadata_repair_is_durable_and_bounded() {
        use crate::inventory::{Command,ITEM_NAMESPACE};
        let item=Item{roll: None, id:ITEM_NAMESPACE+1,definition:0x47000fdc,visual:0x1f00058c,quantity:1,stack_limit:1,equipment_mask:0,location:Location::Bag(0)};
        let old=Inventory::default().apply(&Command::Grant(item.clone())).unwrap();
        let repair=Command::RepairEquipment{item:item.id,definition:item.definition,mask:16};
        let restored=old.apply(&Command::decode(&repair.encode()).unwrap()).unwrap();
        assert_eq!(old.items().next().unwrap().equipment_mask,0);
        assert_eq!(restored.items().next().unwrap().equipment_mask,16);
        assert_eq!(slot(restored.items().next().unwrap(),1),Some(16));
        assert!(restored.apply(&repair).is_err(),"Do not overwrite an existing mask");
        assert!(old.apply(&Command::RepairEquipment{item:item.id,definition:0x47001009,mask:16}).is_err());
        assert!(old.apply(&Command::RepairEquipment{item:item.id,definition:item.definition,mask:64}).is_err());
        let worn=restored.apply(&Command::Wear{item:item.id,slot:16}).unwrap();
        assert_eq!(worn.items().next().unwrap().location,Location::Equipped(16));
        assert_eq!(worn.apply(&Command::Unequip{item:item.id}).unwrap().items().next().unwrap().location,Location::Bag(0));
    }
    #[test] fn profiles_use_sex_specific_aspects_and_enforce_race() {
        for p in profiles() {
            let i=Item{roll: None, id:crate::inventory::ITEM_NAMESPACE+1,definition:p.definition,visual:p.visual,quantity:1,stack_limit:1,equipment_mask:p.slot,location:Location::Bag(0)};
            assert!(slot(&i,if p.race==1 {2}else{1}).is_none());
            let bag=Inventory::default().apply(&crate::inventory::Command::Grant(i.clone())).unwrap()
                .apply(&crate::inventory::Command::Wear{item:i.id,slot:i.equipment_mask}).unwrap();
            for (sex,table) in [(0x1000,p.male),(0x2000,p.female)] {
                assert_eq!(aspects(&bag,p.race,sex),vec![(table,0x10,1.0),(table,2,0.0)]);
            }
            assert_eq!(armor(&bag,p.race),p.armor.unwrap_or(0));
            // Every accepted apparel profile must also be serializable through
            // the equipment reply, including recovered gloves and headwear.
            let metadata=crate::inventory_request::Equipment{valid:u32::from(p.slot),preferred:u32::from(p.slot),primary:0,secondary:0};
            for equipped in [true,false] {
                assert!(crate::ui::hand_equipment_at(7,i.id,p.slot,0x40,metadata,equipped,3).is_ok(),
                    "Equipment reply rejected {:08x}, slot {:x}, equipped {equipped}",p.definition,p.slot);
            }
        }
    }
}
