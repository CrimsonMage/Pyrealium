//! Read-only NPC conversations. Explicit native identities; no display-name
//! dispatch, item consumption, reward or quest advancement in this fallback.
//! Existing quest/shop handlers must run before this catalog.
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Debug, Deserialize)]
pub struct Topic { pub title:String, pub text:String }
#[derive(Debug, Deserialize)]
pub struct Profile { pub entity:u32, pub name:String, pub topics:Vec<Topic> }

pub fn profiles()->&'static BTreeMap<u32,Profile> {
    static DATA:OnceLock<BTreeMap<u32,Profile>>=OnceLock::new();
    DATA.get_or_init(|| {
        #[derive(Deserialize)] struct Catalog { version:u32, profiles:Vec<Profile> }
        let catalog:Catalog=serde_json::from_str(include_str!("../../../content/npc-dialogue-v1.json"))
            .expect("validated NPC dialogue catalog");
        assert_eq!(catalog.version,1);
        let mut result=BTreeMap::new();
        for p in catalog.profiles {
            assert!(p.entity>>24==0x47 && !p.name.trim().is_empty() && !p.topics.is_empty());
            for i in 0..p.topics.len() {
                let text=p.reply(i);
                assert!(crate::interaction::speech(1,&p.name,&text).is_ok());
                assert!(crate::interaction::dialogue(&p.name,&text).is_ok());
            }
            assert!(result.insert(p.entity,p).is_none());
        }
        result
    })
}

impl Profile {
    pub fn reply(&self, page:usize)->String {
        let index=page%self.topics.len();
        let text=&self.topics[index].text;
        if self.topics.len()==1 {text.clone()} else {
            format!("{text} (Topic {} of {}. Speak with me again for the next topic.)",index+1,self.topics.len())
        }
    }
}

/// Bounded session-local browsing, reset when switching speaker or character.
/// The client cannot supply page indices or choose a different NPC identity.
#[derive(Default)]
pub struct Cursor { previous:Option<(u64,u64,usize)> }
impl Cursor {
    pub fn page(&self, owner:u64, target:u64)->usize {
        self.previous.filter(|(o,t,_)|*o==owner && *t==target).map_or(0,|(_,_,p)|p)
    }
    pub fn advance(&mut self, owner:u64,target:u64,count:usize) {
        self.previous=Some((owner,target,(self.page(owner,target)+1)%count.max(1)));
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn every_reply_fits_native_speech_and_dialogue_and_has_exact_identity() {
        let data=profiles();assert!(data.len()>200);
        assert_eq!(data[&0x470022fe].name,"Digger 'Dillo Mama");
        assert!(data[&0x470022fe].topics.iter().any(|t|t.title=="Nature's Ally"));
        assert!(data[&0x47002308].topics.iter().any(|t|t.title=="Nature's Ally"));
        for entity in [0x47002795,0x47002796] {
            assert!(data[&entity].topics.iter().any(|t|t.title=="Gurog Civil War"));
        }
        assert!(!data.contains_key(&0x47003c3a),"hostile Zroth must not gain friendly dialogue");
        assert!(!data.contains_key(&0x47003e6a),"hostile Rashan must not gain friendly dialogue");
        assert!(data.values().all(|p|p.name!="Foundry Gates"));
        for p in data.values() {assert_eq!(p.reply(0),p.reply(p.topics.len()));}
    }
    #[test] fn browsing_is_bounded_and_isolated_between_characters_and_speakers() {
        let mut c=Cursor::default();assert_eq!(c.page(1,10),0);
        c.advance(1,10,3);assert_eq!(c.page(1,10),1);
        assert_eq!(c.page(2,10),0);assert_eq!(c.page(1,11),0);
        c.advance(1,10,3);c.advance(1,10,3);assert_eq!(c.page(1,10),0);
    }
}
