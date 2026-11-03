//! Explicit NPC trophy slots from installed DAT properties. These probabilities
//! override observations of those same items; capture frequency is not drop rate.
use std::{collections::BTreeMap,io};
use serde::Deserialize;
use crate::{dat::Dat,inventory::{Item,Location},inventory_request::Metadata};

#[derive(Deserialize)]
struct File {schema:u32,rules:Vec<Rule>}
#[derive(Deserialize)]
struct Rule {entity:u32,sources:BTreeMap<String,String>,drops:Vec<Drop>}
#[derive(Deserialize)]
struct Drop {definition:u32,visual:u32,probability:f32,slot:u8}
#[derive(Default)]
pub(crate) struct Trophies(BTreeMap<u32,Vec<(f32,Item)>>);
impl Trophies {
    pub fn load(dat:&mut Dat,metadata:&mut BTreeMap<u32,Option<Metadata>>)->io::Result<Self> {
        let file:File=serde_json::from_str(include_str!("../../../content/native-trophies.json"))?;
        if file.schema!=1 || file.rules.len()>16384 {return Err(io::Error::other("invalid trophy catalog"));}
        let mut result=Self::default();let mut rejected=0;
        for rule in file.rules {
            if rule.entity>>24!=0x47 || rule.drops.len()>15 || rule.sources.is_empty() || rule.sources.len()>32 {
                return Err(io::Error::other("invalid trophy source"));
            }
            let compatible=rule.sources.iter().all(|(key,expected)| {
                let Ok(did)=u32::from_str_radix(key,16) else{return false};
                let Ok(bytes)=dat.read(did) else{return false};
                let hash=bytes.iter().fold(0xcbf29ce484222325u64,|h,b|(h^u64::from(*b)).wrapping_mul(0x100000001b3));
                format!("{hash:016x}")==*expected
            });
            if !compatible {rejected+=1;continue;}
            let mut slots=std::collections::BTreeSet::new();let mut drops=Vec::new();
            for d in rule.drops {
                if !d.probability.is_finite() || !(0.0..=1.0).contains(&d.probability) || !(1..=15).contains(&d.slot)
                    || !slots.insert(d.slot) {return Err(io::Error::other("invalid trophy probability or slot"));}
                let info=metadata.get(&d.definition).copied().flatten().map(Ok).unwrap_or_else(||Metadata::load(dat,d.definition));
                let Ok(info)=info else{rejected+=1;continue};
                if dat.read(d.definition).is_err() || dat.read(d.visual).is_err(){rejected+=1;continue;}
                let item=Item {roll: None, id:crate::inventory::ITEM_NAMESPACE+1,definition:d.definition,visual:d.visual,
                    quantity:1,stack_limit:1,equipment_mask:info.equipment.map_or(0,|e|e.valid as u16&0x7ff),location:Location::Bag(0)};
                if crate::item_services::acquired(7,&item,info).is_err(){rejected+=1;continue;}
                metadata.insert(d.definition,Some(info));drops.push((d.probability,item));
            }
            if !drops.is_empty() && result.0.insert(rule.entity,drops).is_some(){return Err(io::Error::other("duplicate trophy rule"));}
        }
        println!("Native trophy loot: {} compatible creature rules; {rejected} incompatible rules/items withheld.",result.0.len());
        Ok(result)
    }
    pub fn contains(&self,entity:u32,definition:u32)->bool {
        self.0.get(&entity).is_some_and(|drops|drops.iter().any(|(_,item)|item.definition==definition))
    }
    pub fn apply(&self,entity:u32,items:&mut Vec<Item>,random:&mut u64) {
        let Some(drops)=self.0.get(&entity) else{return};
        // A previously captured trophy must not turn a 5% native slot into 100%.
        items.retain(|item|!drops.iter().any(|(_,t)|t.definition==item.definition));
        let rolled=drops.iter().filter(|(chance,_)|*chance>0. && (*chance>=1. || crate::creature_ai::random(random)<*chance))
            .map(|(_,item)|item.clone()).collect::<Vec<_>>();
        items.truncate(32usize.saturating_sub(rolled.len()));items.extend(rolled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_trophies_validate_against_dat_and_item_transport() {
        let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let mut dat=Dat::open(std::path::Path::new(&directory).join("portal.dat")).unwrap();
        let mut metadata=BTreeMap::new();let catalog=Trophies::load(&mut dat,&mut metadata).unwrap();
        let first=catalog.0.get(&0x470012b2).expect("native 5% trophy source missing");
        assert_eq!(first[0].0,0.05);assert_eq!(first[0].1.definition,0x470039b6);
        assert!(metadata.contains_key(&first[0].1.definition));
    }
    #[test]
    fn native_chance_replaces_captured_trophy_and_preserves_other_loot() {
        let trophy=crate::item_services::potion(1,0);let mut other=trophy.clone();other.definition+=1;
        let mut random=12345;let mut data=Trophies::default();let mut hits=0;
        data.0.insert(0x470012b2,vec![(0.05,trophy.clone())]);
        for _ in 0..10000 {
            let mut items=vec![other.clone(),trophy.clone()];data.apply(0x470012b2,&mut items,&mut random);
            assert_eq!(items[0].definition,other.definition);hits+=items.len()-1;
        }
        assert!((400..600).contains(&hits),"{hits}");
        for (chance,count) in [(0.,1),(1.,2)] {
            data.0.insert(1,vec![(chance,trophy.clone())]);
            let mut items=vec![other.clone(),trophy.clone(),trophy.clone()];data.apply(1,&mut items,&mut random);
            assert_eq!(items.len(),count);
        }
    }
}
