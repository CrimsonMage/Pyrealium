//! Combat and skill visuals, health stories, rating and effect lines.
use super::*;

pub(super) fn training_combat_visual(
    peer: &mut Peer,
    character: u64,
    store: &Store,
    scene: &mut crate::scene::Scene,
    fighting: bool,
    swing: bool,
    skill: bool,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    let mode = if fighting { crate::starter_gear::combat_mode(&saved.inventory) }
        else { crate::combat_visual::PEACE };
    let (mut next, stamp) = peer.emotes.next_visual(saved.instance).map_err(other)?;
    let mut last_stamp = stamp;
    let mut events = vec![
        crate::combat_visual::mode_id(character, saved.instance, stamp, mode).map_err(other)?,
    ];
    if swing {
        let (after, stamp) = next.next_visual(saved.instance).map_err(other)?;
        next = after;
        last_stamp = stamp;
        let attack=if skill && mode==crate::combat_visual::MAGIC {
            crate::combat_visual::attack_behavior(character,saved.instance,stamp,if saved.creation.as_ref().is_some_and(|r|r.race==4){0x410000bc}else{0x410000b9})
        } else {crate::combat_visual::weapon_attack(character,saved.instance,stamp,mode)};
        events.push(attack.map_err(other)?);
        if mode==crate::combat_visual::MAGIC {
            if let Some((fx,_))=saved.creation.as_ref().and_then(|r|crate::combat_visual::magic_effects(r.race)) {
                let (after,stamp)=next.next_visual(saved.instance).map_err(other)?;next=after;last_stamp=stamp;
                events.push(crate::combat_visual::effect(character,saved.instance,stamp,fx).map_err(other)?);
            }
        }
    }
    peer.validate_scene_batch(&events)?;
    let mut deliveries = Vec::new();
    for event in events {
        deliveries.extend(scene.broadcast(character, event).map_err(other)?);
    }
    scene
        .save_combat_mode(character, mode, last_stamp)
        .map_err(other)?;
    peer.emotes = next;
    Ok(deliveries)
}

/// A player's body lies down dead (`death` behaviour, actions locked) or stands up in the peace
/// mode again, for the player and the observers; the mode is kept for late observers.
pub(super) fn player_body_alive(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    alive: bool,
) -> Vec<crate::scene::Delivery> {
    let Ok((mut next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let mut last = stamp;
    let mut wires = Vec::new();
    if alive {
        let Ok((after, second)) = next.next_visual(instance) else {
            return Vec::new();
        };
        next = after;
        last = second;
        let (Ok(a), Ok(b)) = (
            crate::combat_visual::player_alive(owner, instance, stamp),
            crate::combat_visual::stop_death(owner, instance, second),
        ) else {
            return Vec::new();
        };
        wires.extend([a, b]);
    } else {
        let Ok(a) = crate::combat_visual::player_death(owner, instance, stamp) else {
            return Vec::new();
        };
        wires.push(a);
    }
    let mut deliveries = Vec::new();
    for wire in wires {
        let Ok(more) = scene.broadcast(owner, wire) else {
            return Vec::new();
        };
        deliveries.extend(more);
    }
    let mode = if alive { crate::combat_visual::PEACE } else { crate::combat_visual::DEAD_MODE };
    if scene.save_combat_mode(owner, mode, last).is_err() {
        return Vec::new();
    }
    peer.emotes = next;
    deliveries
}

pub(super) fn publish_health_story(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    attacker: u64,
    health_change: i32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) =
        crate::combat_visual::damage_story(owner, instance, stamp, attacker, health_change)
    else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) => {
            if scene.save_visual_stamp(owner, stamp).is_err() {
                return Vec::new();
            }
            peer.emotes = next;
            deliveries
        }
        Err(_) => Vec::new(),
    }
}

pub(super) fn skill_visual(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    behavior: u32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) = crate::combat_visual::attack_behavior(owner, instance, stamp, behavior) else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) if scene.save_visual_stamp(owner, stamp).is_ok() => {
            peer.emotes = next;
            deliveries
        }
        _ => Vec::new(),
    }
}


/// [`skill_visual`] for a skill that fires a missile: the cast story names the skill and its
/// fire-missile hook, so every client in view makes and flies the missile itself.
#[allow(clippy::too_many_arguments)]
pub(super) fn skill_missile_visual(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    behavior: u32,
    skill: u32,
    target: u64,
    hook: u32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) = crate::combat_visual::skill_missile_story(owner, instance, stamp, behavior, skill, target, hook) else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) if scene.save_visual_stamp(owner, stamp).is_ok() => {
            peer.emotes = next;
            deliveries
        }
        _ => Vec::new(),
    }
}

pub(super) fn player_fx(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    fx: u32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) = crate::combat_visual::effect(owner, instance, stamp, fx) else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) if scene.save_visual_stamp(owner, stamp).is_ok() => {
            peer.emotes = next;
            deliveries
        }
        _ => Vec::new(),
    }
}


pub(super) fn rating_sentence(cast: &crate::abilities::PreparedCast<'_>) -> String {
    let mut lines = Vec::new();
    if let Some((ratings, duration)) = cast.caster_ratings() {
        lines.push(crate::combat::describe(&ratings, duration));
    }
    if let Some((who, ratings, duration)) = cast.other_ratings() {
        if who != cast.player() {
            lines.push(format!("On your target, {}", crate::combat::describe(&ratings, duration)));
        }
    }
    lines.join(" ")
}
pub(super) fn store_ratings(
    runtime: &mut Option<crate::encounters::Encounters>,
    cast: &crate::abilities::PreparedCast<'_>,
    now: u64,
) -> io::Result<()> {
    if cast.caster_ratings().is_none() && cast.other_ratings().is_none() {
        return Ok(());
    }
    let runtime = runtime.as_mut().ok_or_else(|| invalid("Lasting skill effects require combat rules on this server."))?;
    if let Some((ratings, duration)) = cast.caster_ratings() {
        runtime.add_ratings(cast.player(), cast.skill(), ratings, duration, now)?;
    }
    if let Some((who, ratings, duration)) = cast.other_ratings().filter(|_|!matches!(cast.outcome(),crate::abilities::Outcome::Strike{..})) {
        runtime.add_ratings(who, cast.skill(), ratings, duration, now)?;
    }
    Ok(())
}

/// The timed effects on `target` as examination lines, with the time left.
pub(super) fn effect_lines(
    encounters: &Option<crate::encounters::Encounters>,
    abilities: &Option<crate::abilities::Runtime>,
    target: u64,
    now: u64,
) -> Vec<String> {
    let (Some(encounters), Some(abilities)) = (encounters, abilities) else {
        return Vec::new();
    };
    encounters
        .effects_on(target, now)
        .into_iter()
        .map(|(skill, did, left)| {
            format!("{}: {} s left", abilities.effect_label(skill, did), left.div_ceil(1000))
        })
        .collect()
}

/// The timed effects a cast put on players join the effect lists (`WM_Effect` 0x17 follows with
/// the next tick, `Event::EffectAdded`).
pub(super) fn store_effects(
    runtime: &mut Option<crate::encounters::Encounters>,
    cast: &crate::abilities::PreparedCast<'_>,
    now: u64,
) {
    if let Some(runtime) = runtime.as_mut() {
        for effect in cast.applied() {
            runtime.note_effect(effect.entity, cast.skill(), effect.did, effect.duration_ms, now, effect.power, effect.pulsed);
        }
    }
}
