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
fn wilderness_is_grounded_separated_and_uses_real_combat_and_loot() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let content=Content::load(&root.join("wilderness-world-v1.tsv"),Path::new(&data)).unwrap();
    let source:serde_json::Value=serde_json::from_str(include_str!("../../../content/wilderness-content-v1.json")).unwrap();
    let report:serde_json::Value=serde_json::from_str(include_str!("../../../content/wilderness-world-v1.report.json")).unwrap();
    let encounters=Encounters::load(Path::new("training"),&content).unwrap();
    let health=encounters.initial_health().collect::<BTreeMap<_,_>>();
    let ai=creature_ai::catalog().unwrap();
    let native=include_str!("../../../content/wilderness-creature-combat-v1.tsv").lines().filter(|l|l.starts_with("47"))
        .map(|l|{let f=l.split('\t').collect::<Vec<_>>();(u32::from_str_radix(f[0],16).unwrap(),
            (f[1].parse::<u32>().unwrap(),f[6].parse::<u16>().unwrap()))}).collect::<BTreeMap<_,_>>();
    let regions=source["regions"].as_array().unwrap();
    let membership=report["groups"].as_array().unwrap().iter().flat_map(|g|
        g["members"].as_array().unwrap().iter().map(move |id|(id.as_u64().unwrap(),g["region"].as_str().unwrap())))
        .collect::<BTreeMap<_,_>>();
    let mut geometry=Geometry::open(Path::new(&data)).unwrap();
    let installed=root.join("../../data/world/legions-world.tsv");
    let fixed=std::fs::read_to_string(&installed).unwrap_or_default().lines().filter_map(|l| {
        let f=l.split('\t').collect::<Vec<_>>();
        // Later guide-anchored camps were not part of this pack's original
        // 80m exclusion baseline; their own test checks their 6m separation.
        if f.len()!=13 || f[0].starts_with("8140c10000") || f[0].starts_with("8140d30000") {return None;}
        let cell=u32::from_str_radix(f[3],16).ok()?;
        outdoors(cell).then(||xy(cell,[f[4].parse().unwrap(),f[5].parse().unwrap(),f[6].parse().unwrap()]))
    }).collect::<Vec<_>>();
    let mut entities=BTreeSet::new();let mut covered=BTreeSet::new();
    assert!(content.objects().count()>2000);
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
            "Creature outside its wiki wilderness roster");
        let polygon=r["polygon"].as_array().unwrap();
        for (a,b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
            let ax=a[0].as_f64().unwrap();let ay=a[1].as_f64().unwrap();
            let bx=b[0].as_f64().unwrap();let by=b[1].as_f64().unwrap();
            assert!((bx-ax)*(p[1]-ay)-(by-ay)*(p[0]-ax)>=-0.001,"Outside inferred wilderness envelope");
        }
        entities.insert(object.entity);
    }
    assert_eq!(covered.len(),27);assert!(entities.len()>400);
    let world=Content::load(&installed,Path::new(&data)).unwrap();
    let world_encounters=Encounters::load(Path::new("training"),&world).unwrap();
    for quest in pyrealium::hunting_quests::profiles() {
        assert!(world.objects().any(|o|quest.targets.contains(&o.entity) && world_encounters.is_creature(o.id)),
            "No world hunt targets for {}",quest.title);
        let speakers=world.objects().filter(|o|o.entity==quest.npc).collect::<Vec<_>>();
        assert!(!speakers.is_empty(),"Missing hunt giver for {}",quest.title);
        for npc in speakers {assert!(!world_encounters.is_creature(npc.id),"Quest giver is attackable: {}",quest.title);}
    }
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
                    assert_eq!(roll.level,native[&entity].1.clamp(1,150),"Loot level: {entity:08x}");
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
fn wilderness_groups_fill_once_and_refill_after_the_configured_delay() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let content=Content::load(&root.join("wilderness-world-v1.tsv"),Path::new(&data)).unwrap();
    let report:serde_json::Value=serde_json::from_str(include_str!("../../../content/wilderness-world-v1.report.json")).unwrap();
    let mut encounters=Encounters::load(Path::new("training"),&content).unwrap();
    assert_eq!(encounters.spawners().group_count(),report["groups"].as_array().unwrap().len());
    let total=content.objects().count();
    let first=content.objects().next().unwrap().id;
    let dir = pyrealium::db::testdb::fresh();
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
}

#[test]
fn hunting_camps_use_native_ground_loot_and_group_respawns() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let content=Content::load(&root.join("hunting-world-v2.tsv"),Path::new(&data)).unwrap();
    let source:serde_json::Value=serde_json::from_str(include_str!("../../../content/hunting-content-v2.json")).unwrap();
    let report:serde_json::Value=serde_json::from_str(include_str!("../../../content/hunting-world-v2.report.json")).unwrap();
    let world=Content::load(&root.join("../../data/world/legions-world.tsv"),Path::new(&data)).unwrap();
    let mut encounters=Encounters::load(Path::new("training"),&content).unwrap();
    let mut geometry=Geometry::open(Path::new(&data)).unwrap();
    assert_eq!(content.objects().count(),8);assert_eq!(encounters.spawners().len(),6);
    assert_eq!(encounters.spawners().group_count(),3);
    let mut dat=Dat::open(Path::new(&data).join("portal.dat")).unwrap();
    let mut loot=Loot::default();loot.catalog=Catalog::load(&mut dat,&mut BTreeMap::new()).unwrap();
    loot.seed_for_test(715).unwrap();let ai=creature_ai::catalog().unwrap();
    for object in content.objects() {
        let p=object.placement;
        assert!(outdoors(p.cell) && geometry.contact(p) && geometry.dry_ground(p));
        assert!(world.objects().filter(|o|o.id!=object.id).all(|o|p.distance(o.placement).is_none_or(|d|d>=5.999)));
        if encounters.is_creature(object.id) {
            let camp=source["camps"].as_array().unwrap().iter().find(|c| {
                let id=c["id"].as_u64().unwrap();object.id==id || object.id==id+1
            }).unwrap();
            let anchor=[camp["land"][0].as_f64().unwrap(),camp["land"][1].as_f64().unwrap()];
            assert!(distance(xy(p.cell,p.coordinates),anchor)<=32.001);
            assert_eq!(encounters.spawners().policy(object.id).unwrap().regeneration_ms,120000);
            assert!(!ai[&object.entity].loot_proof);
            let mut drops=0;
            for n in 0..20 {
                loot.add(object.id,7,n,object.entity,ai[&object.entity]);
                drops+=loot.get(object.id,7,n).unwrap().items.len();loot.remove(object.id);
            }
            assert!(drops>0,"No loot from guide camp");
        } else {assert!(pyrealium::hunting_quests::supported(object.entity));}
    }
    let dir = pyrealium::db::testdb::fresh();
    {
        let mut store=Store::open(&dir).unwrap();let mut scene=Scene::with_content(content);
        encounters.initialize_spawners(&mut scene,0).unwrap();
        let events=encounters.tick(100,&scene,&mut store,std::iter::empty(),None).unwrap();
        let children=events.iter().filter_map(|e|if let Event::Respawn{actor,..}=e {Some(*actor)}else{None}).collect::<BTreeSet<_>>();
        assert_eq!(children.len(),6);
        for group in report["groups"].as_array().unwrap() {
            assert_eq!(group["members"].as_array().unwrap().iter().filter(|id|children.contains(&id.as_u64().unwrap())).count(),2);
        }
        let first=*children.first().unwrap();
        for id in children {scene.respawn_content(id).unwrap();encounters.spawned(id).unwrap();}
        scene.despawn_content(first).unwrap();encounters.removed(first,1000);
        assert!(!encounters.tick(120900,&scene,&mut store,std::iter::empty(),None).unwrap().iter().any(|e|matches!(e,Event::Respawn{..})));
        let due=encounters.tick(121000,&scene,&mut store,std::iter::empty(),None).unwrap();
        assert_eq!(due.iter().filter_map(|e|if let Event::Respawn{actor,..}=e {Some(*actor)}else{None}).collect::<Vec<_>>(),vec![first]);
    }
}
