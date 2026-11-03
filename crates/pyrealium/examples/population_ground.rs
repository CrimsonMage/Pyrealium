//! Read-only native ground validation of proposed captured population.
use pyrealium::{content::Content, geometry::Geometry};
use sha2::{Digest,Sha256};
use std::{collections::BTreeMap,path::Path};
fn main()->Result<(),Box<dyn std::error::Error>> {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args.len()!=3 {return Err("Usage: population_ground DAT_DIRECTORY CANDIDATES_TSV REPORT_JSON".into());}
    let content=Content::load(Path::new(&args[1]),Path::new(&args[0]))?;
    let text=std::fs::read_to_string(&args[1])?;
    let rows=text.lines().filter_map(|l|u64::from_str_radix(l.split('\t').next()?,16).ok().map(|id|(id,l))).collect::<BTreeMap<_,_>>();
    let mut objects=content.objects().collect::<Vec<_>>();objects.sort_by_key(|o|o.placement.cell);
    let mut geometry=Geometry::open(Path::new(&args[0]))?;let mut accepted=BTreeMap::new();let mut rejected=0;
    for object in objects {
        let p=object.placement;
        if geometry.contact(p) {
            accepted.insert(format!("{:016x}",object.id),format!("{:x}",Sha256::digest(rows[&object.id].as_bytes())));
        } else {rejected+=1;}
    }
    std::fs::write(&args[2],serde_json::to_string_pretty(&accepted)?)?;
    println!("Native ground: {} accepted, {rejected} withheld; positions unchanged",accepted.len());Ok(())
}
