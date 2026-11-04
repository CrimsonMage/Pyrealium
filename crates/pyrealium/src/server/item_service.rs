//! Item service transactions (inventory and progression changes together).
use super::*;

/// Admit the complete native item/vital update before committing a service's
/// ownership and currency changes. Only the server's bounded grant/removal
/// plans are supported here; no client object description enters this path.
pub(super) fn item_service_transaction(
    peer: &mut Peer,
    character: u64,
    items: &[crate::inventory::Command],
    changes: &[crate::progression::Command],
    completion: Vec<Vec<u8>>,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    item_service_transaction_quest(peer, character, items, changes, completion, store, scene, metadata, false, false)
}
pub(super) fn item_service_transaction_quest(peer:&mut Peer,character:u64,items:&[crate::inventory::Command],changes:&[crate::progression::Command],completion:Vec<Vec<u8>>,store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,supplies:bool,practice:bool)->io::Result<Vec<crate::scene::Delivery>> {
    item_service_transaction_profile(peer,character,items,changes,completion,store,scene,metadata,supplies,practice,None,None)
}
pub(super) fn item_service_transaction_profile(
    peer: &mut Peer, character: u64,
    items: &[crate::inventory::Command], changes: &[crate::progression::Command],
    mut completion: Vec<Vec<u8>>, store: &mut Store, scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>, supplies: bool, practice: bool, emote:Option<&crate::quest_emotes::Outcome>, profession:Option<&crate::professions::Change>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if !scene.can_observe(character, character) || !saved.progression.alive() || peer.combat {
        return Err(invalid(
            "Item services require a ready living player out of combat.",
        ));
    }
    if items
        .iter()
        .filter(|item| matches!(item, crate::inventory::Command::Grant(_) | crate::inventory::Command::GrantFront(_)))
        .count()
        // A butchered corpse gives up to two traits.
        > if profession.is_some() || emote.is_some() {17} else {2}
    {
        return Err(invalid("Item service output limit exceeded."));
    }
    let first = items
        .iter()
        .any(|item| item.allocated_id().is_some())
        .then(|| store.next_item_id())
        .transpose()?;
    let (bag, state) = if items.is_empty() && changes.is_empty() {(saved.inventory.clone(),saved.progression.clone())} else {crate::player_transaction::candidate(
        &saved.inventory,
        &saved.progression,
        items,
        changes,
        first,
    )?};
    let mut events = Vec::new();
    let mut removed = Vec::new();
    let mut granted = Vec::new();
    let mut reduced_stacks = Vec::new();
    for command in items {
        match command {
            crate::inventory::Command::ToolWear {..} if profession.is_some() => {},
            crate::inventory::Command::ConsumeDense { item }
            | crate::inventory::Command::ConsumeStackDense { item, .. } => {
                let owned = saved
                    .inventory
                    .items()
                    .find(|owned| owned.id == *item)
                    .ok_or_else(|| invalid("Service item is not owned."))?;
                if !matches!(owned.location, crate::inventory::Location::Bag(_)) {
                    return Err(invalid("Service items must be in your bag."));
                }
                let instance = scene.inventory_instance(character, *item).map_err(other)?;
                events.push(crate::item_services::removed(*item).map_err(other)?);
                events.push(world::destroy_inventory_item(*item, instance));
                removed.push(*item);
            }
            crate::inventory::Command::ConsumePart { item, .. } => {
                // The stack stays with the rest: re-created with its new count, as a split or
                // a merge does (`merge_stacks`).
                let reduced = bag
                    .items()
                    .find(|owned| owned.id == *item)
                    .ok_or_else(|| invalid("Service item is not owned."))?;
                let old = scene.inventory_instance(character, *item).map_err(other)?;
                let new = scene.next_item_instance(character, *item).map_err(other)?;
                events.push(world::destroy_inventory_item(*item, old));
                events.push(world::create_inventory_item(reduced, character, new).map_err(other)?);
                reduced_stacks.push((*item, new));
            }
            crate::inventory::Command::Grant(item) | crate::inventory::Command::GrantFront(item) => {
                let item = bag
                    .items()
                    .find(|owned| owned.id == item.id)
                    .ok_or_else(|| invalid("Service grant absent from final inventory."))?;
                let info = metadata
                    .get(&item.definition)
                    .and_then(|info| *info)
                    .ok_or_else(|| invalid("Service item metadata unavailable."))?;
                let instance = scene
                    .admit_inventory_grant(character, item.id)
                    .map_err(other)?;
                events
                    .push(world::create_inventory_item(item, character, instance).map_err(other)?);
                events.push(crate::item_services::acquired(character, item, info).map_err(other)?);
                granted.push((item.id, instance));
            }
            _ => return Err(invalid("Unsupported item service transition.")),
        }
    }
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state) != saved.skills.available_credits(&saved.progression) {
        events.push(crate::ui::skills_state(&saved.skills, &state, &saved.skill_used).map_err(other)?);
    }
    events.append(&mut completion);
    if let Some(outcome)=emote {
        if crate::player_transaction::encode_items(items)!=crate::player_transaction::encode_items(&outcome.items) || crate::player_transaction::encode_changes(changes)!=crate::player_transaction::encode_changes(&outcome.changes) {return Err(invalid("Quest exchange differs from its emote outcome"));}
        let next=outcome.quests.iter().try_fold(saved.quests.clone(),|book,c|book.apply(c)).map_err(other)?;
        events.extend(next.updates_since(&saved.quests).map_err(other)?);
        for line in &outcome.lines {events.push(script::chat(line,1).map_err(other)?);}
    }
    peer.validate_scene_batch(&events)?;
    if let Some(outcome)=emote {store.emote_transaction(&peer.session.account,character,outcome)?;}
    else if let Some(change)=profession {store.profession_transaction(&peer.session.account,character,items,changes,change)?;}
    else {store.player_transaction_quest(&peer.session.account, character, items, changes, supplies, practice)?;}
    for item in removed {
        scene.inventory_removed(character, item).map_err(other)?;
    }
    for (item, instance) in reduced_stacks {
        scene.apparel_changed(character, item, instance).map_err(other)?;
    }
    for (item, instance) in granted {
        scene
            .inventory_granted(character, item, instance)
            .map_err(other)?;
    }
    let updates = scene.player_stats(character, &state);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
