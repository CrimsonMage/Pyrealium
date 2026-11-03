//! Build-7148 native examination request/profile subset. Never executes scripts.
use crate::{
    dat::Dat,
    script,
    world::{self, WorldError},
};
use std::{collections::BTreeMap, io, path::Path};

pub const REQUEST: u32 = 0x820a0007;

/// Runtime inventory state, not inferred ownership or a visual's default stack.
pub fn item_lines(item:&crate::inventory::Item,location:&str)->Vec<String> {
    let mut lines=vec![location.to_owned(),format!("Quantity: {} / {}",item.quantity,item.stack_limit)];
    let slots=[(1,"Head"),(2,"Torso"),(4,"Back"),(8,"Legs"),(16,"Feet"),(32,"Hands"),(0x40,"Main hand"),(0x80,"Off hand"),(0x100,"Ring I"),(0x200,"Ring II"),(0x400,"Neck")];
    let mask=crate::apparel::recovered_slot(item.definition,item.visual).unwrap_or(item.equipment_mask);
    let names:Vec<_>=slots.iter().filter(|(slot,_)|mask&slot!=0).map(|(_,name)|*name).collect();
    if !names.is_empty(){lines.push(format!("Equipment: {}",names.join(", ")));}
    if let Some(armor)=item.roll.map(|r|u32::from(r.armor)).or_else(||crate::apparel::item_armor(item.definition)).filter(|v|*v>0){lines.push(format!("Armor: {armor}"));}
    else if crate::apparel::required_race(item.definition).is_some(){lines.push("Armor rating unavailable.".into());}
    if let Some(r)=item.roll {lines.push(format!("{} level: {}",if r.crafted{"Crafted"}else{"Treasure"},r.level));if r.damage>0{lines.push(format!("Weapon damage: {}",r.damage));}}
    if let Some(p)=crate::treasure_weapons::profile(item) {
        lines.push(format!("Required level: {}",p.minimum_level));
        if item.roll.is_none(){lines.push("Weapon damage rating unavailable.".into());}
    }
    if item.definition==crate::arwic_mines::SWORD {lines.push("Weapon damage: 10".into());lines.push("Return this quest sword to the Arwic Mines Overseer in your backpack.".into());}
    if item.definition==crate::arwic_mines::GUIDE {lines.push("Double-click to read the next of 15 pages. Text appears in chat.".into());}
    if let Some(q)=crate::recovered_quests::use_profile(item.definition){lines.push(format!("Begins {}. Level {}+. Defeat {} targets for {} XP.",q.title,q.level,q.goal,q.xp));}
    if let Some(race)=crate::apparel::required_race(item.definition){lines.push(format!("Race: {}",match race{1=>"Human",2=>"Lugian",4=>"Tumerok",8=>"Drudge",16=>"Empyrean",_=>"Unknown"}));}
    if item.definition==crate::item_services::POTION_DEFINITION && item.visual==crate::item_services::POTION_VISUAL {
        lines.push(format!("Restores {} health.",crate::item_services::POTION_HEALTH));
        lines.push("Requires missing health. Potion recovery: 5 seconds.".into());
    } else if item.definition==crate::crafting::VIGOR_OUTPUT && item.visual==0x1f000e40 {
        lines.push("Restores 25 vigor.".into());
        lines.push("Requires missing vigor. Potion recovery: 5 seconds.".into());
    }
    lines
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Object(u64),
    Clear,
}

/// Decode only the object and cleared request profiles proved by build 7148.
/// Clear/default construction (00609530 / 006095f0) sets type and admin to
/// zero; StreamPack 00609660 then emits those two words without a union value.
pub fn decode_request(bytes: &[u8]) -> Result<Request, WorldError> {
    // 36 and 44 are the 7148 forms. The 7230 client's object request carries the examination
    // panel's tab filter word (0x203 for the General tab) where 7148 had a zero, and one more
    // zero word after it (48 bytes); its clear request is the 7148 one.
    if !matches!(bytes.len(), 36 | 44 | 48) {
        return Err(WorldError::InvalidValue);
    }
    let word = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if word(0) != 5
        || word(4) != 1
        || [0, u32::MAX].contains(&word(8))
        || word(12) != 0x44000100
        || word(16) != 0
        || word(20) != 0xffff009a
        || word(28) != 0
    {
        return Err(WorldError::InvalidValue);
    }
    match (word(24), bytes.len()) {
        (0, 36) if word(32) == 0 => Ok(Request::Clear),
        (0x40000002, 44) if word(40) == 0 => {
            let id = u64::from(word(32)) | (u64::from(word(36)) << 32);
            if [0, u64::MAX].contains(&id) {
                return Err(WorldError::InvalidValue);
            }
            Ok(Request::Object(id))
        }
        (0x40000002, 48) if word(44) == 0 => {
            let id = u64::from(word(32)) | (u64::from(word(36)) << 32);
            if [0, u64::MAX].contains(&id) {
                return Err(WorldError::InvalidValue);
            }
            Ok(Request::Object(id))
        }
        _ => Err(WorldError::InvalidValue),
    }
}

/// Legacy object-only entry point; clear requests are intentionally distinct.
pub fn decode(bytes: &[u8]) -> Result<u64, WorldError> {
    match decode_request(bytes)? {
        Request::Object(id) => Ok(id),
        Request::Clear => Err(WorldError::InvalidValue),
    }
}

#[derive(Clone, Debug)]
pub enum Name {
    Literal(String),
    Localized([u32; 3]),
    Corpse(Box<Name>),
}
impl Name {
    pub fn wire(&self) -> Result<Vec<u8>,WorldError> {
        match self {
            Self::Literal(s)=>world::literal_name(s),
            Self::Localized(words)=>Ok(words.iter().flat_map(|w|w.to_le_bytes()).collect()),
            Self::Corpse(name)=>{
                // Native localized corpse-name StringInfo and nested victim name.
                let mut bytes=Vec::new();
                for word in [0x509a8ff6u32,0x25000011,1,0xe4ea98e6,9] {bytes.extend(word.to_le_bytes());}
                bytes.extend(name.wire()?);Ok(bytes)
            }
        }
    }
}
#[derive(Default)]
pub struct Names(BTreeMap<u32, Option<Name>>, BTreeMap<u32, (u32, String)>);
impl Names {
    pub fn item_lines(&self,item:&crate::inventory::Item,location:&str)->Vec<String> {
        let mut lines=item_lines(item,location);
        lines.extend(self.description_lines(item.definition));
        lines.truncate(12);
        lines
    }
    pub fn load(directory: &Path, definitions: impl Iterator<Item = u32>) -> io::Result<Self> {
        let mut portal = Dat::open(directory.join("portal.dat"))?;
        let dataset=ac2_dat::DatDataset::open(ac2_dat::DatOpenOptions::for_retail_install(directory.to_path_buf())).map_err(io::Error::other)?;
        let tables=ac2_db::StoreStringTables::new(ac2_db::ResourceStore::new(dataset),0x58000000);
        let mut names = Self::default();
        for id in definitions {
            if names.0.contains_key(&id) {
                continue;
            }
            if names.0.len() >= 16384 {
                return Err(io::Error::other("examination name capacity exceeded"));
            }
            // An unavailable default name cannot prevent entering the world.
            // No substituted StringInfo or unsupported prefix is offset-guessed.
            let name=Self::read(&mut portal, id).ok();
            if let Some(Name::Localized([_,table,0]))=&name {
                // The native Description token is independent of the name. An
                // absent/empty/unresolved entry is not substituted with a name.
                let info=ac2_db::RenderInfo {string_id:0x6c459364,table_id:*table,..Default::default()};
                if let Ok(text)=ac2_db::inq_string(&tables,&info,false) {
                    let text=String::from_utf16_lossy(&text);
                    if !text.trim().is_empty() {names.1.insert(id,(*table,text));}
                }
            }
            names.0.insert(id, name);
        }
        Ok(names)
    }
    pub fn get(&self, definition: u32, fallback: &str) -> Name {
        if definition==crate::arwic_mines::GLYPH {return Name::Literal(crate::arwic_mines::name(definition).unwrap().into());}
        self.0
            .get(&definition)
            .and_then(|name| name.clone())
            .unwrap_or_else(|| Name::Literal(fallback.to_owned()))
    }
    pub fn description_chat(&self, definition:u32)->Option<Result<Vec<u8>,WorldError>> {
        let (table,_)=self.1.get(&definition)?;
        Some(script::localized(*table,0x6c459364).and_then(|mut args| {
            args.extend(script::integer(1)); script::private_call(script::CHAT_STRING,&args)
        }))
    }
    pub fn description_lines(&self,definition:u32)->Vec<String> {
        let Some((_,text))=self.1.get(&definition) else{return Vec::new()};
        let mut lines=Vec::new();let mut line=String::new();
        for word in text.split_whitespace() {
            if word.len()>480 || word.chars().any(char::is_control){continue;}
            if line.len()+word.len()+1>480 {lines.push(std::mem::take(&mut line));if lines.len()==3{return lines;}}
            if !line.is_empty(){line.push(' ');}line.push_str(word);
        }
        if !line.is_empty(){lines.push(line);}lines
    }
    fn read(portal: &mut Dat, definition: u32) -> io::Result<Name> {
        let invalid = || io::Error::other("unsupported definition name prefix");
        if definition >> 24 != 0x47 {
            return Err(invalid());
        }
        let did = 0x81000000 | (definition & 0xffffff);
        let bytes = portal.read(did)?;
        if bytes.len() > 65536 {
            return Err(invalid());
        }
        let mut pos = 0;
        let mut word = || -> io::Result<u32> {
            let data = bytes.get(pos..pos + 4).ok_or_else(invalid)?;
            pos += 4;
            Ok(u32::from_le_bytes(data.try_into().unwrap()))
        };
        if word()? & 1 == 0 || word()? != did {
            return Err(invalid());
        }
        let desc = word()?;
        if desc & 4 == 0 {
            return Err(invalid());
        }
        // Only these fields precede the name in gmWeenieDesc::StreamPack.
        for bit in [1, 0x2000000, 2] {
            if desc & bit != 0 {
                word()?;
            }
        }
        let name = [word()?, word()?, word()?];
        if name[0] == 0 || name[1] >> 24 != 0x25 || name[2] != 0 {
            return Err(invalid());
        }
        Ok(Name::Localized(name))
    }
}

/// ExaminationProfile::StreamPack 00608380, node pack 00608490 and list pack
/// 00609140. The matching 7148 profile ends after its node list, unlike newer
/// retained capture profiles. HeapObject::Pack 006dbb00 adds zero field count.
pub fn response(target: u64, name: Name, lines: &[String]) -> Result<Vec<u8>, WorldError> {
    if lines.len() > 12
        || lines
            .iter()
            .any(|line| line.len() > 512 || line.chars().any(char::is_control))
    {
        return Err(WorldError::LimitExceeded);
    }
    let mut bytes = Vec::new();
    // The 7230 client reads the Legions layout: the object request carries the panel's tab word
    // after the object (the General tab, 0x203), and the profile ends with the tabs it offers
    // after its nodes (the same word). The 7148 layout has neither.
    let newer = crate::data_profile::unconverted_7230();
    for word in [5, 1, 1, 0x44000100, 0, 0xffff0099, 0x40000002, 0, target as u32, (target >> 32) as u32] {
        bytes.extend(u32::to_le_bytes(word));
    }
    if newer {
        bytes.extend(0x203u32.to_le_bytes());
    }
    bytes.extend(u32::to_le_bytes(((lines.len() + 1) * 2) as u32));
    let text: Vec<_> = std::iter::once(name)
        .chain(lines.iter().cloned().map(Name::Literal))
        .collect();
    for (index, text) in text.into_iter().enumerate() {
        for word in [
            0x40000004,
            2,
            if index == 0 { 0xbe5b6513 } else { 0x33be4035 },
        ] {
            bytes.extend(u32::to_le_bytes(word));
        }
        bytes.extend(text.wire()?);
        bytes.extend(0x40000006u32.to_le_bytes()); // Break closes the current row.
        bytes.extend(2u32.to_le_bytes());
    }
    if newer {
        bytes.extend(0x203u32.to_le_bytes());
    }
    bytes.extend(0u32.to_le_bytes());
    script::private_call(0x820a0009, &bytes)
}

#[cfg(test)]
mod item_tests {
    use super::*;
    use crate::inventory::{Item,Location,ITEM_NAMESPACE};
    #[test]
    fn inventory_profiles_include_descriptions_and_only_supported_effects() {
        let mut names=Names::default();
        let mut item=Item {roll: None, id:ITEM_NAMESPACE+1,definition:crate::item_services::POTION_DEFINITION,
            visual:crate::item_services::POTION_VISUAL,quantity:1,stack_limit:1,equipment_mask:0,location:Location::Bag(0)};
        names.1.insert(item.definition,(0x25000001,"Native item description.".into()));
        let lines=names.item_lines(&item,"In your inventory.");
        assert!(lines.iter().any(|s|s.contains("Restores 25 health")));
        assert!(lines.iter().any(|s|s=="Native item description."));
        assert!(response(item.id,Name::Literal("Potion".into()),&lines).is_ok());
        item.definition=0x47000f43;item.visual=0x1f000e52;
        let lines=names.item_lines(&item,"In your inventory.");
        assert!(!lines.iter().any(|s|s.contains("Restores")),"Slayer elixirs are not healing potions");
        item.definition=0x47000fdc;item.visual=0x1f00058c;
        let lines=names.item_lines(&item,"In the loot container.");
        assert!(lines.iter().any(|s|s=="Equipment: Feet"));
        assert!(!lines.iter().any(|s|s.starts_with("Armor:")),"Rolled armor cannot be inferred from a boot mesh");
    }
}
