//! Acceptance for the explicitly inferred content pack, not retail placement proof.
use pyrealium::{content::Content,encounters::{Encounters,Event},geometry::Geometry,scene::Scene,store::Store};
use std::{collections::BTreeSet,path::Path};

#[test]
fn inferred_camps_are_grounded_capacity_limited_and_never_promote_services() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else{return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let content=Content::load(&root.join("inferred-world-v1.tsv"),Path::new(&data)).unwrap();
    let report:serde_json::Value=serde_json::from_str(include_str!("../../../content/inferred-world-v1.report.json")).unwrap();
    let mut encounters=Encounters::load(Path::new("training"),&content).unwrap();
    let members=report["groups"].as_array().unwrap().iter().flat_map(|g|g["members"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap())).collect::<BTreeSet<_>>();
    let capacity=report["groups"].as_array().unwrap().iter().map(|g|g["capacity"].as_u64().unwrap() as usize).sum::<usize>();
    let mut geometry=Geometry::open(Path::new(&data)).unwrap();let mut npcs=0;
    for object in content.objects() {
        assert!(geometry.contact(object.placement),"{} is not grounded",object.id);
        assert!(geometry.dry_ground(object.placement),"{} is underwater",object.id);
        assert_eq!(encounters.is_creature(object.id),members.contains(&object.id),"Wrong combat role: {:?}",object.name);
        if !members.contains(&object.id){npcs+=1;}
    }
    assert!(npcs>=20);assert!(members.len()>600);
    let dir = pyrealium::db::testdb::fresh();
    {
        let mut store=Store::open(&dir).unwrap();let mut scene=Scene::with_content(content);
        encounters.initialize_spawners(&mut scene,0).unwrap();
        let events=encounters.tick(100,&scene,&mut store,std::iter::empty(),None).unwrap();
        let children=events.iter().filter_map(|e|if let Event::Respawn{actor,..}=e{Some(*actor)}else{None}).collect::<BTreeSet<_>>();
        assert_eq!(children.len(),capacity);
        for group in report["groups"].as_array().unwrap() {
            let group_members=group["members"].as_array().unwrap();
            assert_eq!(group_members.iter().filter(|id|children.contains(&id.as_u64().unwrap())).count(),group["capacity"].as_u64().unwrap() as usize);
            assert!(children.contains(&group_members[0].as_u64().unwrap()),"Each initial camp must contain its quest starter");
        }
        for id in children {encounters.spawned(id).unwrap();}
        assert_eq!(encounters.spawners().live(),capacity);
        assert!(!encounters.tick(1000,&scene,&mut store,std::iter::empty(),None).unwrap().iter().any(|e|matches!(e,Event::Respawn{..})));
    }
}
