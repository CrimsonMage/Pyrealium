//! Native container transport for bounded, authored encounter rewards.
//! Unclaimed rewards expire with their encounter, and are not owned inventory.
//! The grant is journaled before the pending reward is removed.
use crate::{inventory::Item,script,world::WorldError};
use std::{collections::BTreeMap,io};

pub const OPEN:u32=0x820e000c;
pub const CLOSE:u32=0x820e0047;
pub const TAKE:u32=0x820e0036;
pub const ALL:u32=0x820e0068;
pub const TRANSMUTE:u32=0x820e006c;
pub fn handles(f:u32,b:&[u8])->bool{
    [OPEN,CLOSE,ALL,TRANSMUTE].contains(&f) || (f==TAKE && b.get(96..100).is_some_and(|v|u32::from_le_bytes(v.try_into().unwrap())>>16==0x8140))
}
#[derive(Debug,Clone,PartialEq,Eq)]
pub enum Request {Open(u64),Close(u64),Take{container:u64,player:u64,item:Option<u64>},Transmute{container:u64,player:u64}}
fn words(b:&[u8])->Vec<u32>{b.chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect()}
fn iid(w:&[u32],i:usize)->u64{u64::from(w[i])|(u64::from(w[i+1])<<32)}
fn write(out:&mut Vec<u8>,w:impl IntoIterator<Item=u32>){for x in w{out.extend(x.to_le_bytes());}}
pub fn decode(function:u32,b:&[u8])->Result<Request,WorldError>{
    let bad=WorldError::InvalidValue;
    let w=words(b);
    if function==TRANSMUTE {
        if b.len()!=104 || w[..7]!=[5,1,1,0x44000100,0,0x234b0000,15]{return Err(bad)}
        let f=&w[7..22];let container=iid(f,8);let player=iid(f,13);
        let mut expected=[0u32;15];expected[2]=u32::MAX;expected[3]=1;expected[10]=u32::MAX;
        for i in [8,9,13,14]{expected[i]=f[i];}
        if f!=expected || !crate::inventory_request::tags_agree(&b[88..103],&[2,2,1,2,2,2,2,2,0x82,0xcd,1,2,2,0x82,0xcd]) || b[103]!=0
            || [0,u64::MAX,player].contains(&container) || [0,u64::MAX].contains(&player){return Err(bad)}
        return Ok(Request::Transmute{container,player});
    }
    if function==OPEN || function==CLOSE {
        if b.len()!=12 || w[0]!=2 || [0,u64::MAX].contains(&iid(&w,1)){return Err(bad)}
        return Ok(if function==OPEN{Request::Open(iid(&w,1))}else{Request::Close(iid(&w,1))});
    }
    if function==ALL {
        if b.len()!=100 || w[..7]!=[5,1,1,0x44000100,0,0x20ef0000,14]{return Err(bad)}
        let f=&w[7..21];let container=iid(f,6);let player=iid(f,12);
        let mut expected=[0u32;14];expected[2]=1;expected[8]=u32::MAX;expected[11]=u32::MAX;
        for i in [6,7,12,13]{expected[i]=f[i];}
        if f!=expected || !crate::inventory_request::tags_agree(&b[84..98],&[2,2,2,2,2,2,0x82,0xcd,1,2,2,1,0x82,0xcd]) || b[98..]!=[0,0]
            || [0,u64::MAX,player].contains(&container) || [0,u64::MAX].contains(&player){return Err(bad)}
        return Ok(Request::Take{container,player,item:None});
    }
    if function==TAKE {
        if b.len()!=284 || w[..7]!=[5,1,1,0x44000100,0,0x1f80000,51]{return Err(bad)}
        let f=&w[7..58];let player=iid(f,2);let container=iid(f,16);let item=iid(f,28);
        let mut expected=[0u32;51];
        for i in [0,10,13,23,25,26,35,47,49]{expected[i]=1;}
        expected[30]=u32::MAX;
        // The slot the item was dropped on (`ContainerDisplay::FinishListBoxDrop`: the slot under
        // the pointer, copied into the descriptor's three target-slot fields by the player's
        // script) or -1. The server places the item itself.
        for i in [11,24,50]{expected[i]=f[11];}
        // The item's slot in the container (the script copies it into two fields).
        expected[1]=f[1];expected[39]=f[1];
        if f[1]>u32::from(crate::inventory::BAG_SLOTS){return Err(bad)}
        if f[11]!=u32::MAX && f[11]>u32::from(crate::inventory::BAG_SLOTS){return Err(bad)}
        // What the script classifies the move as (`InventorySystem` 0xD, from the weenie types of the two containers: a move
        // into the player is 12, 20 or 36) and the target's weenie type; the client's own
        // classification, never authority.
        if f[12]>64{return Err(bad)}
        expected[12]=f[12];expected[15]=f[15];
        for i in [2,8,32]{expected[i]=player as u32;expected[i+1]=(player>>32) as u32;}
        for i in [16,36]{expected[i]=container as u32;expected[i+1]=(container>>32) as u32;}
        for i in [28,44]{expected[i]=item as u32;expected[i+1]=(item>>32) as u32;}
        const TAGS:[u8;51]=[2,2,0x82,0xcd,2,2,2,2,0x82,0xcd,2,2,2,2,2,2,0x82,0xcd,0x82,0xcd,2,2,2,2,2,2,2,2,0x82,0xcd,1,2,0x82,0xcd,2,2,0x82,0xcd,2,2,2,2,2,2,0x82,0xcd,2,2,2,2,2];
        if f!=expected || !crate::inventory_request::tags_agree(&b[232..283],&TAGS) || b[283]!=0 || [0,u64::MAX,player].contains(&container)
            || [0,u64::MAX].contains(&player) || [0,u64::MAX,player,container].contains(&item){return Err(bad)}
        return Ok(Request::Take{container,player,item:Some(item)});
    }
    Err(bad)
}
fn list(ids:&[u64])->Vec<u8>{
    let mut b=Vec::new();write(&mut b,[5,1,1,0x44000100,0,0xffff003b,ids.len() as u32]);
    for id in ids{b.extend(id.to_le_bytes());}write(&mut b,[0]);b
}
pub fn opened(container:u64,item:Option<u64>)->Result<Vec<u8>,WorldError>{
    opened_items(container,&item.into_iter().collect::<Vec<_>>())
}
pub fn opened_items(container:u64,items:&[u64])->Result<Vec<u8>,WorldError>{
    if items.len()>32 {return Err(WorldError::LimitExceeded)}
    let mut b=script::integer(0);b.extend(list(&[]));b.extend(list(items));b.extend(script::identity(container));
    script::private_call(0x820e005d,&b)
}
pub fn closed(container:u64)->Result<Vec<u8>,WorldError>{
    let mut b=script::integer(0);b.extend(script::identity(container));script::private_call(0x820e0060,&b)
}
pub struct Reward {pub owner:u64,pub items:Vec<Item>,pub expires:u64}
#[derive(Default)]
pub struct Catalog {supported:BTreeMap<u32,Item>,custom:BTreeMap<crate::world_rules::Target,Vec<(u32,Vec<Item>)>>,tables:BTreeMap<u32,Vec<(u32,Vec<Item>)>>,profiles:BTreeMap<u32,Vec<(u32,Vec<Item>)>>,trophies:crate::native_trophies::Trophies,limits:crate::loot_limits::Limits,weapons:std::collections::BTreeSet<u32>,treasure:Vec<Item>,levels:BTreeMap<u32,u16>}
impl Catalog {
    pub fn load(dat:&mut crate::dat::Dat,metadata:&mut BTreeMap<u32,Option<crate::inventory_request::Metadata>>)->io::Result<Self> {
        Self::load_with_rules(dat,metadata,&crate::world_rules::Rules::default())
    }
    pub fn load_with_rules(dat:&mut crate::dat::Dat,metadata:&mut BTreeMap<u32,Option<crate::inventory_request::Metadata>>,rules:&crate::world_rules::Rules)->io::Result<Self> {
        let mut items=BTreeMap::new();let mut rejected=0;
        for line in include_str!("../content/loot-items.tsv").lines().skip(1).filter(|l|!l.is_empty() && !l.starts_with('#')) {
            let f=line.split('\t').collect::<Vec<_>>();if f.len()!=3{return Err(io::Error::other("loot item row"))}
            let hex=|s|u32::from_str_radix(s,16).map_err(io::Error::other);
            let definition=hex(f[0])?;let visual=hex(f[1])?;
            let info=metadata.get(&definition).copied().flatten().map(Ok).unwrap_or_else(||crate::inventory_request::Metadata::load(dat,definition));
            let Ok(info)=info else {rejected+=1;continue};
            if dat.read(definition).is_err() || dat.read(visual).is_err(){rejected+=1;continue}
            let item=Item{roll:None,id:crate::inventory::ITEM_NAMESPACE+1,definition,visual,quantity:1,stack_limit:1,
                equipment_mask:crate::apparel::recovered_slot(definition,visual).unwrap_or(info.equipment.map_or(0,|e|e.valid as u16 & 0x7ff)),location:crate::inventory::Location::Bag(0)};
            if crate::item_services::acquired(7,&item,info).is_err(){rejected+=1;continue}
            metadata.insert(definition,Some(info));items.insert(definition,item);
        }
        let mut catalog=Self::default();let mut missing=0;
        for line in include_str!("../content/observed-loot.tsv").lines().skip(1).filter(|l|!l.is_empty() && !l.starts_with('#')) {
            let f=line.split('\t').collect::<Vec<_>>();if f.len()!=4{return Err(io::Error::other("loot bundle row"))}
            let entity=u32::from_str_radix(f[0],16).map_err(io::Error::other)?;
            let profile=f[1].parse::<u32>().map_err(io::Error::other)?;let count=f[2].parse::<u32>().map_err(io::Error::other)?;
            let bundle=f[3].split(',').map(|id|u32::from_str_radix(id,16).ok().and_then(|id|items.get(&id).cloned())).collect::<Option<Vec<_>>>();
            let Some(bundle)=bundle else {missing+=1;continue};
            if !(1..=32).contains(&bundle.len()) || count==0 || count>100000{return Err(io::Error::other("loot bundle bounds"))}
            catalog.tables.entry(entity).or_default().push((count,bundle.clone()));
            if profile!=0 {catalog.profiles.entry(profile).or_default().push((count,bundle));}
        }
        println!("Observed loot: {} creature tables, {} native profiles, {} compatible items; {rejected} unsupported items and {missing} incomplete bundles withheld. Random affixes and retail weights remain unrecovered.",catalog.tables.len(),catalog.profiles.len(),items.len());
        // Only supported usable profiles enter the generic inferred treasure pool.
        // Captured quest/special items remain tied to their observed creature.
        catalog.weapons=crate::treasure_weapons::compatible(dat,metadata);
        catalog.treasure=items.values().filter(|i|catalog.weapons.contains(&i.definition) || crate::apparel::recovered_slot(i.definition,i.visual).is_some()
            ||matches!(i.definition,crate::item_services::POTION_DEFINITION|crate::crafting::VIGOR_OUTPUT|crate::crafting::MUCOR)).cloned().collect();
        for line in include_str!("../content/creature-combat.tsv").lines()
            .chain(include_str!("../content/inferred-creature-combat-v1.tsv").lines())
            .chain(include_str!("../content/regional-creature-combat-v1.tsv").lines())
            .chain(include_str!("../content/wilderness-creature-combat-v1.tsv").lines())
            .chain(include_str!("../content/arwic-mines-creature-combat-v1.tsv").lines()) {
            let f=line.split('\t').collect::<Vec<_>>();if f.len()<7||!f[0].starts_with("47"){continue;}
            catalog.levels.insert(u32::from_str_radix(f[0],16).map_err(io::Error::other)?,f[6].parse::<u16>().map_err(io::Error::other)?.clamp(1,150));
        }
        catalog.trophies=crate::native_trophies::Trophies::load(dat,metadata)?;
        catalog.limits=crate::loot_limits::Limits::load(dat)?;
        catalog.supported=items;
        catalog.set_world_rules(rules)?;
        if !catalog.custom.is_empty() {println!("World rules: {} custom loot tables.",catalog.custom.len());}
        Ok(catalog)
    }
    /// Whether the DAT-backed catalog was loaded (corpse loot enabled).
    pub fn loaded(&self)->bool {!self.supported.is_empty()}
    /// Operator tables may use any validated loot item; others are refused and the
    /// current tables stay. Corpses already rolled keep their loot.
    pub fn set_world_rules(&mut self,rules:&crate::world_rules::Rules)->io::Result<()> {
        let mut custom=BTreeMap::new();
        for (&target,outcomes) in &rules.loot {
            let mut table=Vec::new();
            for outcome in outcomes {
                let bundle=outcome.items.iter().map(|id|self.supported.get(id).cloned().ok_or_else(||io::Error::other(
                    format!("world rule loot item {id:08x} is not a supported loot item (see content/loot-items.tsv)")))).collect::<io::Result<Vec<_>>>()?;
                table.push((outcome.weight,bundle));
            }
            custom.insert(target,table);
        }
        self.custom=custom;
        Ok(())
    }
    fn roll(&self,container:u64,entity:u32,ai:crate::creature_ai::Profile,random:&mut u64)->Vec<Item> {
        if ai.loot_proof{return vec![]}
        use crate::world_rules::Target;
        if let Some(table)=self.custom.get(&Target::Actor(container)).or_else(||self.custom.get(&Target::Entity(entity))) {
            // An authored table replaces every native/inferred source, trophies included.
            let total=table.iter().map(|(w,_)|u64::from(*w)).sum::<u64>();
            let mut roll=(crate::creature_ai::random(random) as f64*total as f64) as u64;
            for (weight,items) in table {if roll<u64::from(*weight){return items.clone()}roll-=u64::from(*weight);}
            return vec![];
        }
        let level=self.levels.get(&entity).copied().or_else(||self.limits.0.get(&entity).map(|q|q.level)).unwrap_or(1);
        let mut items=if let Some(quantity)=self.limits.0.get(&entity) {
            // Native bounds apply to generic treasure, separately from trophy and
            // quest slots. The original quantity distribution remains unavailable.
            let pool=self.treasure.iter().filter(|i|!self.trophies.contains(entity,i.definition) && self.eligible(i,level)).collect::<Vec<_>>();
            if pool.is_empty() {Vec::new()} else {(0..quantity.roll(random)).map(|_| {
                pool[(crate::creature_ai::random(random)*pool.len() as f32) as usize].clone()
            }).collect()}
        } else if self.levels.contains_key(&entity) {self.inferred_roll(entity,ai,random)}else{self.observed_roll(entity,ai,random)};
        self.roll_stats(&mut items,level,random);
        self.trophies.apply(entity,&mut items,random);items
    }
    fn inferred_roll(&self,entity:u32,ai:crate::creature_ai::Profile,random:&mut u64)->Vec<Item> {
        use crate::creature_ai::random as draw;
        let mut output=Vec::new();
        let level=self.levels.get(&entity).copied().unwrap_or(1);
        let pool=self.treasure.iter().filter(|i|self.eligible(i,level)).collect::<Vec<_>>();
        // A sampled bundle is evidence for eligibility, not a fixed retail drop.
        // Inferred policy: one observed slot at 35%, plus 0..2 usable rolls.
        let observed=self.observed_roll(entity,ai,random);
        if !observed.is_empty()&&draw(random)<0.35 {
            output.push(observed[(draw(random)*observed.len() as f32) as usize].clone());
        }
        for probability in [0.70,0.25] {
            if pool.is_empty()||draw(random)>=probability{continue;}
            let item=pool[(draw(random)*pool.len() as f32) as usize].clone();
            if !output.iter().any(|i|i.definition==item.definition){output.push(item);}
        }
        output
    }
    fn eligible(&self,item:&Item,level:u16)->bool {
        !self.weapons.contains(&item.definition) || crate::treasure_weapons::profile(item).is_some_and(|p|p.minimum_level<=level)
    }
    fn roll_stats(&self,items:&mut [Item],level:u16,random:&mut u64) {
        use crate::creature_ai::random as draw;
        for item in items {
            if self.weapons.contains(&item.definition) {
                crate::treasure_weapons::roll(item,level,random);
            } else if crate::apparel::recovered_slot(item.definition,item.visual).is_some() {
                let base=crate::apparel::item_armor(item.definition).unwrap_or(u32::from(level)*2+2);
                let armor=((base as f32)*(0.75+draw(random)*0.5)).round().clamp(1.,1000.) as u16;
                item.roll=Some(crate::inventory::Roll{armor,tint:(draw(random)*1001.) as u16,level,..Default::default()});
            }
        }
    }
    fn observed_roll(&self,entity:u32,ai:crate::creature_ai::Profile,random:&mut u64)->Vec<Item> {
        let Some(table)=self.tables.get(&entity).or_else(||self.profiles.get(&ai.loot_profile)) else{return vec![]};
        let total=table.iter().map(|(w,_)|u64::from(*w)).sum::<u64>();
        let mut roll=(crate::creature_ai::random(random)*total as f32) as u64;
        for (weight,items) in table {if roll<u64::from(*weight){return items.clone()}roll-=u64::from(*weight);}
        vec![]
    }
}
#[derive(Default)]
pub struct Loot {rewards:BTreeMap<u64,Reward>,sequence:u64,pub catalog:Catalog,random:u64}
impl Loot {
    /// Reproducible isolated integration runs; never accepted from a client.
    pub fn seed_for_test(&mut self,seed:u64)->io::Result<()> {
        if seed==0||!self.rewards.is_empty(){return Err(io::Error::other("invalid test loot seed"));}
        self.random=seed;Ok(())
    }
    pub fn add(&mut self,container:u64,owner:u64,now:u64,entity:u32,ai:crate::creature_ai::Profile)->usize {
        self.add_with_quest(container,owner,now,entity,ai,None)
    }
    pub fn add_with_quest(&mut self,container:u64,owner:u64,now:u64,entity:u32,ai:crate::creature_ai::Profile,quest:Option<Item>)->usize {
        if let Some(reward)=self.rewards.get(&container).filter(|r|r.expires>now){return reward.items.len()}
        if self.random==0 {self.random=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64 ^ container;}
        let mut items=self.catalog.roll(container,entity,ai,&mut self.random);
        if let Some(item)=quest.filter(|_|!ai.loot_proof) {items.retain(|i|i.definition!=item.definition);items.truncate(31);items.push(item);}
        self.insert(container,owner,now,items)
    }
    /// Test-only (`PYREALIUM_TEST_CORPSE_LOOT=1`): `count` authored potions on a body whose roll was
    /// empty, so the corpse window can be tested on creatures with no loot table.
    pub fn add_test_potions(&mut self,container:u64,owner:u64,now:u64,count:u16)->usize {
        let items=(0..count).map(|i|crate::item_services::potion(0,i)).collect();
        self.rewards.remove(&container);
        self.insert(container,owner,now,items)
    }
    fn insert(&mut self,container:u64,owner:u64,now:u64,mut items:Vec<Item>)->usize {
        self.rewards.retain(|_,r|r.expires>now);
        if let Some(reward)=self.rewards.get(&container){return reward.items.len()}
        if self.rewards.len()>=4096 || items.len()>32{return 0}
        for item in &mut items {self.sequence+=1;item.id=0x8191000000000000|self.sequence;}
        let count=items.len();self.rewards.insert(container,Reward{owner,items,expires:now.saturating_add(60000)});count
    }
    pub fn get(&self,container:u64,owner:u64,now:u64)->Option<&Reward>{self.rewards.get(&container).filter(|r|r.owner==owner && r.expires>now)}
    pub fn inspect_item(&self,item:u64,owner:u64,now:u64,visible:impl Fn(u64)->bool)->Option<&Item>{
        self.rewards.iter().filter(|(container,r)|r.owner==owner&&r.expires>now&&visible(**container))
            .find_map(|(_,r)|r.items.iter().find(|i|i.id==item))
    }
    pub fn contains(&self,container:u64,now:u64)->bool{self.rewards.get(&container).is_some_and(|r|r.expires>now)}
    pub fn remove(&mut self,container:u64){self.rewards.remove(&container);}
    pub fn claimed(&mut self,container:u64,item:u64) {
        if let Some(reward)=self.rewards.get_mut(&container){reward.items.retain(|i|i.id!=item);}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_quantity_overrides_fallback_but_not_lootproof_or_operator_tables() {
        let mut catalog=Catalog::default();let mut random=37;
        let ai=crate::creature_ai::Profile::default();
        catalog.treasure=vec![crate::item_services::potion(1,0)];
        catalog.tables.insert(17,vec![(1,vec![crate::item_services::potion(1,0)])]);
        catalog.limits.0.insert(17,crate::loot_limits::Quantity{minimum:3,maximum:6,level:1});
        let counts=(0..500).map(|_|catalog.roll(21,17,ai,&mut random).len()).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(counts,(3..=6).collect());
        catalog.limits.0.insert(17,crate::loot_limits::Quantity{minimum:0,maximum:0,level:1});
        assert!(catalog.roll(21,17,ai,&mut random).is_empty(),"zero overrides a captured bundle");
        catalog.custom.insert(crate::world_rules::Target::Actor(21),vec![(1,vec![crate::item_services::potion(1,0)])]);
        assert_eq!(catalog.roll(21,17,ai,&mut random).len(),1,"operator table wins");
        let mut protected=ai;protected.loot_proof=true;
        assert!(catalog.roll(21,17,protected,&mut random).is_empty());
        catalog.custom.clear();catalog.treasure.clear();
        catalog.limits.0.insert(17,crate::loot_limits::Quantity{minimum:3,maximum:6,level:1});
        assert!(catalog.roll(21,17,ai,&mut random).is_empty(),"no fabricated fallback item");
    }
    #[test]
    fn installed_native_boss_quantity_is_used_and_rolled_stats_survive_claim_transport() {
        let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let mut dat=crate::dat::Dat::open(std::path::Path::new(&directory).join("portal.dat")).unwrap();
        let catalog=Catalog::load(&mut dat,&mut BTreeMap::new()).unwrap();
        assert_eq!(catalog.limits.0.len(),312);
        assert_eq!(catalog.weapons.len(),69);
        let boss=0x4700390b; // Baron Colier: native 3..6 generic treasure slots.
        let quantity=catalog.limits.0.get(&boss).unwrap();assert_eq!((quantity.minimum,quantity.maximum),(3,6));
        let mut loot=Loot{catalog,random:42,..Default::default()};let mut seen=std::collections::BTreeSet::new();
        for corpse in 100..200 {
            loot.add(corpse,7,0,boss,crate::creature_ai::Profile::default());
            let reward=loot.get(corpse,7,0).unwrap();
            let normal=reward.items.iter().filter(|i|!loot.catalog.trophies.contains(boss,i.definition)).count();
            assert!((3..=6).contains(&normal));
            let original=reward.items.clone();let random=loot.random;
            assert_eq!(loot.add(corpse,8,1,boss,crate::creature_ai::Profile::default()),original.len());
            assert_eq!(loot.random,random);assert!(loot.get(corpse,8,1).is_none());
            for item in original {
                if let Some(roll)=item.roll {assert_eq!(roll.level,56);seen.insert((item.definition,roll.armor,roll.tint));
                    // Claiming replaces the temporary corpse IID with a journal-owned IID.
                    let mut owned=item.clone();owned.id=crate::inventory::ITEM_NAMESPACE+1;
                    let command=crate::inventory::Command::Grant(owned.clone());
                    let decoded=crate::inventory::Command::decode(&command.encode()).unwrap();
                    let inventory=crate::inventory::Inventory::default().apply(&decoded).unwrap();
                    assert_eq!(inventory.items().next(),Some(&owned));
                }
                loot.claimed(corpse,item.id);
            }
            assert_eq!(loot.add(corpse,7,2,boss,crate::creature_ai::Profile::default()),0);
        }
        assert!(seen.len()>50,"gear must carry varied durable stats");
    }
    #[test]
    fn quest_bonus_is_owned_once_and_never_replaces_normal_loot() {
        let mut loot=Loot::default();let ai=crate::creature_ai::Profile::default();
        loot.catalog.tables.insert(17,vec![(1,vec![crate::item_services::potion(1,0)])]);
        let hammer=crate::arwic_mines::item(crate::arwic_mines::HAMMER,1,0,1).unwrap();
        assert!(crate::merchants::sale_value(&hammer).is_none());
        assert_eq!(loot.add_with_quest(21,7,100,17,ai,Some(hammer.clone())),2);
        let reward=loot.get(21,7,100).unwrap();
        let iid=reward.items.iter().find(|i|i.definition==hammer.definition).unwrap().id;
        assert!(loot.get(21,8,100).is_none());loot.claimed(21,iid);
        assert_eq!(loot.add_with_quest(21,8,101,17,ai,Some(hammer.clone())),1);
        assert!(!loot.get(21,7,101).unwrap().items.iter().any(|i|i.definition==hammer.definition));
        let mut protected=ai;protected.loot_proof=true;
        loot.add_with_quest(22,7,101,17,protected,Some(hammer));
        assert!(loot.get(22,7,101).unwrap().items.is_empty());
    }
    #[test]
    fn rewards_are_owned_temporary_and_one_time(){
        let mut loot=Loot::default();loot.insert(21,7,100,vec![crate::item_services::potion(1,0)]);
        assert!(loot.get(21,8,200).is_none());assert!(loot.get(21,7,60100).is_none());
        assert!(loot.get(21,7,200).is_some());loot.remove(21);assert!(loot.get(21,7,200).is_none());
    }
    #[test]
    fn malformed_container_requests_cannot_supply_inventory_authority(){
        for f in [OPEN,CLOSE,TAKE,ALL,TRANSMUTE] {for n in 0..300{assert!(decode(f,&vec![0;n]).is_err());}}
        let b=script::identity(21);assert_eq!(decode(OPEN,&b).unwrap(),Request::Open(21));
        assert!(decode(OPEN,&script::identity(0)).is_err());
    }
    /// The request the 7230 player script sends for an item dragged out of a corpse
    /// (`CInventorySystem::MoveItem`, from the server double's corpse, a live body's classification
    /// 36): the item's slot in the body, the slot it was dropped on, the script's classification of
    /// the move and the target's weenie type, the ids typed 0x84.
    #[test]
    fn a_take_carries_the_scripts_slots_and_classification(){
        let mut f=[0u32;51];
        for i in [0,10,13,23,25,26,35,47,49]{f[i]=1;}
        f[1]=1;f[39]=1;f[11]=13;f[24]=13;f[50]=13;f[12]=36;f[15]=0x1f;f[30]=u32::MAX;
        for i in [2,8,32]{f[i]=7;}
        for i in [16,36]{f[i]=21;}
        for i in [28,44]{f[i]=42;}
        let mut b=Vec::new();write(&mut b,[5,1,1,0x44000100,0,0x1f80000,51]);write(&mut b,f);
        b.extend([2,4,0x84,0xcd,2,2,2,2,0x84,0xcd,2,4,2,2,2,4,0x84,0xcd,0x82,0xcd,2,2,2,2,4,4,4,2,0x84,0xcd,1,2,0x84,0xcd,2,2,0x84,0xcd,2,4,2,2,2,2,0x84,0xcd,2,2,2,2,4,0]);
        // The container's id must be a body's (`handles`); the test's is checked through decode.
        assert_eq!(decode(TAKE,&b).unwrap(),Request::Take{container:21,player:7,item:Some(42)});
        let mut moved=b.clone();moved[28+12*4]=65;assert!(decode(TAKE,&moved).is_err());
        let mut slot=b.clone();slot[28+24*4]^=1;assert!(decode(TAKE,&slot).is_err());
        let mut source=b.clone();source[28+39*4]^=1;assert!(decode(TAKE,&source).is_err());
    }
    #[test]
    fn transmute_rejects_client_price_and_identity_tampering(){
        let mut b=Vec::new();write(&mut b,[5,1,1,0x44000100,0,0x234b0000,15]);
        write(&mut b,[0,0,u32::MAX,1,0,0,0,0,21,0,u32::MAX,0,0,7,0]);
        b.extend([2,2,1,2,2,2,2,2,0x82,0xcd,1,2,2,0x82,0xcd,0]);
        assert_eq!(decode(TRANSMUTE,&b).unwrap(),Request::Transmute{container:21,player:7});
        for slot in [0,1,4,5,6,7,10,11,12]{let mut tampered=b.clone();tampered[28+slot*4]^=1;assert!(decode(TRANSMUTE,&tampered).is_err());}
        b[28+8*4]=7;assert!(decode(TRANSMUTE,&b).is_err());
    }
    #[test]
    fn multi_item_claims_preserve_the_original_roll_and_owner() {
        let mut loot=Loot::default();
        let items=vec![crate::item_services::potion(1,0),crate::item_services::potion(2,1)];
        loot.catalog.tables.insert(17,vec![(1,items)]);
        let ai=crate::creature_ai::Profile::default();
        assert_eq!(loot.add(21,7,100,17,ai),2);
        let ids=loot.get(21,7,100).unwrap().items.iter().map(|i|i.id).collect::<Vec<_>>();
        assert_ne!(ids[0],ids[1]);
        loot.claimed(21,ids[0]);loot.claimed(21,ids[0]);
        // Repeated death/open notifications cannot reroll or steal a corpse.
        let random=loot.random;assert_eq!(loot.add(21,8,200,17,ai),1);assert_eq!(random,loot.random);
        assert!(loot.get(21,8,200).is_none());
        assert_eq!(loot.get(21,7,200).unwrap().items[0].id,ids[1]);
        loot.claimed(21,ids[1]);assert_eq!(loot.add(21,7,300,17,ai),0);
        assert!(loot.contains(21,300));assert!(loot.get(21,7,60100).is_none());
    }
    #[test]
    fn missing_profiles_do_not_substitute_potions_and_lootproof_is_respected() {
        let mut catalog=Catalog::default();let mut random=123;
        catalog.profiles.insert(411,vec![(1,vec![crate::item_services::potion(1,0)])]);
        let mut ai=crate::creature_ai::Profile::default();
        assert!(catalog.roll(0,17,ai,&mut random).is_empty());
        ai.loot_profile=411;assert_eq!(catalog.roll(0,17,ai,&mut random).len(),1);
        ai.loot_proof=true;assert!(catalog.roll(0,17,ai,&mut random).is_empty());
    }
    #[test]
    fn installed_trophy_rolls_without_observed_bundle_and_does_not_reroll_corpse() {
        let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let mut dat=crate::dat::Dat::open(std::path::Path::new(&directory).join("portal.dat")).unwrap();
        let mut metadata=BTreeMap::new();let mut catalog=Catalog::load(&mut dat,&mut metadata).unwrap();
        catalog.tables.clear();catalog.profiles.clear();
        let ai=crate::creature_ai::Profile::default();let mut random=12345;let mut hits=0;
        for _ in 0..10000 {
            let items=catalog.roll(0,0x470012b2,ai,&mut random);
            assert!(items.iter().all(|i|i.definition==0x470039b6));hits+=items.len();
        }
        assert!((400..600).contains(&hits),"native 5% slot: {hits}");
        let mut protected=ai;protected.loot_proof=true;
        assert!(catalog.roll(0,0x470012b2,protected,&mut random).is_empty());
        let mut loot=Loot {catalog,random:12345,..Default::default()};
        let count=loot.add(21,7,100,0x470012b2,ai);let state=loot.random;
        assert_eq!(loot.add(21,8,200,0x470012b2,ai),count);
        assert_eq!(loot.random,state);assert!(loot.get(21,8,200).is_none());
    }
    #[test]
    fn inferred_vermin_loot_varies_and_armor_rolls_are_serializable() {
        let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let mut dat=crate::dat::Dat::open(std::path::Path::new(&directory).join("portal.dat")).unwrap();
        let catalog=Catalog::load(&mut dat,&mut BTreeMap::new()).unwrap();
        assert!(!catalog.treasure.is_empty());
        let ai=crate::creature_ai::Profile::default();let mut random=55182;
        let mut shapes=std::collections::BTreeSet::new();let mut stats=std::collections::BTreeSet::new();
        for _ in 0..500 {
            let items=catalog.roll(0,0x47001640,ai,&mut random);
            shapes.insert(items.iter().map(|i|i.definition).collect::<Vec<_>>());
            for item in items {
                if let Some(roll)=item.roll {
                    stats.insert((roll.armor,roll.tint));
                    let command=crate::inventory::Command::Grant(item.clone());
                    let decoded=crate::inventory::Command::decode(&command.encode()).unwrap();
                    let restored=crate::inventory::Inventory::default().apply(&decoded).unwrap();
                    assert_eq!(restored.items().next().unwrap(),&item);
                }
            }
        }
        assert!(shapes.len()>20,"Only {} distinct bundles",shapes.len());
        assert!(stats.len()>100,"Only {} armor variants",stats.len());
        let mut protected=ai;protected.loot_proof=true;
        assert!(catalog.roll(0,0x47001640,protected,&mut random).is_empty());
    }
    #[test]
    fn network_loot_fixture_seed() {
        let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let mut dat=crate::dat::Dat::open(std::path::Path::new(&directory).join("portal.dat")).unwrap();
        let catalog=Catalog::load(&mut dat,&mut BTreeMap::new()).unwrap();let ai=crate::creature_ai::catalog().unwrap();
        let mut state=5;
        let a=catalog.roll(0,0x470008bf,ai[&0x470008bf],&mut state);
        let b=catalog.roll(0,0x470008bf,ai[&0x470008bf],&mut state);
        state=5;let c=catalog.roll(0,0x47000fa2,ai[&0x47000fa2],&mut state);
        assert!(!a.is_empty()&&!b.is_empty()&&c.len()>=2,"UDP claim fixtures need claimable rolls with seed 5");
        state=5;let boss=catalog.roll(0,0x4700390b,crate::creature_ai::Profile::default(),&mut state);
        assert_eq!(boss.len(),4);
        assert!(boss.iter().any(|i|i.definition==0x47000f0b && i.roll.unwrap().damage==115));
        assert!(boss.iter().any(|i|crate::treasure_weapons::profile(i).is_some_and(|p|p.minimum_level==15)));
    }
}
