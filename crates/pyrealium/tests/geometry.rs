use pyrealium::{falling::Fall,geometry::Geometry,movement::Placement};

#[test]
fn repaired_lake_bank_uses_adjacent_native_water_and_rejects_submerged_spawns() {
    let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else{return};
    let world=Geometry::open(std::path::Path::new(&path)).unwrap();
    let height=world.water_height(0x74a90008).unwrap().unwrap();
    assert!((height-96.7).abs()<0.001);
    assert_eq!(Some(height),world.water_height(0x75a90001).unwrap());
    let p=Placement{cell:0x74a90008,coordinates:[55.5,155.4,93.9],heading:0};
    assert!(!world.dry_ground(p));
    assert!(world.dry_ground(Placement{coordinates:[55.5,155.4,100.],..p}));
    assert_eq!(None,world.water_height(0x73a90008).unwrap(),"Do not flood unrelated dry blocks");
}

#[test]
fn native_geometry_verifies_dungeon_floor_and_blocks_walls() {
    let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let mut world=Geometry::open(std::path::Path::new(&path)).unwrap();
    let p=Placement::HUMAN_START;
    let floor=world.ground(p,2.,5.).expect("Native Shelter floor");
    assert!(world.contact(floor));
    let mut above=floor;above.coordinates[2]+=3.;assert!(!world.contact(above));
    let mut nearby=floor;nearby.coordinates[0]+=0.5;
    assert!(world.clear(floor,nearby,1.));
    let mut beyond=floor;beyond.coordinates[0]+=100.;
    assert!(!world.clear(floor,beyond,1.),"A ray must not pass through the Shelter wall");
    let idol=Placement{coordinates:[58.,243.,-45.010002],..p};
    let mut next=idol;next.coordinates[1]-=0.2;
    assert!(world.walk_step(idol,next,0.35).is_some(),"Authored tutorial pursuit has a verified walkable step");
    let bad=Placement{coordinates:[63.060577,239.199,-44.606205],..p};
    assert!(world.ground(bad,2.,5.).is_none(),"The old authored spawn is outside the room floor");
}

#[test]
fn native_walking_crosses_outdoor_cell_and_landblock_boundaries() {
    let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let mut world=Geometry::open(std::path::Path::new(&path)).unwrap();
    // Probe both 20 m subcell edges and the 160 m block edge near Cragstone.
    // Only DAT-confirmed floors qualify; this adds no world placements.
    for boundary in [20.,160.] {
        let mut walked=0;
        for y in (10..160).step_by(20) {
            let raw=Placement{cell:0x76b80001,coordinates:[boundary-0.1,y as f32,200.],heading:0}.translated([0.;3]).unwrap();
            let Some(from)=world.ground(raw,300.,500.) else {continue;};
            let to=from.translated([0.2,0.,0.]).unwrap();
            let Some(next)=world.walk_step(from,to,0.35) else {continue;};
            assert_ne!(from.cell,next.cell);assert!(world.contact(next));
            assert!((from.offset_to(next).unwrap()[0]-0.2).abs()<0.001);
            let back=world.walk_step(next,from,0.35).expect("Reverse walking step");
            assert_eq!(back.cell,from.cell);assert!(from.distance(back).unwrap()<0.01);
            assert!(world.walk_step(from,from.translated([2.,0.,0.]).unwrap(),0.35).is_none());
            eprintln!("Verified boundary {boundary}: {from:?} -> {next:?}");walked+=1;
        }
        assert!(walked>0,"No walkable native crossing at {boundary} m");
    }
}

#[test]
fn captured_outdoor_observations_have_native_ground() {
    let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let mut world=Geometry::open(std::path::Path::new(&path)).unwrap();
    for p in [
        Placement{cell:0x76b5002b,coordinates:[101.744698,52.311005,138.205872],heading:0},
        Placement{cell:0x76b50001,coordinates:[6.200933,18.482271,129.885986],heading:0},
    ] {
        let floor=world.ground(p,500.,1000.).expect("Captured outdoor native floor");
        assert!((p.coordinates[2]-floor.coordinates[2]).abs()<0.02,"Capture must agree with this DAT terrain");
        eprintln!("Outdoor capture {p:?}, floor={floor:?}");
        let observer=p.translated([12.,0.,0.]).unwrap();
        let observer=world.ground(observer,500.,1000.).expect("Native observer floor");
        eprintln!("Outdoor observer {observer:?}");
        assert!(world.contact(observer));
        if p.cell==0x76b5002b {assert!((observer.coordinates[2]-141.96219).abs()<0.02,"Rendered observer floor fixture");}
        else {assert!((observer.coordinates[2]-126.52316).abs()<0.02,"Second outdoor pursuit floor fixture");}
        let mut walking=p;
        for _ in 0..60 {
            let proposed=walking.translated([0.2,0.,0.]).unwrap();
            walking=world.walk_step(walking,proposed,0.35).expect("Captured creature can walk toward the test observer");
            assert!(world.contact(walking));
        }
        assert!(walking.distance(observer).unwrap()<0.03);
    }
}

#[test]
fn native_downhill_contacts_clear_fall_history_before_a_small_drop() {
    let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let mut world=Geometry::open(std::path::Path::new(&path)).unwrap();
    let start=Placement{cell:0x76b50001,coordinates:[6.200933,18.482271,129.885986],heading:0};
    let mut fall=Fall::default();
    let mut walking=world.ground(start,2.,5.).unwrap();
    let initial_z=walking.coordinates[2];fall.observe(0.,walking);
    for i in 1..=120 {
        let proposed=walking.translated([0.2,0.,0.]).unwrap();
        walking=world.walk_step(walking,proposed,0.35).expect("Native downhill step");
        assert!(world.contact(walking));
        fall.observe(i as f64*0.1,walking);
        assert_eq!(fall.resolve_landing(walking.coordinates[2],310,||world.contact(walking)),0);
    }
    eprintln!("Native downhill: {initial_z} -> {}",walking.coordinates[2]);
    assert!(initial_z-walking.coordinates[2]>6.,"Route must exceed the damage distance cumulatively");
    // A quick 0.12 m step is within the contact tolerance; it must not charge
    // the entire hillside walked before it, even at an apparent impact speed.
    let step=walking.translated([0.,0.,-0.12]).unwrap();
    assert!(world.contact(step));fall.observe(12.012,step);
    assert_eq!(fall.resolve_landing(step.coordinates[2],310,||world.contact(step)),0);
}
