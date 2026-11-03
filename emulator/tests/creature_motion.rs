//! Exercise the same native movement setup that the live server uses, across
//! every installed actor (captured, dungeon, and inferred wilderness).
use pyrealium::{content::Content,encounters::Encounters};
use std::{collections::BTreeMap,path::Path};

#[test]
fn installed_population_resolves_native_locomotion_without_uniform_fallbacks() {
    let Ok(data)=std::env::var("AC2_TEST_GAME_DATA") else {return};
    let root=Path::new(env!("CARGO_MANIFEST_DIR"));
    let content=Content::load(&root.join("../data/world/legions-world.tsv"),Path::new(&data)).unwrap();
    let mut encounters=Encounters::load(Path::new("training"),&content).unwrap();
    encounters.load_motion(&content,Path::new(&data)).unwrap();
    let mut speeds=BTreeMap::<u32,usize>::new();let mut count=0;
    for object in content.objects() {
        if let Some(profile)=encounters.movement_profile(object.id) {
            assert_eq!(profile.descriptor>>24,0x32,"{:016x}",object.id);
            assert!(profile.combat.speed.is_finite() && profile.combat.speed>=0.);
            *speeds.entry((profile.combat.speed*1000.).round() as u32).or_default()+=1;
            count+=1;
        }
    }
    assert!(count>8000);assert!(speeds.len()>10,"All creatures must not have one pace: {speeds:?}");
    println!("{count} creatures use {} native speed limits, {:.3}..{:.3} m/s after placement scale",speeds.len(),
        *speeds.first_key_value().unwrap().0 as f32/1000.,*speeds.last_key_value().unwrap().0 as f32/1000.);
}
