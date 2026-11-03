//! Installed DAT regressions for recovered service identity and localized text.
use pyrealium::{dat::Dat,examination::Names,services::{Profiles,Service}};
use std::path::Path;
#[test]
fn gateway_and_road_signs_are_readable_native_services() {
    let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
    let directory=Path::new(&directory);let mut dat=Dat::open(directory.join("portal.dat")).unwrap();
    let mut profiles=Profiles::open(&mut dat).unwrap();
    for did in [0x47001382,0x470011c1,0x470011bd] {assert_eq!(profiles.get(&mut dat,did),Some(Service::Sign));}
    assert_ne!(profiles.get(&mut dat,0x47001c27),Some(Service::Sign));
    let names=Names::load(directory,[0x47001382,0x470011c1,0x47001c27].into_iter()).unwrap();
    assert!(names.description_chat(0x47001382).unwrap().is_ok());
    assert!(names.description_lines(0x47001382).join(" ").contains("shores of Lake Blessed"));
    assert!(names.description_lines(0x47001c27).join(" ").contains("Arwic Factotum"));
    assert!(names.description_chat(0x4700ffff).is_none());
}
