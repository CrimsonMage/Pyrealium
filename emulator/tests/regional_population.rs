//! Acceptance for inferred wilderness; source coordinates are not retail proof.
use pyrealium::{content::Content, creature_ai, dat::Dat, encounters::{Encounters, Event},
    geometry::Geometry, loot::{Catalog, Loot}, scene::Scene, store::Store};
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

fn outdoors(cell:u32)->bool {(1..=64).contains(&(cell&65535))}
fn xy(cell:u32,p:[f32;3])->[f64;2] {
    [(cell>>24) as f64*8.+p[0] as f64/20.,((cell>>16)&255) as f64*8.+p[1] as f64/20.]
}
fn distance(a:[f64;2],b:[f64;2])->f64 {(a[0]-b[0]).hypot(a[1]-b[1])*20.}

#[test]
fn regional_wilderness_is_grounded_separated_and_uses_real_combat_and_loot() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
    let content=Content::load(&root.join("regional-world-v1.tsv"),Path::new(&data)).unwrap();
    let source:serde_json::Value=serde_json::from_str(include_str!("../content/regional-content-v1.json")).unwrap();
    let report:serde_json::Value=serde_json::from_str(include_str!("../content/regional-world-v1.report.json")).unwrap();
    let encounters=Encounters::load(Path::new("training"),&content).unwrap();
    let health=encounters.initial_health().collect::<BTreeMap<_,_>>();
    let ai=creature_ai::catalog().unwrap();
    let native=include_str!("../content/regional-creature-combat-v1.tsv").lines().filter(|l|l.starts_with("47"))
        .map(|l|{let f=l.split('\t').collect::<Vec<_>>();(u32::from_str_radix(f[0],16).unwrap(),
            (f[1].parse::<u32>().unwrap(),f[6].parse::<u16>().unwrap()))}).collect::<BTreeMap<_,_>>();
    let regions=source["regions"].as_array().unwrap();
    let membership=report["groups"].as_array().unwrap().iter().flat_map(|g|
        g["members"].as_array().unwrap().iter().map(move |id|(id.as_u64().unwrap(),g["region"].as_str().unwrap())))
        .collect::<BTreeMap<_,_>>();
    let mut geometry=Geometry::open(Path::new(&data)).unwrap();
    let installed=root.join("../../data/world/legions-world.tsv");
    let fixed=std::fs::read_to_string(installed).unwrap_or_default().lines().filter_map(|l| {
        let f=l.split('\t').collect::<Vec<_>>();
        if f.len()!=13 || f[0].starts_with("8140c00000") {return None;}
        let cell=u32::from_str_radix(f[3],16).ok()?;
        outdoors(cell).then(||xy(cell,[f[4].parse().unwrap(),f[5].parse().unwrap(),f[6].parse().unwrap()]))
    }).collect::<Vec<_>>();
    let mut entities=BTreeSet::new();let mut covered=BTreeSet::new();
    assert!(content.objects().count()>3000);
    for object in content.objects() {
        assert!(encounters.is_creature(object.id),"Missing combat role: {:?}",object.name);
        assert_eq!(health[&object.id],native[&object.entity].0);
        assert!(outdoors(object.placement.cell));
        assert!(geometry.contact(object.placement),"Not on ground: {:x}",object.id);
        assert!(geometry.dry_ground(object.placement),"Underwater: {:x}",object.id);
        assert!(!ai[&object.entity].loot_proof);
        let p=xy(object.placement.cell,object.placement.coordinates);
        assert!(fixed.iter().all(|old|distance(p,*old)>=79.999),"Encroaches on an existing actor: {:x}",object.id);
        for town in source["town_exclusions"].as_array().unwrap() {
            let t=[town["land"][0].as_f64().unwrap(),town["land"][1].as_f64().unwrap()];
            assert!(distance(p,t)>=town["radius_m"].as_f64().unwrap()-0.001,"In town: {:?}",town["name"]);
        }
        let name=membership[&object.id];covered.insert(name);
        let r=regions.iter().find(|r|r["name"]==name).unwrap();
        assert!(r["creatures"].as_array().unwrap().iter().any(|c|c["entity"].as_u64()==Some(object.entity as u64)),
            "Creature outside its wiki regional roster");
        let polygon=r["polygon"].as_array().unwrap();
        for (a,b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
            let ax=a[0].as_f64().unwrap();let ay=a[1].as_f64().unwrap();
            let bx=b[0].as_f64().unwrap();let by=b[1].as_f64().unwrap();
            assert!((bx-ax)*(p[1]-ay)-(by-ay)*(p[0]-ax)>=-0.001,"Outside inferred regional envelope");
        }
        entities.insert(object.entity);
    }
    assert_eq!(covered.len(),21);assert!(entities.len()>250);
    // Exercise the actual loot pipeline for every placed creature, including
    // templates absent from the original capture combat catalog.
    let mut dat=Dat::open(Path::new(&data).join("portal.dat")).unwrap();
    let mut metadata=BTreeMap::new();
    let mut loot=Loot::default();loot.catalog=Catalog::load(&mut dat,&mut metadata).unwrap();
    loot.seed_for_test(947).unwrap();
    for entity in entities {
        let mut count=0;let mut variants=BTreeSet::new();
        for n in 0..24 {
            loot.add(21,7,n,entity,ai[&entity]);
            let reward=loot.get(21,7,n).unwrap();
            count+=reward.items.len();
            variants.insert(reward.items.iter().map(|i|i.definition).collect::<Vec<_>>());
            for item in &reward.items {
                if let Some(roll)=item.roll {
                    assert_eq!(roll.level,native[&entity].1);
                    if pyrealium::treasure_weapons::profile(item).is_some() {
                        assert!(pyrealium::treasure_weapons::valid_finish(item));
                        assert!(roll.damage>0);
                    } else {assert!(roll.armor>0);}
                }
            }
            loot.remove(21);
        }
        assert!(count>0,"No supported loot: {entity:08x}");
        assert!(variants.len()>1,"Fixed loot bundle: {entity:08x}");
    }
}

#[test]
fn regional_groups_fill_once_and_refill_after_the_configured_delay() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
    let content=Content::load(&root.join("regional-world-v1.tsv"),Path::new(&data)).unwrap();
    let report:serde_json::Value=serde_json::from_str(include_str!("../content/regional-world-v1.report.json")).unwrap();
    let mut encounters=Encounters::load(Path::new("training"),&content).unwrap();
    assert_eq!(encounters.spawners().group_count(),report["groups"].as_array().unwrap().len());
    let total=content.objects().count();
    let first=content.objects().next().unwrap().id;
    let dir=std::env::temp_dir().join(format!("ac2-regional-{}",std::process::id()));
    {
        let mut store=Store::open(&dir).unwrap();let mut scene=Scene::with_content(content);
        encounters.initialize_spawners(&mut scene,0).unwrap();
        let events=encounters.tick(100,&scene,&mut store,std::iter::empty(),None).unwrap();
        let children=events.iter().filter_map(|e|if let Event::Respawn{actor,..}=e {Some(*actor)}else{None})
            .collect::<BTreeSet<_>>();
        assert_eq!(children.len(),total);
        for group in report["groups"].as_array().unwrap() {
            assert_eq!(group["regeneration_ms"],180000);
            assert_eq!(group["members"].as_array().unwrap().iter().filter(|id|children.contains(&id.as_u64().unwrap())).count(),
                group["capacity"].as_u64().unwrap() as usize);
        }
        for id in children {scene.respawn_content(id).unwrap();encounters.spawned(id).unwrap();}
        assert_eq!(encounters.spawners().live(),total);
        scene.despawn_content(first).unwrap();encounters.removed(first,1000);
        let early=encounters.tick(180900,&scene,&mut store,std::iter::empty(),None).unwrap();
        assert!(!early.iter().any(|e|matches!(e,Event::Respawn{..})));
        let due=encounters.tick(181000,&scene,&mut store,std::iter::empty(),None).unwrap();
        assert_eq!(due.iter().filter_map(|e|if let Event::Respawn{actor,..}=e {Some(*actor)}else{None}).collect::<Vec<_>>(),vec![first]);
        scene.respawn_content(first).unwrap();encounters.spawned(first).unwrap();
        assert_eq!(encounters.spawners().live(),total);
    }
    std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
}
