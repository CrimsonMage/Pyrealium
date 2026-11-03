use super::*;
use crate::{scene::Body, world};

fn fixture(label: &str) -> (Encounters, Scene, Store, std::path::PathBuf, u64) {
    let path = std::env::temp_dir().join(format!(
        "ac2-skill-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut store = Store::open(&path).unwrap();
    store.create("local", "Spell Tester").unwrap();
    store
        .progression_command("local", 1, Command::AwardXp(1000))
        .unwrap();
    store
        .quest_command("local", 1, crate::quests::Command::Accept)
        .unwrap();
    let id = crate::content::NAMESPACE + 7;
    let position = Placement::HUMAN_START;
    let mut target = position;
    target.coordinates[0] += 2.0;
    let content = Content::fixture(id, target, None);
    let mut runtime = Encounters::parse(
        include_str!("../../content/development-encounters.tsv"),
        &content,
    )
    .unwrap();
    runtime.dice = crate::combat::Dice::for_test(1);
    runtime.enable_training_loot();
    runtime.next_regen = u64::MAX;
    let mut scene = Scene::with_content(content);
    runtime.initialize_spawners(&mut scene, 0).unwrap();
    assert_eq!(
        runtime.spawners.poll(0),
        vec![crate::spawners::Action::Spawn(id)]
    );
    scene.respawn_content(id).unwrap();
    runtime.spawned(id).unwrap();
    let bytes = world::create_stationary_object(&world::StationaryObject {
        character_id: 1,
        parent_visual_did: 1,
        entity_did: 2,
        cell_id: position.cell,
        position: position.coordinates,
        orientation: position.orientation(),
        instance_stamp: 1,
        visual_ordering_stamp: 0,
        timestamps: [0; 4],
    })
    .unwrap();
    scene.enter(Body::new(1, position, bytes).unwrap()).unwrap();
    let actor = runtime.actors.get_mut(&id).unwrap();
    actor.target = Some(1);
    actor.next_attack = u64::MAX;
    (runtime, scene, store, path, id)
}

fn bolt() -> Strike {
    Strike {
        ratings: None,
        skill: 0x59,
        bonus: 3,
        vigor_damage: 0,
        weapon_damage: true,
        cost: 32,
        range: 60.0,
        behavior: 0x410000b6,
        fx: 0x41000241,
        burn: Some(Over {
            health: 1,
            vigor: 0,
            period_ms: 1000,
            duration_ms: 30000,
        }),
        effects: [(0, 0); 2],
    }
}

#[test]
fn treasure_weapon_damage_drives_skill_hits_but_not_periodic_ticks() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("treasure-weapon");
    let item = crate::inventory::Item {
        id: crate::inventory::ITEM_NAMESPACE + 1, definition: 0x47000f0b, visual: 0x1f0002a5,
        equipment_mask: 0x40, quantity: 1, stack_limit: 1, location: crate::inventory::Location::Bag(0),
        roll: Some(crate::inventory::Roll {damage:28,level:10,..Default::default()})
    };
    store.inventory_command("local",1,crate::inventory::Command::Grant(item.clone())).unwrap();
    store.inventory_command("local",1,crate::inventory::Command::Wear {item:item.id,slot:0x40}).unwrap();
    runtime.strike(1,target,0,&scene,&store,bolt()).unwrap();
    runtime.tick(0,&mut scene,&mut store,[1].into_iter(),None).unwrap();
    assert_eq!(runtime.actors[&target].health,29,"28 weapon + 3 Flame Bolt bonus");
    runtime.tick(1000,&mut scene,&mut store,[1].into_iter(),None).unwrap();
    assert_eq!(runtime.actors[&target].health,28,"burn cannot add the weapon a second time");
    drop(store);
    let reopened=Store::open(&path).unwrap();
    assert_eq!(crate::professions::weapon_damage(&reopened.characters[&1].inventory),Some(28));
    drop(reopened);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn burn_kill_uses_normal_quest_reward_loot_and_spawner_transaction_once() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("burn");
    runtime.actors.get_mut(&target).unwrap().health = 26;
    let before = store.characters[&1].progression.clone();
    runtime
        .strike(1, target, 0, &scene, &store, bolt())
        .unwrap();
    let first = runtime
        .tick(0, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert!(first.iter().any(|e| matches!(
        e,
        Event::SkillSwing {
            behavior: 0x410000b6,
            ..
        }
    )));
    assert_eq!(store.characters[&1].progression.vigor, before.vigor - 32);
    assert_eq!(runtime.actors[&target].health, 3);
    let mut events = Vec::new();
    for now in [1000, 2000, 3000, 4000] {
        events.extend(
            runtime
                .tick(now, &mut scene, &mut store, [1].into_iter(), None)
                .unwrap(),
        );
    }
    assert_eq!(runtime.actors[&target].health, 0);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Loot { .. }))
            .count(),
        1
    );
    assert_eq!(store.characters[&1].quests.get().unwrap().killed, 1);
    assert_eq!(store.characters[&1].progression.xp, before.xp + 40);
    assert_eq!(store.characters[&1].progression.gold, before.gold + 2);
    assert_eq!(store.characters[&1].progression.vigor, before.vigor - 32);
    assert!(runtime.burns.is_empty());
    // A new incarnation at the same content ID must not inherit the old burn.
    scene.despawn_content(target).unwrap();
    runtime.removed(target, 4000);
    assert_eq!(
        runtime.spawners.poll(34000),
        vec![crate::spawners::Action::Spawn(target)]
    );
    scene.respawn_content(target).unwrap();
    runtime.spawned(target).unwrap();
    runtime
        .tick(34000, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(runtime.actors[&target].health, 60);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.characters[&1].quests.get().unwrap().killed, 1);
    assert_eq!(reopened.characters[&1].progression.xp, before.xp + 40);
    assert_eq!(reopened.characters[&1].progression.vigor, before.vigor - 32);
    drop(reopened);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn cancelled_or_invalid_spell_cannot_charge_or_leave_a_burn() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("cancel");
    let before = store.characters[&1].progression.clone();
    assert!(runtime.strike(1, 1, 0, &scene, &store, bolt()).is_err());
    assert!(
        runtime
            .strike(1, target + 1, 0, &scene, &store, bolt())
            .is_err()
    );
    runtime
        .strike(1, target, 0, &scene, &store, bolt())
        .unwrap();
    runtime.stop(1);
    runtime
        .tick(0, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression, before);
    assert_eq!(runtime.actors[&target].health, 60);
    assert!(runtime.burns.is_empty());
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn vigor_drains_clamp_expire_and_restore_on_a_new_spawn_lifetime() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("vigor");
    let actor = runtime.actors.get_mut(&target).unwrap();
    actor.rule.vigor = Some(100);
    actor.vigor = Some(100);
    let before = store.characters[&1].progression.clone();
    let effect = Strike { bonus: 0, vigor_damage: 40, weapon_damage: false,
        burn: Some(Over {health: 0, vigor: 35, period_ms: 1000, duration_ms: 2000}), ..bolt() };
    runtime.strike(1, target, 0, &scene, &store, effect).unwrap();
    for (now, expected) in [(0,60),(900,60),(1000,25),(2000,0),(3000,0)] {
        let events = runtime.tick(now, &mut scene, &mut store, [1].into_iter(), None).unwrap();
        assert_eq!(runtime.actors[&target].vigor, Some(expected));
        assert_eq!(runtime.actors[&target].health, 60);
        assert_eq!(store.characters[&1].progression.xp, before.xp);
        assert_eq!(store.characters[&1].progression.vigor, before.vigor-32);
        assert!(!events.iter().any(|e| matches!(e, Event::Loot{..}|Event::Hit{..})));
        if [0,1000,2000].contains(&now) {
            assert!(events.iter().any(|e| matches!(e,Event::Vigor{actor,vigor} if *actor==target && *vigor==expected)));
        }
    }
    assert!(runtime.burns.is_empty());
    assert!(runtime.inspection_stats().any(|(id,key,value)|id==target && key==0x103 && value==0));
    // Removing and publishing a new incarnation restores both authoritative vitals.
    assert!(runtime.spawners.died(target, 3000, 0));
    runtime.spawners.poll(3000);
    scene.despawn_content(target).unwrap(); runtime.removed(target,3000);
    assert_eq!(runtime.spawners.poll(33000),vec![crate::spawners::Action::Spawn(target)]);
    scene.respawn_content(target).unwrap(); runtime.spawned(target).unwrap();
    assert_eq!(runtime.actors[&target].vigor,Some(100));
    drop(store);
    let reopened=Store::open(&path).unwrap();
    assert_eq!(reopened.characters[&1].progression.vigor,before.vigor-32);
    drop(reopened); std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn missed_vigor_spell_charges_once_but_never_applies_or_ticks() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("vigor-miss");
    // Choose a deterministic first roll above even the highest permitted hit chance.
    let seed=(1..1000).find(|seed|crate::combat::Dice::for_test(*seed).unit()>0.95).unwrap();
    runtime.dice=crate::combat::Dice::for_test(seed);
    let actor=runtime.actors.get_mut(&target).unwrap();actor.rule.vigor=Some(100);actor.vigor=Some(100);
    let before=store.characters[&1].progression.clone();
    runtime.strike(1,target,0,&scene,&store,Strike {vigor_damage:40,
        burn:Some(Over{health:1,vigor:10,period_ms:1000,duration_ms:2000}),..bolt()}).unwrap();
    let events=runtime.tick(0,&mut scene,&mut store,[1].into_iter(),None).unwrap();
    assert!(!events.iter().any(|e|matches!(e,Event::Vigor{..}|Event::Hit{..})));
    assert_eq!(runtime.actors[&target].vigor,Some(100));assert_eq!(runtime.actors[&target].health,60);
    assert_eq!(store.characters[&1].progression.vigor,before.vigor-32);
    assert!(runtime.burns.is_empty());
    drop(store);std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn a_debuff_changes_ratings_without_inventing_weapon_damage_and_expires() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("debuff");
    let effect = Strike {
        bonus: 0,
        weapon_damage: false,
        burn: None,
        ratings: Some((
            crate::combat::Ratings {
                armor: -5,
                ..Default::default()
            },
            1000,
        )),
        ..bolt()
    };
    runtime
        .strike(1, target, 0, &scene, &store, effect)
        .unwrap();
    let events = runtime
        .tick(0, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(runtime.actors[&target].health, 60);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Hit { victim, .. } if *victim == target))
    );
    assert_eq!(runtime.ratings_for(target).armor, -5);
    runtime
        .tick(1000, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(runtime.ratings_for(target).armor, 0);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn healing_over_time_respects_period_expiry_and_never_resurrects() {
    let (mut runtime, mut scene, mut store, path, _) = fixture("heal");
    store
        .progression_command("local", 1, Command::Damage(60))
        .unwrap();
    let before = store.characters[&1].progression.health;
    runtime
        .mend(
            1,
            0x123,
            1,
            Over {
                health: 5,
                vigor: 0,
                period_ms: 1000,
                duration_ms: 2000,
            },
            0,
            0,
        )
        .unwrap();
    for (now, healed) in [(0, 0), (1000, 5), (2000, 10), (3000, 10)] {
        runtime
            .tick(now, &mut scene, &mut store, [1].into_iter(), None)
            .unwrap();
        assert_eq!(store.characters[&1].progression.health, before + healed);
    }
    runtime
        .mend(
            1,
            0x123,
            1,
            Over {
                health: 5,
                vigor: 0,
                period_ms: 1000,
                duration_ms: 2000,
            },
            0,
            3000,
        )
        .unwrap();
    store
        .progression_command("local", 1, Command::Damage(u32::MAX))
        .unwrap();
    runtime
        .tick(4000, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression.health, 0);
    assert!(runtime.mends.is_empty());
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn an_expired_heal_never_catches_up_after_a_stall_but_instant_support_lands() {
    let (mut runtime, mut scene, mut store, path, _) = fixture("expiry");
    store
        .progression_command("local", 1, Command::Damage(60))
        .unwrap();
    let before = store.characters[&1].progression.health;
    runtime
        .mend(
            1,
            0x123,
            1,
            Over {
                health: 5,
                vigor: 0,
                period_ms: 1000,
                duration_ms: 2000,
            },
            0,
            0,
        )
        .unwrap();
    runtime
        .tick(3000, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression.health, before);
    runtime
        .mend(
            1,
            0x124,
            1,
            Over {
                health: 25,
                vigor: 0,
                period_ms: 1000,
                duration_ms: 0,
            },
            0,
            3000,
        )
        .unwrap();
    runtime
        .tick(3100, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression.health, before + 25);
    assert!(runtime.mends.is_empty());
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn zero_cost_ranged_skill_releases_without_spending_vigor() {
    let (mut runtime, mut scene, mut store, path, target) = fixture("free");
    let before = store.characters[&1].progression.vigor;
    runtime
        .strike(1, target, 0, &scene, &store, Strike { cost: 0, ..bolt() })
        .unwrap();
    // The server obtains this category from the equipped implement.
    runtime.attacks.get_mut(&1).unwrap().category = 2;
    let events = runtime
        .tick(0, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::SkillSwing { .. })));
    assert_eq!(
        runtime.actors[&target].health, 60,
        "No impact before release"
    );
    // Flame Bolt fires a missile the client flies (`crate::skill_missiles`): released, it lands
    // after its flight.
    runtime
        .tick(
            crate::projectiles::RELEASE_MS,
            &mut scene,
            &mut store,
            [1].into_iter(),
            None,
        )
        .unwrap();
    assert_eq!(runtime.actors[&target].health, 60, "No impact while the bolt flies");
    let events = runtime
        .tick(
            crate::projectiles::RELEASE_MS + 1000,
            &mut scene,
            &mut store,
            [1].into_iter(),
            None,
        )
        .unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::Hit {victim, health_change, ..} if *victim == target && *health_change < 0)));
    assert_eq!(store.characters[&1].progression.vigor, before);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn zero_regeneration_does_not_submit_invalid_zero_healing() {
    let (mut runtime, mut scene, mut store, path, _) = fixture("zero-regen");
    runtime.actors.clear();
    runtime.next_regen = 0;
    store
        .progression_command("local", 1, Command::Damage(60))
        .unwrap();
    store
        .progression_command("local", 1, Command::SpendVigor(50))
        .unwrap();
    let before = store.characters[&1].progression.clone();
    runtime
        .boost_regen(
            1,
            0x123,
            Regen {
                health: 0.0,
                vigor: 0.0,
                duration_ms: 1000,
            },
            0,
        )
        .unwrap();
    runtime
        .tick(0, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression, before);
    runtime
        .tick(
            runtime.regen_interval.max(1000),
            &mut scene,
            &mut store,
            [1].into_iter(),
            None,
        )
        .unwrap();
    assert!(store.characters[&1].progression.health > before.health);
    assert!(store.characters[&1].progression.vigor > before.vigor);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn recasting_a_single_tick_heal_refreshes_instead_of_stacking() {
    let (mut runtime, mut scene, mut store, path, _) = fixture("refresh");
    store
        .progression_command("local", 1, Command::Damage(60))
        .unwrap();
    let before = store.characters[&1].progression.health;
    let effect = Over {
        health: 5,
        vigor: 0,
        period_ms: 1000,
        duration_ms: 1000,
    };
    runtime.mend(1, 0x123, 1, effect, 0, 0).unwrap();
    runtime.mend(1, 0x123, 1, effect, 0, 500).unwrap();
    runtime
        .tick(1000, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression.health, before);
    runtime
        .tick(1500, &mut scene, &mut store, [1].into_iter(), None)
        .unwrap();
    assert_eq!(store.characters[&1].progression.health, before + 5);
    assert!(runtime.mends.is_empty());
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

fn added(events: &[Event]) -> Vec<(u64, super::Applied)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::EffectAdded { owner, effect } => Some((*owner, *effect)),
            _ => None,
        })
        .collect()
}

fn removed(events: &[Event]) -> Vec<(u64, u32, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::EffectRemoved {
                owner,
                eid,
                expired,
            } => Some((*owner, *eid, *expired)),
            _ => None,
        })
        .collect()
}

#[test]
fn timed_effects_are_listed_replaced_expire_and_leave_with_their_owner() {
    let (mut runtime, mut scene, mut store, path, creature) = fixture("ledger");
    let tick = |runtime: &mut Encounters, scene: &mut Scene, store: &mut Store, now| {
        runtime
            .tick(now, scene, store, [1].into_iter(), None)
            .unwrap()
    };
    // A buff on the player and a burn on the creature join the lists with the next tick.
    runtime.note_effect(1, 0x67, 0x7000_0001, 2000, 0, 5.0, false);
    runtime.note_effect(creature, 0x59, 0x7000_0002, 5000, 0, 0.0, false);
    let events = tick(&mut runtime, &mut scene, &mut store, 0);
    let on = added(&events);
    assert_eq!(on.len(), 2);
    let first = on.iter().find(|(owner, _)| *owner == 1).unwrap().1;
    assert_eq!(
        (first.skill, first.did, first.cast_ms, first.duration_ms),
        (0x67, 0x7000_0001, 0, 2000)
    );
    assert_eq!(runtime.effects_on(1, 500), [(0x67, 0x7000_0001, 1500)]);
    assert_eq!(
        runtime.effects_on(creature, 500),
        [(0x59, 0x7000_0002, 4500)]
    );
    // A recast replaces the record: the old one leaves (not expired), a new id joins.
    runtime.note_effect(1, 0x67, 0x7000_0001, 2000, 1000, 5.0, false);
    let events = tick(&mut runtime, &mut scene, &mut store, 1000);
    assert_eq!(removed(&events), [(1, first.eid, false)]);
    let second = added(&events)[0].1;
    assert_ne!(second.eid, first.eid);
    assert_eq!(runtime.effects_on(1, 1000), [(0x67, 0x7000_0001, 2000)]);
    // The first record's time passes without a removal: the recast replaced it.
    let events = tick(&mut runtime, &mut scene, &mut store, 2500);
    assert!(removed(&events).is_empty());
    // The recast runs out at its own time.
    let events = tick(&mut runtime, &mut scene, &mut store, 3000);
    assert_eq!(removed(&events), [(1, second.eid, true)]);
    assert!(runtime.effects_on(1, 3000).is_empty());
    // A new buff leaves with its owner's death, not expired; the creature's burn goes on.
    runtime.note_effect(1, 0x56, 0x7000_0003, 60_000, 3000, 5.0, false);
    let events = tick(&mut runtime, &mut scene, &mut store, 3100);
    let third = added(&events)[0].1;
    store
        .progression_command("local", 1, Command::Damage(u32::MAX))
        .unwrap();
    let events = tick(&mut runtime, &mut scene, &mut store, 3200);
    assert_eq!(removed(&events), [(1, third.eid, false)]);
    assert_eq!(runtime.effects_on(creature, 3200).len(), 1);
    // The creature's burn leaves at its time.
    let events = tick(&mut runtime, &mut scene, &mut store, 5100);
    let gone = removed(&events);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].0, creature);
    assert!(runtime.effects_on(creature, 5100).is_empty());
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn quest_effects_follow_the_quest_book_and_outlive_a_death() {
    let (mut runtime, mut scene, mut store, path, _creature) = fixture("quest-effects");
    let tick = |runtime: &mut Encounters, scene: &mut Scene, store: &mut Store, now| {
        runtime.tick(now, scene, store, [1].into_iter(), None).unwrap()
    };
    const DECEIT: u32 = 0x6f00_020f;
    const SKIN: u32 = 0x6f00_0212;
    // Applied: both join the strip with the time the book gives them (Unix seconds 1000, 24 h).
    runtime.sync_quest_effects(1, &[(DECEIT, 1000 + 86_400), (SKIN, 1000 + 86_400)], 1000, 0, true);
    let events = tick(&mut runtime, &mut scene, &mut store, 0);
    let on = added(&events);
    assert_eq!(on.len(), 2);
    assert!(on.iter().all(|(o, a)| *o == 1 && a.skill == ledger::QUEST_EFFECT_SKILL && a.duration_ms == 86_400_000));
    // A sync with nothing changed sends nothing; a login sync adds silently.
    runtime.sync_quest_effects(1, &[(DECEIT, 1000 + 86_400), (SKIN, 1000 + 86_400)], 1500, 500, true);
    assert!(added(&tick(&mut runtime, &mut scene, &mut store, 500)).is_empty());
    // A death leaves the quest's effects on (the quest book holds them).
    store.progression_command("local", 1, Command::Damage(u32::MAX)).unwrap();
    assert!(removed(&tick(&mut runtime, &mut scene, &mut store, 600)).is_empty());
    assert_eq!(runtime.effects_on(1, 600).len(), 2);
    // Removed (the reward lifts the curses): both leave, not expired.
    runtime.sync_quest_effects(1, &[], 2000, 1000, true);
    let gone = removed(&tick(&mut runtime, &mut scene, &mut store, 1000));
    assert_eq!(gone.len(), 2);
    assert!(gone.iter().all(|(o, _, expired)| *o == 1 && !expired));
    // One applied with an hour left expires at its time.
    runtime.sync_quest_effects(1, &[(SKIN, 3600)], 0, 2000, false);
    assert_eq!(runtime.applied_to(1, 2000).len(), 1, "the login lists it");
    let expired = removed(&tick(&mut runtime, &mut scene, &mut store, 2000 + 3_600_000));
    assert_eq!(expired.len(), 1);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn an_escort_follows_its_player_rejoins_from_afar_and_is_lost_on_death() {
    let (mut runtime, mut scene, mut store, path, creature) = fixture("escort");
    runtime.actors.get_mut(&creature).unwrap().target = None;
    let entity = runtime.actors[&creature].entity;
    assert_eq!(runtime.start_escort(1, entity, &scene), Some(creature));
    assert_eq!(runtime.escorting(1), [creature]);
    let moves = |events: &[Event]| events.iter().filter_map(|e| match e { Event::Move { actor, placement } if *actor == creature => Some(*placement), _ => None }).collect::<Vec<_>>();
    // Close by (2 m): it stays.
    let events = runtime.tick(0, &scene, &mut store, [1].into_iter(), None).unwrap();
    assert!(moves(&events).is_empty());
    // Left far behind (a portal): it rejoins beside the player.
    let mut far = Placement::HUMAN_START;
    far.coordinates[0] += 50.0;
    far.coordinates[1] += 50.0;
    scene.warp_creature(creature, far, 0).unwrap();
    let events = runtime.tick(1000, &scene, &mut store, [1].into_iter(), None).unwrap();
    assert!(moves(&events).is_empty());
    assert!(!events.iter().any(|e| matches!(e, Event::Warp { .. })), "it waits for the player to arrive");
    let events = runtime.tick(4000, &scene, &mut store, [1].into_iter(), None).unwrap();
    let to = events.iter().filter_map(|e| match e { Event::Warp { actor, placement } if *actor == creature => Some(*placement), _ => None }).collect::<Vec<_>>();
    assert_eq!(to.len(), 1);
    scene.warp_creature(creature, to[0], 1000).unwrap();
    // (Walking after the player needs the walkable ground, which no unit fixture loads: the
    // live escort check covers it.)
    // Its death tells the quest; ending the escort releases the others.
    runtime.actors.get_mut(&creature).unwrap().health = 0;
    let events = runtime.tick(5000, &scene, &mut store, [1].into_iter(), None).unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::EscortLost { player: 1, entity: e2 } if *e2 == entity)));
    assert!(runtime.escorting(1).is_empty());
    runtime.end_escort(1);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
