    use super::*;

    fn caster() -> (Progression, Book) {
        let state = Progression::default()
            .apply(&Command::AwardXp(1000))
            .unwrap();
        let book = Book::default()
            .with_starter_skills(&state, 1, 0x1000)
            .unwrap()
            .apply(crate::skills::Command::Train(0x59), &state, 1)
            .unwrap();
        (state, book)
    }

    #[test]
    fn flame_bolt_validates_training_implement_and_target_before_committing() {
        let (state, book) = caster();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let mut runtime = Runtime::load().unwrap();
        assert!(matches!(
            runtime.prepare(
                1,
                0x59,
                Aim::Creature(2),
                hands,
                &Book::default(),
                &state,
                0
            ),
            Err(AbilityError::NotLearned)
        ));
        assert!(matches!(
            runtime.prepare(
                1,
                0x59,
                Aim::Creature(2),
                (Hand::Unknown, Hand::Empty),
                &book,
                &state,
                0
            ),
            Err(AbilityError::Hand { .. })
        ));
        for aim in [Aim::Own, Aim::Player(2, 2.0), Aim::Other] {
            assert!(matches!(
                runtime.prepare(1, 0x59, aim, hands, &book, &state, 0),
                Err(AbilityError::TargetCreature)
            ));
        }
        let cast = runtime
            .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap();
        assert_eq!(cast.presentation(), (0x410000b6, 0x41000241));
        assert_eq!(
            *cast.outcome(),
            Outcome::Strike {
                target: 2,
                bonus: 3,
                vigor_damage: 0,
                weapon_damage: true,
                cost: 32,
                range: 60.0,
                burn: Some(Over {
                    health: 1,
                    vigor: 0,
                    period_ms: 1000,
                    duration_ms: 30000
                })
            }
        );
        assert!(
            cast.commands().is_empty(),
            "a queued strike pays at impact, not twice"
        );
        drop(cast);
        // Queue/persistence failure must not spend the cooldown.
        runtime
            .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap()
            .commit();
        assert!(matches!(
            runtime.prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 999),
            Err(AbilityError::Cooldown { .. })
        ));
        assert!(
            runtime
                .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 1000)
                .is_ok()
        );
        let empty = state.apply(&Command::SpendVigor(state.vigor)).unwrap();
        assert!(matches!(
            runtime.prepare(1, 0x59, Aim::Creature(2), hands, &book, &empty, 1000),
            Err(AbilityError::InsufficientVigor)
        ));
    }

    #[test]
    fn healing_uses_native_cost_and_rank_and_does_not_heal_an_enemy() {
        let (state, book) = caster();
        let book = book
            .apply(crate::skills::Command::Train(0x55), &state, 1)
            .unwrap();
        let state = state.apply(&Command::Damage(50)).unwrap();
        let mut runtime = Runtime::load().unwrap();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let cast = runtime
            .prepare(1, 0x55, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap();
        assert!(matches!(
            cast.outcome(),
            Outcome::Restore { health: 25, .. }
        ));
        assert_eq!(cast.next().health, state.health + 25);
        assert_eq!(cast.next().vigor, state.vigor.saturating_sub(100));
        drop(cast);
        assert!(matches!(
            runtime.prepare(1, 0x55, Aim::Player(2, 41.0), hands, &book, &state, 0),
            Err(AbilityError::OutOfRange)
        ));
        let cast = runtime
            .prepare(1, 0x55, Aim::Player(2, 40.0), hands, &book, &state, 0)
            .unwrap();
        assert!(matches!(
            cast.outcome(),
            Outcome::Support {
                target: 2,
                health: 25,
                ..
            }
        ));
        assert_eq!(cast.next().health, state.health);
    }

    #[test]
    fn trained_native_vigor_attacks_keep_cost_target_and_rank_tables() {
        let state = Progression::default().apply(&Command::AwardXp(1000)).unwrap();
        let mut lugian = Book::default().with_starter_skills(&state, 2, 0x1000).unwrap();
        for skill in [0x11e, 0x110, 0x10f] {
            lugian = lugian.apply(crate::skills::Command::Train(skill), &state, 2).unwrap();
        }
        let mut runtime = Runtime::load().unwrap();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let cast = runtime.prepare(1, 0x10f, Aim::Creature(2), hands, &lugian, &state, 0).unwrap();
        assert!(matches!(cast.outcome(), Outcome::Strike {
            bonus: 7, vigor_damage: 40, weapon_damage: true, cost: 80, burn: None, ..
        }));
        assert!(cast.commands().is_empty(), "cost is paid once at impact");
        drop(cast);
        assert!(matches!(runtime.prepare(1, 0x10f, Aim::Player(2, 1.), hands, &lugian, &state, 0), Err(AbilityError::TargetCreature)));
        assert!(matches!(runtime.prepare(1, 0x10f, Aim::Own, hands, &lugian, &state, 0), Err(AbilityError::TargetCreature)));
        assert!(matches!(runtime.prepare(1, 0x10f, Aim::Creature(2), (Hand::Empty, Hand::Empty), &lugian, &state, 0), Err(AbilityError::Hand {..})));
        assert_eq!(runtime.definitions[&0x10f].vigor_effect.at(50), -400.);

        let mut tumerok = Book::default().with_starter_skills(&state, 4, 0x1000).unwrap();
        for skill in [0x2a1, 0x2a6] {
            tumerok = tumerok.apply(crate::skills::Command::Train(skill), &state, 4).unwrap();
        }
        let cast = runtime.prepare(1, 0x2a6, Aim::Creature(2), (Hand::Implement(8), Hand::Empty), &tumerok, &state, 0).unwrap();
        assert!(matches!(cast.outcome(), Outcome::Strike { vigor_damage: 0, cost: 50,
            burn: Some(Over {health: 0, vigor: 2, period_ms: 1000, duration_ms: 15000}), .. }));
        drop(cast);
        for skill in [0xb8, 0xd0, 0x10f, 0x116, 0x127, 0x16d, 0x29d, 0x2a6, 0x2d0, 0x317, 0x31a] {
            assert!(runtime.supports(skill), "native vigor skill {skill:x}");
        }
        // Enabling vigor alone must not admit a partially decoded compound spell.
        assert!(!runtime.supports(0x52));
    }

    #[test]
    fn effect_tables_reject_nonfinite_and_ambiguous_coordinates() {
        for table in ["1:NaN", "NaN:2", "1:2,1:3", "-1:2", "1:100001"] {
            assert!(Table::parse(table).is_err(), "{table}");
        }
    }

    #[test]
    fn unknown_effect_components_are_not_silently_omitted_from_a_paid_cast() {
        let mut runtime = Runtime::load().unwrap();
        // This native spell combines damage with an unresolved status record.
        assert!(!runtime.supports(0x52));
        assert!(matches!(
            runtime.prepare(
                1,
                0x52,
                Aim::Creature(2),
                (Hand::Implement(0x40000), Hand::Empty),
                &Book::default(),
                &Progression::default(),
                0
            ),
            Err(AbilityError::Unsupported)
        ));
        assert!(runtime.skills.is_empty() && runtime.players.is_empty());
    }

    #[test]
    fn retail_skill_records_drive_heal_and_strike_values() {
        let runtime = Runtime::load().unwrap();
        let ease = &runtime.definitions[&0x55];
        assert_eq!(
            (ease.reset_ms, ease.vigor, ease.range, ease.targets),
            (7000, 100, 40.0, 0x101)
        );
        assert_eq!((ease.health.at(1), ease.health.at(50)), (25.0, 250.0));
        let bolt = &runtime.definitions[&0x59];
        assert_eq!(
            (bolt.reset_ms, bolt.vigor, bolt.range, bolt.right, bolt.left),
            (1000, 32, 60.0, 0x40000, 1)
        );
        assert_eq!((bolt.damage.at(1), bolt.damage.at(50)), (3.0, 75.0));
        let burn = &bolt.timed[0];
        assert_eq!(
            (burn.values.at(1), burn.values.at(50), burn.period_ms),
            (-1.0, -4.0, 1000)
        );
        assert_eq!(burn.duration.at(1), 30.0);
        assert!(bolt.harmful() && !ease.harmful());
        let defense = runtime.definitions[&0x6b]
            .ratings_for(Side::Own, 1)
            .unwrap();
        assert_eq!(defense.0.attack(crate::combat::School::Melee), 7);
        assert_eq!(defense.1, 180_000);
        let aura = runtime.definitions[&0xa1]
            .ratings_for(Side::Target, 50)
            .unwrap();
        assert_eq!(aura.0.defense(crate::combat::School::Magic), 100);
        let calm = &runtime.definitions[&0x67];
        assert_eq!(
            calm.buff.as_ref().map(|b| (b.vigor.at(1), b.vigor.at(50))),
            Some((1.1, 2.0))
        );
        assert!(runtime.definitions[&0xb0].weapon);
        assert_eq!(runtime.definitions[&0x343].travel, Some(Travel::Lifestone));
        assert_eq!(runtime.definitions[&0x366].travel, Some(Travel::Portal));
        assert!(
            matches!(runtime.definitions[&0x39d].travel, Some(Travel::Telepads(ref pads)) if pads[0].cell == 0x6ea70025)
        );
    }

    #[test]
    fn hands_follow_the_implement_masks() {
        assert!(holds(0x40000, Hand::Implement(0x40000)));
        assert!(!holds(0x40000, Hand::Implement(0x2)));
        assert!(!holds(0x40000, Hand::Empty));
        assert!(holds(0x1, Hand::Empty));
        assert!(holds(0x80073, Hand::Empty));
        assert!(holds(0xa0062, Hand::Implement(0x2)));
        assert!(holds(0xffffffff, Hand::Empty));
        assert!(!holds(0xfffffffe, Hand::Empty));
        assert!(!holds(0x40000, Hand::Unknown));
    }

    #[test]
    fn table_interpolates_between_level_points_and_clamps_outside() {
        let table = Table::parse("1:3,50:75,150:500").unwrap();
        assert_eq!(table.at(0), 3.0);
        assert!((table.at(25) - 38.265).abs() < 0.01);
        assert_eq!(table.at(150), 500.0);
        assert_eq!(table.at(400), 500.0);
    }

    #[test]
    fn a_strike_names_the_timed_effects_it_puts_on_the_creature() {
        let (state, book) = caster();
        let hands = (Hand::Implement(0x40000), Hand::Empty);
        let mut runtime = Runtime::load().unwrap();
        // Flame Bolt's burn is on the creature when the bolt lands, not on the player now.
        let cast = runtime
            .prepare(1, 0x59, Aim::Creature(2), hands, &book, &state, 0)
            .unwrap();
        assert!(cast.applied().is_empty());
        assert_eq!(cast.creature_effects(), [(0x7000_0776, 30_000), (0, 0)]);
        assert_eq!(
            runtime.effect_label(0x59, 0x7000_0776),
            "Burning (health lost over time)"
        );
    }
