use pyrealium::{content::Content,encounters::Encounters,services::Service};
use std::path::Path;

#[test]
fn translated_population_uses_native_combat_and_protects_services() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else{return};
    let root=pyrealium::content_db::checkout::repository();
    let content=Content::load(&root.join("recovered-population.tsv"),Path::new(&data)).unwrap();
    let proof:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(root.join("recovered-population.report.json")).unwrap()).unwrap();
    let encounters=Encounters::load(Path::new("training"),&content).unwrap();
    let mut monsters=0;let mut protected=0;let mut vendors=0;
    for a in proof["actors"].as_array().unwrap() {
        let id=u64::from_str_radix(a["id"].as_str().unwrap(),16).unwrap();
        let object=content.objects().find(|o|o.id==id).unwrap();
        assert_eq!(object.entity as u64,a["entity"].as_u64().unwrap());
        assert_eq!(object.name.as_deref(),a["name"].as_str());
        if a["role"]=="monster" {assert!(encounters.is_creature(id),"Missing combat for {}",a["name"]);monsters+=1;}
        else {assert!(!encounters.is_creature(id),"Noncombat actor {} was attackable",a["name"]);protected+=1;}
        assert_eq!(encounters.spawners().owns(id),a["role"]=="monster",
            "Only verified combat placements may own population slots: {}",a["name"]);
        if a["role"]=="vendor" {assert_eq!(object.service,Some(Service::Vendor));vendors+=1;}
    }
    assert!(monsters>3500);assert!(protected>200);assert!(vendors>=20);
    let mut encounters=encounters;
    let services=content.objects().filter(|o|!encounters.is_creature(o.id)).map(|o|(o.id,o.placement)).collect::<Vec<_>>();
    let homes=content.objects().filter(|o|encounters.is_creature(o.id)).map(|o|(o.id,o.placement)).collect::<Vec<_>>();
    let mut scene=pyrealium::scene::Scene::with_content(content);
    encounters.initialize_spawners(&mut scene,0).unwrap();
    assert_eq!(encounters.spawners().len(),monsters);
    assert_eq!(encounters.spawners().live(),0);
    for (id,home) in services {assert_eq!(scene.content_placement(id),Some(home));}
    for (id,_) in homes {assert!(scene.content_placement(id).is_none(),"Initial monsters must be created by their spawners");}
}

#[test]
fn recovered_cutter_has_native_ground_and_a_render_observer_position() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else{return};
    let mut geometry=pyrealium::geometry::Geometry::open(Path::new(&data)).unwrap();
    let home=pyrealium::movement::Placement{cell:0x78b8002e,
        coordinates:[104.823303,116.561798,122.845955],heading:0};
    assert!(geometry.contact(home));
    let mut view=home;view.coordinates[0]+=8.;
    let floor=geometry.ground(view,10.,15.).expect("Observer floor beside captured beetle");
    assert!(geometry.contact(floor));
    assert!(geometry.clear(home,floor,1.5),"Observer must have line of sight to the captured creature");
    assert!((floor.coordinates[2]-124.722275).abs()<0.01);
}
