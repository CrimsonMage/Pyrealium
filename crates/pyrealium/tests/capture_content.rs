use pyrealium::{content::Content, encounters::Encounters, services::Service};
use std::path::Path;

#[test]
fn audited_capture_objects_load_with_native_services_and_encounter_profiles() {
    let Ok(data) = std::env::var("AC2_TEST_GAME_DATA") else { return };
    let path=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/capture-spawns-audited.tsv");
    let content=Content::load(&path,Path::new(&data)).unwrap();
    assert!(content.len()>100);
    let mut vendors=0;let mut doors=0;
    for object in content.objects() {
        assert_eq!(&object.spawn[..4],&0x10058u32.to_le_bytes());
        assert_eq!(u64::from_le_bytes(object.spawn[4..12].try_into().unwrap()),object.id);
        assert!(object.placement.valid());
        match object.service {
            Some(Service::Vendor)=>vendors+=1,
            Some(Service::Door{..})=>{doors+=1;assert!(object.updates().next().is_some());},
            _=>{},
        }
    }
    assert!(vendors>=10);assert!(doors>=2);
    let encounters=Encounters::load(Path::new("training"),&content).unwrap();
    assert!(encounters.initial_health().count()>=20);
    assert!(content.objects().any(|o|o.name.as_deref()==Some("Quick Exit to Lost Wish")));
}

#[test]
fn town_npcs_are_not_promoted_to_encounters_by_the_old_catalog() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    // Retain exact source rows; no production store or generated pack required.
    let sources=[include_str!("../../../content/world-observed-expanded.tsv"),include_str!("../../../content/capture-spawns-audited.tsv"),include_str!("../../../content/training-world.tsv")];
    let mut text=String::from("pyrealium-world-v3\n");
    for id in ["8140dc346c4fa355","81409bec26cb5af8","8140b739fea6ec9a",
        "814000000003000a","8140000000030040","8140000000030041","8140000000030042","8140000000030044"] {
        let row=sources.iter().flat_map(|s|s.lines()).find(|l|l.starts_with(id)).unwrap();
        let mut fields=row.split('\t').take(12).collect::<Vec<_>>();
        if fields.len()==11{fields.push("-");}
        text.push_str(&fields.join("\t"));text.push('\n');
    }
    let path=std::env::temp_dir().join(format!("ac2-encounter-regression-{}-{}.tsv",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::write(&path,text).unwrap();
    let loaded=Content::load(&path,Path::new(&data));std::fs::remove_file(&path).unwrap();
    let content=loaded.unwrap();
    let encounters=Encounters::load(Path::new("training"),&content).unwrap();
    let orven=content.objects().find(|o|o.entity==0x47001c27).expect("Captured Orven");
    assert!(!encounters.is_creature(orven.id),"Orven has no recovered combat profile");
    // The Shelter quest's NPCs and props (guard, lieutenant, advance scout, scout, sentry, the
    // training stones 0x47001573..=0x4700157C, the Menhir Shard), as `shelter::supported` listed
    // them before the quest moved to the quest tables.
    let shelter=|entity:u32|matches!(entity,0x4700157d|0x4700157e|0x4700157f|0x47001580|0x47001581|0x47002448|0x47001573..=0x4700157c);
    for object in content.objects().filter(|o|o.entity==pyrealium::training_dialogue::MENTOR_ENTITY || shelter(o.entity)) {
        assert!(!encounters.is_creature(object.id),"A configured quest guide must not become an encounter");
    }
    assert!(encounters.is_creature(0x81409bec26cb5af8));
    assert!(encounters.is_creature(0x8140b739fea6ec9a));
    assert!(encounters.initial_health().all(|(id,_)|content.objects().find(|o|o.id==id).unwrap().service.is_none()));
    let outdoors=encounters.initial_health().filter(|(id,_)|content.objects().find(|o|o.id==*id).is_some_and(|o|(1..=64).contains(&(o.placement.cell&65535)))).count();
    assert_eq!(outdoors,2);
}
