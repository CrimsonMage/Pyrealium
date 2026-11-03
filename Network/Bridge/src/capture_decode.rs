// SPDX-License-Identifier: AGPL-3.0-only
//! Offline evidence adapter. Input: bounded u32-length-prefixed event blobs.
//! Output excludes login, chat, private player state, and non-world identities.
use ac2_net::protocol::*;
use serde_json::{json, Value};
use std::io::{self, Read, Write};

fn world(id: u64) -> bool {
    matches!(id >> 32, 0x81200000 | 0x81300000) || id >> 56 == 0x40
}
fn name(s: &StringInfo) -> Value {
    json!({"literal":s.literal,"table":s.table_id,"key":s.string_id,"variables":s.variables.len()})
}
fn decode(bytes: &[u8]) -> Value {
    let event = match ServerEvent::decode(bytes, Build::Legions7160) {
        Ok(v) => v,
        Err(e) => return json!({"error":e.to_string()}),
    };
    match event {
        ServerEvent::CreateObject(o) if world(o.id) => {
            let Some(p) = o.physics.position else {return json!({"excluded":"unplaced"})};
            if o.physics.parent.is_some() || o.weenie.container.is_some() || o.weenie.wielder.is_some()
                || o.weenie.pet_summoner.is_some() || o.weenie.killer.is_some() || o.weenie.claimant.is_some() {
                return json!({"excluded":"owned-attached-pet-or-corpse"});
            }
            json!({"type":"create","source_iid":format!("{:016x}",o.id),
                "entity":o.weenie.entity_did,"visual":o.visual.parent.or(o.visual.did),
                "cell":p.cell,"position":p.origin,"orientation":p.rotation,
                "scale":o.weenie.scale,"visual_mask":o.visual.bitfield,"visual_scale":o.visual.scale,
                "physics_mask":o.physics.bitfield,"mode":o.physics.mode,
                "velocity":o.physics.velocity,"behaviors":o.physics.behaviors.iter().map(|b|json!({"id":b.behavior,"mode":b.mode,"rate":b.time_scale})).collect::<Vec<_>>(),
                "name":o.weenie.name.as_ref().map(name),"icon":o.weenie.icon,
                "faction":o.weenie.faction_type,"description_mask":o.weenie.data_mask})
        }
        ServerEvent::Object(o) if world(o.sender.id) => {
            let data = match o.action {
                ObjectAction::Parent {..} => json!({"type":"attached"}),
                ObjectAction::DoMode(v) | ObjectAction::SetMode(v) => json!({"type":"mode","value":v}),
                ObjectAction::DoBehavior(b) => json!({"type":"behavior","value":b.behavior,"mode":b.mode,"rate":b.time_scale}),
                ObjectAction::DoFx {fx,scalar} => json!({"type":"fx","value":fx,"scalar":scalar}),
                ObjectAction::DoTargettedFx {fx,scalar,..} => json!({"type":"target_fx","value":fx,"scalar":scalar}),
                ObjectAction::DoFxScript(v) => json!({"type":"fx_script","value":v}),
                ObjectAction::MoveTo {..} | ObjectAction::DoStory(_) => json!({"type":"moving"}),
                _ => return Value::Null,
            };
            let mut data=data; data["source_iid"]=json!(format!("{:016x}",o.sender.id));data
        }
        ServerEvent::Contain(o) if world(o.id) => json!({"type":"attached","source_iid":format!("{:016x}",o.id)}),
        ServerEvent::UpdatePosition(o) if world(o.sender.id) => json!({"type":"position","source_iid":format!("{:016x}",o.sender.id),
            "cell":o.position.cell,"position":o.position.origin,"motion":o.motion}),
        ServerEvent::DestroyObject(o) if world(o.id) => json!({"type":"destroy","source_iid":format!("{:016x}",o.id)}),
        ServerEvent::Interp {function,..} => json!({"type":"script","function":function}),
        _ => Value::Null,
    }
}
/// Separate loot evidence stream: only world creatures/corpses and objects
/// contained by world identities. Never export a player's bag or killer IID.
fn decode_loot(bytes: &[u8]) -> Value {
    let Ok(ServerEvent::CreateObject(o))=ServerEvent::decode(bytes,Build::Legions7160) else {return Value::Null};
    let contained=o.weenie.container.filter(|id|world(*id));
    if contained.is_none() && (!world(o.id) || o.physics.position.is_none() || o.weenie.container.is_some()
        || o.weenie.wielder.is_some() || o.weenie.pet_summoner.is_some() || o.physics.parent.is_some()) {return Value::Null;}
    json!({"source_iid":format!("{:016x}",o.id),"entity":o.weenie.entity_did,
        "visual":o.visual.parent.or(o.visual.did),"container":contained.map(|id|format!("{id:016x}")),
        "name":o.weenie.name.as_ref().map(loot_name),"position":o.physics.position.map(|p|json!({"cell":p.cell,"position":p.origin})),
        "quantity":o.weenie.quantity,
        "corpse":o.weenie.killer.is_some() || o.weenie.claimant.is_some()})
}
fn loot_name(s:&StringInfo)->Value {
    // No literal names or non-string variables (which may carry private IIDs).
    json!({"table":s.table_id,"key":s.string_id,"children":s.variables.iter().filter_map(|(_,v)|
        if let StringVariable::Nested(n)=v {Some(loot_name(n))}else{None}).collect::<Vec<_>>()})
}
fn run(mut input: impl Read, mut out: impl Write) -> io::Result<()> {
    let loot=std::env::args().any(|arg|arg=="--loot");
    loop {
        let mut size=[0;4];
        if input.read(&mut size[..1])? == 0 { break; }
        input.read_exact(&mut size[1..])?;
        let size=u32::from_le_bytes(size) as usize;
        if !(4..=65536).contains(&size) {return Err(io::Error::other("event size limit"))}
        let mut bytes=vec![0;size];input.read_exact(&mut bytes)?;
        serde_json::to_writer(&mut out,&if loot {decode_loot(&bytes)}else{decode(&bytes)})?;out.write_all(b"\n")?;
    }
    out.flush()
}
fn main() -> io::Result<()> {
    run(io::stdin().lock(), io::BufWriter::new(io::stdout().lock()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn object(id: u64, owner: Option<u64>) -> Vec<u8> {
        let mut b=Vec::new();
        b.extend(0x10058u32.to_le_bytes()); b.extend(id.to_le_bytes());
        for word in [2u32,0x1f000023,0x8080,0x10001,0x41000004,0x02990109] {
            b.extend(word.to_le_bytes());
        }
        for value in [59.0f32,240.0,-44.5,1.0,0.0,0.0,0.0] {b.extend(value.to_le_bytes());}
        b.extend([0;8]);
        b.extend((0x2000000u32|if owner.is_some(){0x20}else{0}).to_le_bytes());
        b.extend(0x4700110fu32.to_le_bytes());
        if let Some(owner)=owner { b.extend(owner.to_le_bytes()); }
        b
    }
    #[test]
    fn retains_world_state_and_filters_player_and_owned_objects() {
        let v=decode(&object(0x8130000000001234,None));
        assert_eq!(v["mode"],0x41000004u32);
        assert_eq!(v["entity"],0x4700110fu32);
        assert_eq!(v["position"][2],-44.5);
        assert_eq!(decode(&object(0x8140000000000001,None)),Value::Null);
        assert_eq!(decode(&object(0x8130000000001234,Some(1)))["excluded"],"owned-attached-pet-or-corpse");
    }
    #[test]
    fn rejects_partial_lengths_and_oversized_events() {
        assert!(run(&[1,2][..],Vec::new()).is_err());
        assert!(run(&65537u32.to_le_bytes()[..],Vec::new()).is_err());
        assert!(run(&4u32.to_le_bytes()[..],Vec::new()).is_err());
        assert!(run(&[][..],Vec::new()).is_ok());
    }
    #[test]
    fn loot_evidence_excludes_player_bags_and_player_actors() {
        assert_eq!(decode_loot(&object(0x8140000000000001,None)),Value::Null);
        assert_eq!(decode_loot(&object(0x8130000000001234,Some(0x8140000000000001))),Value::Null);
        let v=decode_loot(&object(0x8130000000001234,Some(0x8130000000005678)));
        assert_eq!(v["container"],"8130000000005678");
        assert_eq!(v["entity"],0x4700110fu32);
        assert!(v.get("killer").is_none());assert!(v.get("claimant").is_none());
    }
    #[test]
    fn corpse_name_export_keeps_only_native_string_references() {
        let nested=StringInfo{string_id:3,table_id:4,literal:Some("private name".into()),variables:vec![]};
        let name=StringInfo{string_id:1,table_id:2,literal:Some("private corpse name".into()),variables:vec![
            (1,StringVariable::Long{kind:10,value:0x8140000000000001}),
            (2,StringVariable::Nested(Box::new(nested)))]};
        assert_eq!(loot_name(&name),json!({"table":2,"key":1,"children":[{"table":4,"key":3,"children":[]}]}));
    }
    #[test]
    fn reports_one_result_per_bounded_input() {
        let bytes=object(0x8130000000001234,None);
        let mut input=Vec::new();input.extend((bytes.len() as u32).to_le_bytes());input.extend(bytes);
        let mut output=Vec::new();run(&input[..],&mut output).unwrap();
        assert_eq!(output.iter().filter(|b|**b==b'\n').count(),1);
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap()["type"],"create");
    }
}
