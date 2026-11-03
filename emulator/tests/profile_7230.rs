//! A separate process keeps the release-specific catalog from affecting Legions tests.
use pyrealium::{abilities::{Aim, Hand, Outcome, Runtime}, progression::{Command, Progression}, skills::Book};

#[test]
fn native_7230_tables_validate_and_flame_bolt_is_executable() {
    pyrealium::data_profile::set_unconverted_7230(true);
    if let Ok(path) = std::env::var("AC2_TEST_7230_PORTAL") {
        let mut portal = pyrealium::dat::Dat::open(path).unwrap();
        pyrealium::skills::validate_assets(&mut portal).unwrap();
        pyrealium::abilities::validate_assets(&mut portal).unwrap();
    }
    let state = Progression::default().apply(&Command::AwardXp(1000)).unwrap();
    let book = Book::default().with_starter_skills(&state, 1, 0x1000).unwrap()
        .apply(pyrealium::skills::Command::Train(0x59), &state, 1).unwrap();
    let mut runtime = Runtime::load().unwrap();
    let cast = runtime.prepare(1, 0x59, Aim::Creature(2),
        (Hand::Implement(0x40000), Hand::Empty), &book, &state, 0).unwrap();
    assert!(matches!(cast.outcome(), Outcome::Strike { cost: 32, bonus: 3, burn: Some(_), .. }));
    assert_eq!(cast.next().vigor, state.vigor, "strike cost is charged at impact");
    assert_ne!(cast.presentation().0, 0);
}
