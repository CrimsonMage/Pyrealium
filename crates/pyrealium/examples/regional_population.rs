//! Ground inferred regional wilderness without moving recovered placements.
use pyrealium::{geometry::Geometry, map_travel::Landscape, movement::Placement};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

fn xy(p: Placement) -> [f64; 2] {
    [(p.cell >> 24) as f64 * 8. + p.coordinates[0] as f64 / 20.,
     ((p.cell >> 16) & 255) as f64 * 8. + p.coordinates[1] as f64 / 20.]
}
fn point(v: &Value) -> Result<[f64; 2], Box<dyn std::error::Error>> {
    Ok([v[0].as_f64().ok_or("x")?, v[1].as_f64().ok_or("y")?])
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 { (a[0]-b[0]).hypot(a[1]-b[1])*20. }
fn inside(p: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    polygon.len() >= 3 && polygon.iter().zip(polygon.iter().cycle().skip(1)).all(|(a,b)|
        (b[0]-a[0])*(p[1]-a[1])-(b[1]-a[1])*(p[0]-a[0]) >= -1e-8)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a=std::env::args().skip(1).collect::<Vec<_>>();
    if a.len()!=5 {return Err("regional_population DAT JSON OUTPUT_TSV REPORT_JSON EXISTING_WORLD_TSV".into());}
    let bytes=std::fs::read(&a[1])?;
    let data: Value=serde_json::from_slice(&bytes)?;
    let namespace=data["namespace"].as_u64().unwrap_or(0x8140c00000000000);
    if namespace & 0xffffff != 0 {return Err("regional namespace must reserve 24 identity bits".into());}
    let mut land=Landscape::load(Path::new(&a[0]))?;
    let mut geometry=Geometry::open(Path::new(&a[0]))?;
    let towns=data["town_exclusions"].as_array().ok_or("towns")?.iter()
        .map(|t| Ok((point(&t["land"])?,t["radius_m"].as_f64().ok_or("radius")?)))
        .collect::<Result<Vec<_>,Box<dyn std::error::Error>>>()?;
    let mut lines=vec!["pyrealium-world-v4".to_owned(),
        "# Inferred wiki regional wilderness. Native-ground verified; not recovered retail generators.".into()];
    let mut base_lines=Vec::new();
    let mut fixed=Vec::new();
    let mut ids=BTreeSet::new();
    let mut before=BTreeSet::new();
    let existing=std::fs::read_to_string(&a[4])?;
    for line in existing.lines() {
        let f=line.split('\t').collect::<Vec<_>>();
        if f.len()!=13 {continue;}
        let existing_id=u64::from_str_radix(f[0],16)?;
        if existing_id & 0xffffffffff000000 == namespace {continue;}
        let cell=u32::from_str_radix(f[3],16)?;
        if (1..=64).contains(&(cell&65535)) {
            let p=Placement{cell,coordinates:[f[4].parse()?,f[5].parse()?,f[6].parse()?],heading:0};
            fixed.push(xy(p));base_lines.push(line);before.insert(cell>>16);
        }
        ids.insert(u64::from_str_radix(f[0],16)?);
    }
    let mut groups=Vec::new();
    let mut failures=Vec::new();
    let mut homes=Vec::new();
    let mut coverage: BTreeMap<String, BTreeSet<u32>>=BTreeMap::new();
    let mut included=0;
    for entry in data["camps"].as_array().ok_or("camps")? {
        let id=entry["id"].as_u64().ok_or("id")?;
        if id & 0xffffffffff000000 != namespace {return Err("camp outside its reserved namespace".into());}
        let center=point(&entry["land"])?;
        let polygon=entry["polygon"].as_array().ok_or("polygon")?.iter().map(point).collect::<Result<Vec<_>,_>>()?;
        let region=entry["region"].as_str().ok_or("region")?;
        let mut accepted=Vec::new();
        let mut positions: Vec<Placement>=Vec::new();
        for (slot,entity) in entry["roster"].as_array().ok_or("roster")?.iter().enumerate() {
            let entity=entity.as_u64().ok_or("entity")? as u32;
            let mut found=None;
            for attempt in 0..64 {
                let radius=if slot==0 && attempt==0 {0.} else {7.+((attempt+slot*7)%26) as f64};
                let angle=(attempt+slot*17) as f64*2.3999632297;
                let proposed=[center[0]+angle.cos()*radius/20.,center[1]+angle.sin()*radius/20.];
                if !inside(proposed,&polygon) || towns.iter().any(|(t,r)|distance(proposed,*t)<*r)
                    || fixed.iter().any(|p|distance(proposed,*p)<80.)
                    || homes.iter().any(|p|distance(proposed,*p)<80.) {continue;}
                let Ok(candidate)=land.destination(proposed,0) else {continue};
                let Some(p)=geometry.ground(candidate,0.5,1.) else {continue};
                if !(1..=64).contains(&(p.cell&65535)) || !geometry.dry_ground(p) || !geometry.contact(p)
                    || positions.iter().any(|old|old.distance(p).is_some_and(|d|d<12.)) {continue;}
                if ![[0.6,0.,0.],[-0.6,0.,0.],[0.,0.6,0.],[0.,-0.6,0.]].into_iter().all(|offset|
                    p.translated(offset).and_then(|to|geometry.walk_step(p,to,0.35)).is_some()) {continue;}
                if positions.first().is_some_and(|home|!geometry.clear(*home,p,1.)) {continue;}
                found=Some(p);break;
            }
            if let Some(p)=found {
                let child=id+slot as u64;
                if !ids.insert(child) {return Err("regional identity collides with existing content".into());}
                let t=&data["templates"][entity.to_string()];
                let visual=t["visual"].as_u64().ok_or("visual")?;
                let name=t["name"].as_str().ok_or("name")?.replace(['\t','\n','\r']," ");
                // Stable heading avoids every creature facing the same compass point.
                let angle=((id ^ (id>>17) ^ slot as u64)%360) as f64*std::f64::consts::PI/180.;
                lines.push(format!("{child:016x}\t{entity:08x}\t{visual:08x}\t{:08x}\t{:.6}\t{:.6}\t{:.6}\t{:.7}\t0\t0\t{:.7}\t-\t{name}",
                    p.cell,p.coordinates[0],p.coordinates[1],p.coordinates[2],(angle/2.).cos(),(angle/2.).sin()));
                accepted.push(child);positions.push(p);included+=1;
                coverage.entry(region.into()).or_default().insert(p.cell>>16);
            } else {
                failures.push(json!({"group":id,"entity":entity,"slot":slot,
                    "reason":"No dry reachable native ground within region and 32m search; town/content separation enforced"}));
                if slot==0 {break;}
            }
        }
        if !accepted.is_empty() {
            homes.push(xy(positions[0]));
            groups.push(json!({"id":id,"capacity":entry["capacity"].as_u64().ok_or("capacity")?.min(accepted.len() as u64),
                "members":accepted,"regeneration_ms":entry["regeneration_ms"],"region":region}));
        }
        if (groups.len()+failures.len())%100==0 {println!("Grounded {included} regional creatures...");}
    }
    let added_blocks=coverage.values().flatten().copied().collect::<BTreeSet<_>>();
    let text=lines.join("\n")+"\n";
    std::fs::write(&a[2],&text)?;
    std::fs::write(&a[3],serde_json::to_string_pretty(&json!({"version":1,"inferred":true,"native_ground_verified":true,
        "source_sha256":format!("{:x}",Sha256::digest(&bytes)),
        "manifest_sha256":format!("{:x}",Sha256::digest(text.as_bytes())),
        "existing_outdoor_sha256":format!("{:x}",Sha256::digest(base_lines.join("\n").as_bytes())),
        "groups":groups,"rejected":failures,"included":included,"regions":coverage,
        "outdoor_blocks_before":before.len(),"newly_populated_outdoor_blocks":added_blocks.difference(&before).count(),
        "outdoor_blocks_after":before.union(&added_blocks).count()}))?)?;
    println!("Regional population: {included} grounded placements, {} spawner groups, {} rejected slots, {} new outdoor blocks",
        groups.len(),failures.len(),added_blocks.difference(&before).count());
    Ok(())
}
