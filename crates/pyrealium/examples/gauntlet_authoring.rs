//! Place the Gauntlet's opponent slots: one per fight room and lane (16 levels x 8 lanes), on
//! native floor ahead of the room's arrival point (`content/gauntlet-content-v1.json` rooms, the
//! retail `dng_gauntlet_<level><lane>` destinations). Where retail's opponent stood in a room is
//! not known: Pyrealium policy is the room's widest walkable direction from the arrival point,
//! half that reach and at most 10 m out, facing back towards the arrival.
use pyrealium::{geometry::Geometry,movement::Placement};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::path::Path;

/// Slot identity: the level and lane in the low bits.
pub const SLOT_BASE: u64 = 0x8140_6a75_0000_0000;

fn main()->Result<(),Box<dyn std::error::Error>> {
    let a=std::env::args().skip(1).collect::<Vec<_>>();
    if a.len()!=4{return Err("gauntlet_authoring DAT CONTENT_JSON OUTPUT_TSV REPORT_JSON".into());}
    let bytes=std::fs::read(&a[1])?;let data:Value=serde_json::from_slice(&bytes)?;
    let mut geometry=Geometry::open(Path::new(&a[0]))?;
    let mut lines=vec!["pyrealium-world-v4".to_owned(),
        "# Gauntlet opponent slots (spawn slots the quest rows summon one at a time, quest_emotes Generate). Ground validated; the spot in each room is Pyrealium policy.".into()];
    let mut failures=vec![];let mut included=0;
    for opponent in data["opponents"].as_array().ok_or("opponents")? {
        let level=opponent["level"].as_u64().ok_or("level")?;
        let entity=u32::from_str_radix(opponent["entity"].as_str().ok_or("entity")?,16)?;
        let visual=u32::from_str_radix(opponent["visual"].as_str().ok_or("visual")?,16)?;
        let name=opponent["name"].as_str().ok_or("name")?.replace(['\t','\n','\r']," ");
        for (lane,letter) in "abcdefgh".chars().enumerate() {
            let room=&data["rooms"][format!("{level}{letter}")];
            let cell=u32::from_str_radix(room["cell"].as_str().ok_or("cell")?.trim_start_matches("0x"),16)?;
            let o=room["origin"].as_array().ok_or("origin")?;
            let start=Placement{cell,coordinates:[o[0].as_f64().unwrap() as f32,o[1].as_f64().unwrap() as f32,o[2].as_f64().unwrap() as f32],heading:0};
            let Some(floor)=geometry.ground(start,0.5,1.5) else {failures.push(json!({"room":format!("{level}{letter}"),"reason":"no floor at arrival"}));continue};
            // Walk 1 m steps in each axis direction while the body fits; keep the widest.
            let mut best:Option<(f32,Vec<Placement>,[f32;2])>=None;
            for dir in [[0.,1.],[0.,-1.],[1.,0.],[-1.,0.]] {
                let mut path=vec![floor];let mut at=floor;
                for _ in 0..20 {
                    let Some(next)=at.translated([dir[0],dir[1],0.]) else{break};
                    let Some(step)=geometry.walk_step(at,next,0.5) else{break};
                    path.push(step);at=step;
                }
                let reach=(path.len()-1) as f32;
                if best.as_ref().is_none_or(|(r,_,_)|reach>*r) {best=Some((reach,path,dir));}
            }
            let (reach,path,dir)=best.unwrap();
            let index=((reach/2.).min(10.) as usize).max(0);
            let spot=path[index];
            if reach<4. || !geometry.contact(spot) {failures.push(json!({"room":format!("{level}{letter}"),"reason":"room too narrow","reach":reach}));continue;}
            // Facing back along the walked direction (yaw about z, w and z of the quaternion).
            let yaw=f64::from(-dir[0]).atan2(f64::from(-dir[1]));
            let (w,z)=((-yaw/2.).cos(),(-yaw/2.).sin());
            let id=SLOT_BASE|(level<<8)|lane as u64;
            lines.push(format!("{id:016x}\t{entity:08x}\t{visual:08x}\t{:08x}\t{:.6}\t{:.6}\t{:.6}\t{w:.6}\t0\t0\t{z:.6}\t-\t{name}",spot.cell,spot.coordinates[0],spot.coordinates[1],spot.coordinates[2]));
            included+=1;
        }
    }
    let text=lines.join("\n")+"\n";
    std::fs::write(&a[2],&text)?;
    let report=json!({"inferred":true,"native_ground_verified":failures.is_empty(),"included":included,"rejected":failures,
        "manifest_sha256":format!("{:x}",Sha256::digest(text.as_bytes())),"source_sha256":format!("{:x}",Sha256::digest(&bytes))});
    std::fs::write(&a[3],serde_json::to_string_pretty(&report)?+"\n")?;
    println!("{included} slots, {} rejected",report["rejected"].as_array().unwrap().len());
    Ok(())
}
