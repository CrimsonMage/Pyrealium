//! Resolve inferred guide camps onto native ground. Source anchors remain in JSON.
use pyrealium::{geometry::Geometry,map_travel::Landscape,movement::Placement};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{collections::BTreeSet,path::Path};
fn main()->Result<(),Box<dyn std::error::Error>> {
    let a=std::env::args().skip(1).collect::<Vec<_>>();
    if a.len()!=5{return Err("population_authoring DAT JSON OUTPUT_TSV REPORT_JSON EXISTING_WORLD_TSV".into());}
    let bytes=std::fs::read(&a[1])?;let data:Value=serde_json::from_slice(&bytes)?;
    let mut own_ids: BTreeSet<u64> = data["npcs"].as_array().ok_or("npcs")?.iter()
        .filter_map(|row|row["id"].as_u64()).collect();
    for camp in data["camps"].as_array().ok_or("camps")? {
        let id=camp["id"].as_u64().ok_or("camp id")?;
        for slot in 0..camp["roster"].as_array().ok_or("roster")?.len() {
            own_ids.insert(id.checked_add(slot as u64).ok_or("camp identity overflow")?);
        }
    }
    if let Some(interiors)=data["interiors"].as_array() {
        own_ids.extend(interiors.iter().filter_map(|row|row["id"].as_u64()));
    }
    let mut land=Landscape::load(Path::new(&a[0]))?;let mut geometry=Geometry::open(Path::new(&a[0]))?;
    let mut lines=vec!["pyrealium-world-v4".to_owned(),"# Inferred guide placements. Ground/collision validated; not exact retail coordinates.".into()];
    let mut groups=vec![];let mut failures=vec![];let mut occupied:Vec<Placement>=vec![];let mut ids=BTreeSet::new();
    // Existing captures/services take priority. Ignore this pack's previous
    // output so regeneration is deterministic against the same source world.
    let existing=std::fs::read_to_string(&a[4])?;
    let mut base_lines=Vec::new();
    for line in existing.lines() {
        let f=line.split('\t').collect::<Vec<_>>();
        if f.len()<7 || !f[0].starts_with("8140") {continue;}
        if own_ids.contains(&u64::from_str_radix(f[0],16)?) {continue;}
        let cell=u32::from_str_radix(f[3],16)?;
        if (1..=64).contains(&(cell&65535)) || cell&65535>=256 {
            occupied.push(Placement{cell,coordinates:[f[4].parse()?,f[5].parse()?,f[6].parse()?],heading:0});
            if cell&65535<256 {base_lines.push(line);}
        }
    }
    let mut append=|id:u64,entity:u32,p:Placement|->Result<(),Box<dyn std::error::Error>> {
        if !ids.insert(id){return Err("inferred placement identity collision".into());}
        let t=&data["templates"][entity.to_string()];let visual=t["visual"].as_u64().ok_or("visual")?;
        let name=t["name"].as_str().ok_or("name")?.replace(['\t','\n','\r']," ");
        lines.push(format!("{id:016x}\t{entity:08x}\t{visual:08x}\t{:08x}\t{:.6}\t{:.6}\t{:.6}\t1\t0\t0\t0\t-\t{name}",p.cell,p.coordinates[0],p.coordinates[1],p.coordinates[2]));Ok(())
    };
    for (kind,entries) in [("camp",&data["camps"]),("npc",&data["npcs"])] {
        for entry in entries.as_array().ok_or("entries")? {
            let id=entry["id"].as_u64().ok_or("id")?;
            let center=[entry["land"][0].as_f64().ok_or("x")?,entry["land"][1].as_f64().ok_or("y")?];
            let roster=if kind=="camp" {entry["roster"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as u32).collect::<Vec<_>>()} else {vec![entry["entity"].as_u64().unwrap() as u32]};
            let mut accepted=vec![];let mut origin=None;
            for (slot,entity) in roster.into_iter().enumerate() {
                let mut found=None;
                for attempt in 0..64 {
                    // Deterministic golden-angle search, <=32m from the coarse
                    // guide point. Native terrain rejects holes and steep faces.
                    let radius=if slot==0 && attempt==0 {0.}else{7.+((attempt+slot*7)%26) as f64};
                    let angle=(attempt+slot*17) as f64*2.3999632297;
                    let xy=[center[0]+angle.cos()*radius/20.,center[1]+angle.sin()*radius/20.];
                    let Ok(candidate)=land.destination(xy,0) else{continue};
                    let Some(p)=geometry.ground(candidate,0.5,1.0) else{continue};
                    if p.cell&65535>64 || !geometry.dry_ground(p) || occupied.iter().any(|old|old.distance(p).is_some_and(|d|d<6.)) || !geometry.contact(p){continue;}
                    // Require a walkable patch and a clear route through the camp.
                    if ![[0.6,0.,0.],[-0.6,0.,0.],[0.,0.6,0.],[0.,-0.6,0.]].into_iter().all(|offset|
                        p.translated(offset).and_then(|to|geometry.walk_step(p,to,0.35)).is_some()) {continue;}
                    if origin.is_some_and(|home|!geometry.clear(home,p,1.0)){continue;}
                    found=Some(p);break;
                }
                if let Some(p)=found {
                    origin.get_or_insert(p);occupied.push(p);let child=id+slot as u64;append(child,entity,p)?;accepted.push(child);
                } else {
                    failures.push(json!({"group":id,"entity":entity,"slot":slot,"reason":"no reachable walkable native ground within 32m"}));
                    // A target-only camp cannot bestow its own quest.
                    if slot==0 {break;}
                }
            }
            if kind=="camp" && !accepted.is_empty() {
                groups.push(json!({"id":id,"capacity":entry["capacity"].as_u64().unwrap().min(accepted.len() as u64),"members":accepted,"regeneration_ms":120000}));
            }
        }
    }
    // A captured interior frame is a candidate, never an excuse to place a
    // monster through a wall. Do not silently relocate rejected room anchors.
    if let Some(interiors)=data["interiors"].as_array() {
        for entry in interiors {
            let id=entry["id"].as_u64().ok_or("interior id")?;
            let entity=entry["entity"].as_u64().ok_or("interior entity")? as u32;
            let candidate=Placement{cell:entry["cell"].as_u64().ok_or("interior cell")? as u32,
                coordinates:[0,1,2].map(|i|entry["position"][i].as_f64().unwrap_or(f64::NAN) as f32),heading:0};
            let point=geometry.ground(candidate,0.5,1.0).filter(|p|p.cell&65535>=256 &&
                p.distance(candidate).is_some_and(|d|d<=1.) && geometry.contact(*p) &&
                !occupied.iter().any(|old|old.distance(*p).is_some_and(|d|d<6.)) &&
                [[0.6,0.,0.],[-0.6,0.,0.],[0.,0.6,0.],[0.,-0.6,0.]].into_iter().all(|offset|
                    p.translated(offset).and_then(|to|geometry.walk_step(*p,to,0.35)).is_some()));
            if let Some(p)=point {append(id,entity,p)?;occupied.push(p);}
            else {failures.push(json!({"id":id,"entity":entity,"reason":"captured interior anchor has no clear native walking patch"}));}
        }
    }
    if let Some(interior_groups)=data["interior_groups"].as_array() {
        for group in interior_groups {
            let members=group["members"].as_array().ok_or("interior group members")?.iter()
                .filter_map(Value::as_u64).filter(|id|ids.contains(id)).collect::<Vec<_>>();
            if !members.is_empty() {groups.push(json!({"id":group["id"],"capacity":members.len(),"members":members}));}
        }
    }
    let text=lines.join("\n")+"\n";std::fs::write(&a[2],&text)?;
    std::fs::write(&a[3],serde_json::to_string_pretty(&json!({"version":1,"inferred":true,"native_ground_verified":true,
        "source_sha256":format!("{:x}",Sha256::digest(&bytes)),"manifest_sha256":format!("{:x}",Sha256::digest(text.as_bytes())),
        "existing_outdoor_sha256":format!("{:x}",Sha256::digest(base_lines.join("\n").as_bytes())),
        "groups":groups,"rejected":failures,"included":ids.len()}))?)?;
    println!("Inferred population: {} grounded placements, {} spawner groups, {} rejected slots",ids.len(),groups.len(),failures.len());Ok(())
}
