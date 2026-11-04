//! Quest items and NPCs used in the world: elixirs, deliveries, training potions, quest actions.
use super::*;






pub(super) fn use_training_potion(
    peer: &mut Peer,
    character: u64,
    item: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    now: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if !scene.can_observe(character, character) || !saved.progression.alive() {
        return Err(invalid("Potion use requires a ready living character."));
    }
    if peer
        .last_world_use_ms
        .is_some_and(|last| now.saturating_sub(last) < crate::item_services::POTION_COOLDOWN_MS)
    {
        return Err(invalid("Wait five seconds before using another potion."));
    }
    let owned = saved
        .inventory
        .items()
        .find(|owned| owned.id == item)
        .filter(|owned| {
            (owned.definition == crate::item_services::POTION_DEFINITION
                && owned.visual == crate::item_services::POTION_VISUAL
                || owned.definition == crate::crafting::VIGOR_OUTPUT && owned.visual == 0x1f000e40)
                && owned.quantity == 1
                && owned.stack_limit == 1
                && owned.equipment_mask == 0
        })
        .ok_or_else(|| {
            invalid("This training potion is not owned or has unsupported properties.")
        })?;
    let vigor = owned.definition == crate::crafting::VIGOR_OUTPUT;
    if !matches!(owned.location, crate::inventory::Location::Bag(_))
        || if vigor {
            saved.progression.vigor == saved.progression.level().vigor
        } else {
            saved.progression.health == saved.progression.level().health
        }
    {
        return Err(invalid(
            "The potion must be in your bag and its vital must be below the maximum.",
        ));
    }
    let instance = scene.inventory_instance(character, item).map_err(other)?;
    let command = crate::inventory::Command::ConsumeDense { item };
    let changes = [if vigor {
        crate::progression::Command::RecoverVigor(25)
    } else {
        crate::progression::Command::Heal(crate::item_services::POTION_HEALTH)
    }];
    let next = saved
        .progression
        .apply(&changes[0])
        .map_err(io::Error::other)?;
    let physics = saved.instance;
    let recovered_health = if vigor {
        0
    } else {
        next.health.saturating_sub(saved.progression.health)
    };
    let mut events = vec![
        crate::item_services::removed(item).map_err(other)?,
        world::destroy_inventory_item(item, instance),
    ];
    events.extend(next.private_changes(&saved.progression));
    events.push(
        script::chat(
            &format!(
                "You recover {} {}.",
                if vigor {
                    next.vigor - saved.progression.vigor
                } else {
                    next.health - saved.progression.health
                },
                if vigor { "vigor" } else { "health" }
            ),
            1,
        )
        .map_err(other)?,
    );
    peer.validate_scene_batch(&events)?;
    store.player_transaction(&peer.session.account, character, &[command], &changes)?;
    scene.inventory_removed(character, item).map_err(other)?;
    peer.last_world_use_ms = Some(now);
    let mut updates = scene.player_stats(character, &next);
    if let Ok(recovered) = i32::try_from(recovered_health) {
        if recovered > 0 {
            updates.extend(publish_health_story(
                peer, scene, character, physics, character, recovered,
            ));
        }
    }
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}

/// `@quest`: the player's underway quests, each with its journal line (the quest tables).
pub(super) fn quest_status(peer: &mut Peer, character: u64, store: &Store) -> io::Result<()> {
    let book = &store.characters[&character].quests;
    let tables = crate::quest_emotes::installed()?;
    let lines: Vec<String> = tables.quests().filter_map(|q| {
        let stage = book.stage(q.journal_id);
        let (done, text) = tables.stage(q.journal_id, stage)?;
        (stage != 0 && !done).then(|| format!("{}: {}", q.title_text, text.replace("{count}", &book.count(q.journal_id).to_string()).replace("{goal}", &q.goal.to_string())))
    }).collect();
    let text = if lines.is_empty() { "You have no quest underway.".to_owned() } else { lines.join("\n") };
    peer.queue_ordered_batch(vec![script::chat(&text, 1).map_err(other)?])
}
