//! Native assets, placement safety, and NPC combat protection for wiki recovery.
use pyrealium::{content::Content, encounters::Encounters, geometry::Geometry, npc_dialogue};
use std::{collections::BTreeSet, path::Path};

#[test]
fn wiki_npcs_are_grounded_unique_and_cannot_be_killed_for_loot() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
    let world=Content::load(&root.join("../../data/world/legions-world.tsv"),Path::new(&data)).unwrap();
    let content=Content::load(&root.join("npc-world-v1.tsv"),Path::new(&data)).unwrap();
    let source:serde_json::Value=serde_json::from_str(include_str!("../content/npc-dialogue-v1.json")).unwrap();
    let encounters=Encounters::load(Path::new("training"),&world).unwrap();
    let mut geometry=Geometry::open(Path::new(&data)).unwrap();
    assert_eq!(content.objects().count(),27);
    let mut identities=BTreeSet::new();
    for npc in content.objects() {
        assert!(identities.insert(npc.entity));
        assert!(npc_dialogue::profiles().contains_key(&npc.entity));
        assert!(!encounters.is_creature(npc.id));
        assert!(encounters.spawners().policy(npc.id).is_none());
        assert!(geometry.contact(npc.placement) && geometry.dry_ground(npc.placement));
        assert_eq!(world.objects().filter(|o|o.entity==npc.entity).count(),1,"Duplicate {:?}",npc.name);
        assert!(world.objects().filter(|o|o.id!=npc.id).all(|o|
            npc.placement.distance(o.placement).is_none_or(|d|d>=5.999)));
        let anchor=source["npcs"].as_array().unwrap().iter().find(|n|n["id"].as_u64()==Some(npc.id)).unwrap();
        let p=npc.placement;
        let x=(p.cell>>24) as f64*160.+p.coordinates[0] as f64;
        let y=((p.cell>>16)&255) as f64*160.+p.coordinates[1] as f64;
        assert!((x-anchor["land"][0].as_f64().unwrap()*20.).hypot(y-anchor["land"][1].as_f64().unwrap()*20.)<=32.001);
    }
    // Every catalog identity is checked against the DAT combat classifier,
    // including variants withheld from the permanent world (spirits/events).
    let path=std::env::temp_dir().join(format!("ac2-npc-identities-{}.tsv",std::process::id()));
    let mut rows=String::from("pyrealium-world-v4\n");
    for (i,p) in source["profiles"].as_array().unwrap().iter().enumerate() {
        let entity=p["entity"].as_u64().unwrap();
        let x=20.*(1+(i/60)) as f32+1.;
        let cell=0x77b80000+((x/20.) as u32*8)+1;
        rows+=&format!("{:016x}\t{entity:08x}\t{:08x}\t{cell:08x}\t{x}\t1\t180\t1\t0\t0\t0\t-\t{}\n",
            0x8140d50000000000u64+entity,p["visual"].as_u64().unwrap(),p["name"].as_str().unwrap());
    }
    // Encounters requires at least one combat actor. Include an independently
    // classified production creature as the positive control.
    let monster=world.objects().find(|o|encounters.is_creature(o.id)).unwrap();
    rows+=&format!("{:016x}\t{:08x}\t{:08x}\t{:08x}\t{}\t{}\t{}\t1\t0\t0\t0\t-\tControl\n",
        monster.id,monster.entity,monster.visual,monster.placement.cell,
        monster.placement.coordinates[0],monster.placement.coordinates[1],monster.placement.coordinates[2]);
    std::fs::write(&path,rows).unwrap();
    let catalog=Content::load(&path,Path::new(&data)).unwrap();
    let runtime=Encounters::load(Path::new("training"),&catalog).unwrap();
    assert!(runtime.is_creature(monster.id));
    for npc in catalog.objects().filter(|o|o.id!=monster.id) {
        assert!(!runtime.is_creature(npc.id),"Conversation on hostile {:?}",npc.name);
    }
    std::fs::remove_file(path).unwrap();
}
