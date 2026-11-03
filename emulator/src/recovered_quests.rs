//! Version-one inferred solo hunt rules, based on native effects and guide tables.
//! Immutable journal policy. Timers use persisted UTC seconds, not session uptime.
use std::{collections::BTreeMap,sync::OnceLock};
use serde::Deserialize;
#[derive(Debug,Deserialize)]
pub struct Profile {
    pub id:u32,pub title:String,pub text:String,pub goal:u16,pub targets:Vec<u32>,
    pub kill_starters:Vec<u32>,pub use_starters:Vec<u32>,pub level:u16,pub xp:u64,
    pub repeat_seconds:u64,pub limit_seconds:u64,
    #[serde(default)] pub delivery:bool,
}
pub fn profiles()->&'static [Profile] {
    static DATA:OnceLock<Vec<Profile>>=OnceLock::new();
    DATA.get_or_init(||{
        #[derive(Deserialize)] struct Catalog {version:u32,quests:Vec<Profile>}
        let d:Catalog=serde_json::from_str(include_str!("../content/inferred-content-v1.json")).expect("validated inferred quest catalog");
        assert_eq!(d.version,1);let mut ids=std::collections::BTreeSet::new();
        for q in &d.quests {assert!(ids.insert(q.id)&&q.id>>24==0x40&&(1..=100).contains(&q.goal)&&(!q.targets.is_empty()||q.delivery&&q.id==DEBT)
            &&!q.title.is_empty()&&(1..=150).contains(&q.level)&&q.xp>0&&q.xp<=100_000_000
            &&q.limit_seconds>0&&q.repeat_seconds>0);}
        d.quests
    })
}
#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct State {pub count:u16,pub started:u64,pub completed:u64}
#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct Book(pub BTreeMap<u32,State>);
pub fn now()->u64 {std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()}
pub fn use_profile(entity:u32)->Option<&'static Profile>{profiles().iter().find(|q|q.use_starters.contains(&entity))}
pub const DEBT:u32=0x4000fe01;
pub const KANTEN:u32=0x47001a3c;
pub const ORVEN:u32=0x47001c27;
pub const BAG:u32=0x47001c1e;
pub fn delivery_npc(entity:u32)->bool {matches!(entity,KANTEN|ORVEN)}
pub fn delivery_metadata(dat:&mut crate::dat::Dat)->std::io::Result<crate::inventory_request::Metadata> {
    dat.read(0x1f00103f)?;crate::inventory_request::Metadata::load(dat,BAG)
}
impl Book {
    /// Server-derived inventory exchange: no packet-supplied rewards/items.
    pub fn delivery(&self,bag:&crate::inventory::Inventory,npc:u32,level:u16,now:u64,first:u64)
        ->std::io::Result<(Vec<crate::inventory::Command>,Vec<crate::progression::Command>,Self)> {
        use crate::inventory::{Item,Location,Command as I};
        let bad=|s:&str|std::io::Error::other(s);
        let q=profiles().iter().find(|q|q.id==DEBT).unwrap();
        let owned=bag.items().find(|i|i.definition==BAG&&i.visual==0x1f00103f&&i.quantity==1&&i.stack_limit==1&&matches!(i.location,Location::Bag(_)));
        if npc==KANTEN {
            if self.0.get(&DEBT).is_some_and(|s|s.completed==0&&now>=s.started&&now<s.started.saturating_add(q.limit_seconds)) {
                return Err(bad("Deliver Kanten's Bag to Orven in Arwic before returning for another job."));
            }
            let next=self.accept(DEBT,level,now)?;
            let items=if owned.is_some(){vec![]}else{vec![I::Grant(Item{roll:None,id:first,definition:BAG,visual:0x1f00103f,
                quantity:1,stack_limit:1,equipment_mask:0,location:Location::Bag(bag.free_slot().ok_or_else(||bad("Make room for Kanten's Bag."))?)})]};
            return Ok((items,vec![],next));
        }
        if npc!=ORVEN{return Err(bad("This NPC does not accept Kanten's Bag."));}
        let state=self.0.get(&DEBT).filter(|s|s.completed==0&&now>=s.started&&now<s.started.saturating_add(q.limit_seconds))
            .ok_or_else(||bad("Speak to Kanten for a delivery job. Expired jobs must be renewed there."))?;
        let item=owned.ok_or_else(||bad("Bring Kanten's Bag in your backpack to complete Gambling Debt."))?;
        let mut next=self.clone();next.0.insert(DEBT,State{count:1,completed:now,..state.clone()});
        Ok((vec![I::ConsumeDense{item:item.id}],vec![crate::progression::Command::AwardXp(q.xp)],next))
    }
    fn can_start(&self,q:&Profile,level:u16,now:u64)->bool {
        level>=q.level && self.0.get(&q.id).is_none_or(|s|if s.completed>0 {
            now>=s.completed.saturating_add(q.repeat_seconds)
        } else {now>=s.started.saturating_add(q.limit_seconds)})
    }
    pub fn accept(&self,id:u32,level:u16,now:u64)->std::io::Result<Self> {
        let q=profiles().iter().find(|q|q.id==id).ok_or_else(||std::io::Error::other("Unknown recovered quest."))?;
        if now==0||!self.can_start(q,level,now){return Err(std::io::Error::other(format!("{} requires level {} and an available quest timer.",q.title,q.level)));}
        let mut next=self.clone();next.0.insert(id,State{started:now,..Default::default()});Ok(next)
    }
    /// Only authoritative first death of a live encounter may call this.
    /// A starter kill bestows but does not also count toward that same quest.
    pub fn kill(&self,entity:u32,level:u16,now:u64)->(Self,u64) {
        let mut next=self.clone();let mut xp=0;
        for q in profiles().iter().filter(|q|!q.delivery) {
            if q.kill_starters.contains(&entity)&&self.can_start(q,level,now) {
                next.0.insert(q.id,State{started:now,..Default::default()});continue;
            }
            let Some(s)=next.0.get_mut(&q.id) else{continue};
            if s.completed>0||now<s.started||now>=s.started.saturating_add(q.limit_seconds)||!q.targets.contains(&entity){continue;}
            s.count=(s.count+1).min(q.goal);
            if s.count==q.goal{s.completed=now;xp+=q.xp;}
        }
        (next,xp)
    }
    pub fn guidance(q:&Profile,s:&State)->String {
        if s.completed>0 {format!("{} complete. Earned {} experience. Repeat after {} minutes.",q.title,q.xp,q.repeat_seconds/60)}
        else {format!("{}\n\nProgress: {} / {}. Reward: {} experience. Time limit: {} minutes.",q.text,s.count,q.goal,q.xp,q.limit_seconds/60)}
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn native_starter_target_count_timers_and_once_only_rewards() {
        let q=profiles().iter().find(|q|q.id==0x40000097).unwrap();let start=q.kill_starters[0];let target=q.targets[0];
        let b=Book::default();assert_eq!(b.kill(start,q.level-1,100).0,b);
        let (mut b,xp)=b.kill(start,q.level,100);assert_eq!(xp,0);assert_eq!(b.0[&q.id].count,0);
        assert_eq!(b.kill(0x47000000,150,101).0,b);
        for i in 0..q.goal {let(n,xp)=b.kill(target,150,102+u64::from(i));b=n;assert_eq!(xp,if i+1==q.goal{q.xp}else{0});}
        let finish=b.0[&q.id].completed;assert_eq!(b.kill(target,150,finish+1).1,0);
        assert_eq!(b.kill(start,150,finish+1).0,b);
        assert_eq!(b.kill(start,150,finish+q.repeat_seconds).0.0[&q.id].count,0);
        let expired=Book::default().kill(start,150,100).0;
        assert_eq!(expired.kill(target,150,100+q.limit_seconds).0,expired);
    }
    #[test] fn elixirs_require_acceptance_and_do_not_grant_kill_credit() {
        let q=use_profile(0x47000f43).unwrap();let b=Book::default();
        assert_eq!(b.kill(q.targets[0],150,100).0,b);
        let b=b.accept(q.id,150,100).unwrap();assert!(b.accept(q.id,150,101).is_err());
        assert_eq!(b.kill(q.targets[0],150,101).0.0[&q.id].count,1);
    }
    #[test] fn all_inferred_quests_encode_in_native_journal_and_finish_once() {
        let mut journal=crate::quests::Book::default();
        for q in profiles().iter().filter(|q|!q.delivery) {
            let mut book=Book::default().accept(q.id,150,100).unwrap();
            for i in 0..q.goal {
                // Accept explicitly isolates progression from other quest starters.
                let (next,xp)=book.kill(q.targets[0],150,101+u64::from(i));book=next;
                assert_eq!(book.0[&q.id].count,i+1,"{}",q.title);
                assert_eq!(xp,if i+1==q.goal{q.xp}else{0},"{}",q.title);
            }
            journal.recovered.0.insert(q.id,book.0[&q.id].clone());
        }
        assert!(journal.list_graph().unwrap().len()>1000);
        assert!(journal.list_graph().unwrap().len()<48*1024,"Leave room for native UI_Load inventory/skill fields");
        assert_eq!(journal.updates().unwrap().len(),profiles().iter().filter(|q|!q.delivery).count());
    }
}
