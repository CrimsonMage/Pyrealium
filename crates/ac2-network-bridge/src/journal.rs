// Native progression messages shared by Pyrealium-family servers.
use ac2_net::protocol::*;
use serde_json::{json, Value};

fn text(s: &StringInfo) -> Value { json!({"literal":s.literal,"table":s.table_id,"key":s.string_id}) }
fn skill(s: &SkillRecord) -> Value { json!({"id":s.skill,"trained":s.trained(),"xp":s.xp.to_string()}) }
pub fn repository(r: &SkillRepository) -> Value {
    json!({"event":"skills","replace":true,"credits":r.credits,"hero_credits":r.hero_credits,
        "untraining":r.untraining,"items":r.skills.iter().map(skill).collect::<Vec<_>>()})
}
fn quest(q: &GmQuestInfo) -> Value {
    json!({"id":q.id,"name":text(&q.name),"description":text(&q.description),"journal":text(&q.journal),
        "icon":q.icon,"status":q.status,"phase":q.phase,"level":q.challenge_level,"done":q.done,
        "expires":q.expires,"remaining":q.seconds_remaining})
}
pub fn quests(v: &PackValue, replace: bool) -> Option<Value> {
    let PackValue::Reference(r)=v else {return None};
    let PackedEntry::Heap{object,..}=r.entries.first()? else {return None};
    let items=match object.native.as_ref()? {
        NativeObject::GmQuestInfo(q)=>vec![quest(q)],
        NativeObject::GmQuestInfoList(q)=>q.iter().map(quest).collect(),
        _=>return None,
    };
    Some(json!({"event":"quests","replace":replace,"items":items}))
}
pub fn decode(function: u32, args: &[u8]) -> Option<Value> {
    if function==0x82130024 {
        let values=unpack_values(args).ok()?;
        return quests(values.get(1)?,false);
    }
    Some(match SkillMessage::decode(function,args)? {
        Ok(SkillMessage::SetRepository(r))=>repository(&r),
        Ok(SkillMessage::SetSkills(s))=>json!({"event":"skills","replace":true,"items":s.iter().map(skill).collect::<Vec<_>>()}),
        Ok(SkillMessage::UpdateSkill(s))=>json!({"event":"skills","replace":false,"items":[skill(&s)]}),
        Ok(SkillMessage::RemoveSkill(id))=>json!({"event":"skill_remove","id":id}),
        Err(e)=>json!({"event":"error","message":format!("Cannot decode native skills: {e}")}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_skill_changes_keep_credits_training_and_full_width_xp() {
        let r=SkillRepository {credits:3,hero_credits:2,untraining:17,
            skills:vec![SkillRecord{skill:17,flags:1,xp:u64::MAX}],};
        let m=SkillMessage::SetRepository(r);
        let v=decode(m.function(),&pack_values(&m.arguments())).unwrap();
        assert_eq!(v["credits"],3);assert_eq!(v["items"][0]["trained"],true);
        assert_eq!(v["items"][0]["xp"],u64::MAX.to_string());
        let m=SkillMessage::RemoveSkill(17);
        assert_eq!(decode(m.function(),&pack_values(&m.arguments())).unwrap()["id"],17);
    }
    #[test]
    fn quest_lists_and_updates_keep_native_strings_and_status() {
        let q=GmQuestInfo {id:42,phase:3,status:0x30000000,..Default::default()};
        let v=PackValue::Reference(PackedReference::native(NativeObject::GmQuestInfoList(vec![q.clone()])));
        assert_eq!(quests(&v,true).unwrap()["items"][0]["phase"],3);
        let args=pack_values(&[PackValue::Int(0),PackValue::Reference(PackedReference::native(NativeObject::GmQuestInfo(Box::new(q)))),PackValue::Int(0)]);
        let decoded=decode(0x82130024,&args).unwrap();
        assert_eq!(decoded["replace"],false);assert_eq!(decoded["items"][0]["status"],0x30000000u32);
    }
}
