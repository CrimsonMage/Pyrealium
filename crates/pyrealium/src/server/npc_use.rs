//! Emotes, NPC dialogue and the training mentor.
use super::*;

// Explicit development services assigned to these DAT-backed NPC types.
// Placements are discovered from the live scene, not a fixed list of IIDs.
pub(super) const TRAINING_MENTOR_ENTITY: u32 = crate::training_dialogue::MENTOR_ENTITY;

/// One trigger of the quest emote tables (`quest_emotes`) on a world object for a player: the
/// matching set's lines, quest stages, items, experience and move, committed as one record.
#[allow(clippy::too_many_arguments)]
pub(super) fn use_emote(peer:&mut Peer,character:u64,target:u64,trigger:crate::quest_emotes::Trigger,item:Option<u64>,store:&mut Store,
    scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,
    encounters:&mut Option<crate::encounters::Encounters>,bootstrap:&mut Option<crate::bootstrap::Bootstrap>,now:u64)->io::Result<Vec<crate::scene::Delivery>> {
    let saved=&store.characters[&character];
    if peer.combat || !saved.progression.alive() {return Err(invalid("Do that while alive and out of combat."));}
    let object=scene.examination_content(character,target).ok_or_else(||invalid("That is not visible."))?;
    let used=item.map(|id|saved.inventory.items().find(|i|i.id==id && matches!(i.location,crate::inventory::Location::Bag(_))).map(|i|i.definition)
        .ok_or_else(||invalid("Use an item from your backpack."))).transpose()?;
    let roll=(now % 1000) as f32/1000.;
    let outcome=crate::quest_emotes::installed()?.run(trigger,object.entity,target,used,&saved.quests,&saved.inventory,&saved.progression,store.next_item_id()?,roll,crate::recovered_quests::now(),&crate::quest_emotes::item)?
        .ok_or_else(||invalid("Nothing happens."))?;
    let player=scene.player_placement(character).ok_or_else(||invalid("Player is not ready."))?;
    let distance=player.distance(object.placement).filter(|d|*d<=outcome.range)
        .ok_or_else(||invalid(&format!("Move within {} metres.",outcome.range)))?;
    let mut events=vec![crate::interaction::success(character,target,player,distance).map_err(other)?];
    if outcome.moveto.is_some() && peer.last_world_use_ms.is_some_and(|last|now.saturating_sub(last)<5000) {return Err(invalid("Wait a few seconds."));}
    for (table,key) in &outcome.tells {events.push(crate::quest_emotes::tell(*table,*key).map_err(other)?);}
    peer.admit_npc_dialogue(now)?;
    let prepared=outcome.moveto.map(|to|prepare_forced_move(peer,character,to,scene,bootstrap)).transpose()?;
    if let Some(prepared)=&prepared {
        events.push(prepared.wire.clone());
        events.push(script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?);
    }
    let mut updates=item_service_transaction_profile(peer,character,&outcome.items,&outcome.changes,events,store,scene,metadata,false,false,Some(&outcome),None,None)?;
    // The quest's effects (ApplyEffect / RemoveEffect) reach the client's strip (`WM_Effect`);
    // an escort starts or ends.
    if let Some(runtime)=encounters.as_mut() {
        let unix=crate::recovered_quests::now();
        runtime.sync_quest_effects(character,&store.characters[&character].quests.quest_effects(unix),unix,now,true);
        if let Some(entity)=outcome.escort {
            if runtime.start_escort(character,entity,scene).is_none() {events_note(peer,"Your escort is not nearby.")?;}
        }
        if outcome.end_escort {runtime.end_escort(character);}
    }
    if let Some(prepared)=prepared {
        if let Some(runtime)=encounters {runtime.stop(character);}
        updates.extend(scene.update(character,prepared.motion.position_stamp,&prepared.update).map_err(other)?);
        peer.motion=Some(prepared.motion);peer.target=None;peer.last_world_use_ms=Some(now);
    }
    Ok(updates)
}

/// A craft's quest emotes (`quest_emotes` Crafted sets): each item the craft made (`made`, entity
/// DIDs) runs the set named for it, committed as one record each.
#[allow(clippy::too_many_arguments)]
pub(super) fn crafted_emotes(peer:&mut Peer,character:u64,made:&[u32],store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,encounters:&mut Option<crate::encounters::Encounters>,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    trigger_emotes(peer,character,crate::quest_emotes::Trigger::Crafted,made,store,scene,metadata,encounters,now)
}
/// Runs the sets of `trigger` named for each of `entities` (no world object: a craft, an escort's
/// loss) for the player, each committed as one record.
#[allow(clippy::too_many_arguments)]
pub(super) fn trigger_emotes(peer:&mut Peer,character:u64,trigger:crate::quest_emotes::Trigger,made:&[u32],store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,encounters:&mut Option<crate::encounters::Encounters>,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    let Some(tables)=crate::quest_emotes::tables() else {return Ok(Vec::new())};
    let mut updates=Vec::new();
    let unix=crate::recovered_quests::now();
    for definition in made {
        let saved=&store.characters[&character];
        if !tables.answers(trigger,*definition,0,None,&saved.quests,unix) {continue;}
        let Some(outcome)=tables.run(trigger,*definition,0,None,&saved.quests,&saved.inventory,&saved.progression,
            store.next_item_id()?,(now % 1000) as f32/1000.,unix,&crate::quest_emotes::item)? else {continue};
        let mut events=Vec::new();
        for (table,key) in &outcome.tells {events.push(crate::quest_emotes::tell(*table,*key).map_err(other)?);}
        updates.extend(item_service_transaction_profile(peer,character,&outcome.items,&outcome.changes,events,store,scene,metadata,false,false,Some(&outcome),None,None)?);
        if let Some(runtime)=encounters.as_mut() {
            runtime.sync_quest_effects(character,&store.characters[&character].quests.quest_effects(unix),unix,now,true);
        }
    }
    Ok(updates)
}

pub(super) fn events_note(peer:&mut Peer,text:&str)->io::Result<()> {
    peer.queue_ordered_batch(vec![script::chat(text,1).map_err(other)?])
}

pub(super) fn npc_use_completion(
    owner: u64,
    target: u64,
    scene: &crate::scene::Scene,
) -> io::Result<Vec<u8>> {
    let player = scene
        .player_placement(owner)
        .ok_or_else(|| invalid("Player is not ready."))?;
    let npc = scene
        .examination_content(owner, target)
        .ok_or_else(|| invalid("NPC is not visible."))?;
    let distance = player.distance(npc.placement).filter(|d| *d <= 4.0);
    let Some(distance) = distance else {
        return Err(invalid("Move within four metres of the NPC."));
    };
    crate::interaction::success(owner, target, player, distance).map_err(other)
}
pub(super) fn npc_interaction_events(owner: u64, target: u64, scene: &crate::scene::Scene,
    name: &str, text: &str) -> io::Result<Vec<Vec<u8>>> {
    Ok(vec![npc_use_completion(owner,target,scene)?,
        crate::interaction::speech(target,name,text).map_err(other)?])
}

pub(super) fn use_npc_dialogue(peer:&mut Peer, owner:u64,target:u64,store:&Store,scene:&crate::scene::Scene,now:u64)
    ->io::Result<()> {
    if peer.combat || !store.characters[&owner].progression.alive() {
        return Err(invalid("Speak to the NPC while alive and out of combat."));
    }
    let object=scene.examination_content(owner,target).ok_or_else(||invalid("NPC is not visible."))?;
    let profile=crate::npc_dialogue::profiles().get(&object.entity).ok_or_else(||invalid("Unknown NPC."))?;
    let completion=npc_use_completion(owner,target,scene)?;
    peer.admit_npc_dialogue(now)?;
    let text=profile.reply(peer.npc_dialogue.page(owner,target));
    peer.queue_ordered_batch(vec![completion,
        crate::interaction::speech(target,&profile.name,&text).map_err(other)?,
        crate::interaction::dialogue(&profile.name,&text).map_err(other)?])?;
    peer.npc_dialogue.advance(owner,target,profile.topics.len());
    Ok(())
}

pub(super) fn mentor_interaction_events(owner: u64, target: u64, scene: &crate::scene::Scene, text: &str) -> io::Result<Vec<Vec<u8>>> {
    let mut events = npc_interaction_events(owner, target, scene, "Pectar, Training Master", text)?;
    events.push(crate::interaction::dialogue("Pectar, Training Master", text).map_err(other)?);
    Ok(events)
}

pub(super) fn use_training_mentor(
    peer: &mut Peer,
    character: u64,
    target: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
    now: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let player = scene
        .player_placement(character)
        .ok_or_else(|| invalid("Player is not ready."))?;
    let mentor = scene
        .examination_content(character, target)
        .filter(|object| object.entity == TRAINING_MENTOR_ENTITY)
        .ok_or_else(|| invalid("That training mentor is not visible."))?;
    if !player.distance(mentor.placement).is_some_and(|distance| distance <= 4.0) {
        return Err(invalid("Move within four metres of the training mentor."));
    }
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid(
            "Speak to the training mentor while alive and out of combat.",
        ));
    }
    peer.admit_npc_dialogue(now)?;
    let book = &store.characters[&character].quests;
    if book.journey_phase() != 0 {
        return quest_action(peer, character, if book.journey_phase() == 3 { "journey claim" } else { "status" }, store, scene, Some(target));
    }
    if book.potion_phase() == 3 {
        return quest_action(peer, character, "journey", store, scene, Some(target));
    }
    if book.potion_phase() != 0 {
        return quest_action(peer, character, if book.potion_phase() == 2 { "potion claim" } else { "status" }, store, scene, Some(target));
    }
    if book.supplies() == Some(true) {
        return quest_action(peer, character, "potion", store, scene, Some(target));
    }
    if book.get().is_some_and(|state| state.claimed) {
        if book.supplies().is_none() {
            return quest_action(peer, character, "supplies", store, scene, Some(target));
        }
        if book.supplies() == Some(false) {
            if let Ok(items) = crate::quests::supplies_items(&store.characters[&character].inventory, store.next_item_id()?) {
                let changes = crate::quests::rewards(&crate::quests::Command::ClaimSupplies);
                let next = book.apply(&crate::quests::Command::ClaimSupplies).map_err(other)?;
                let events = mentor_interaction_events(character, target, scene, &next.guidance())?;
                return item_service_transaction_quest(peer, character, &items, &changes, events, store, scene, metadata, true, false);
            }
        }
    }
    let action = if book.get().is_none() {
        "start"
    } else if book.ready() {
        "claim"
    } else {
        "status"
    };
    quest_action(peer, character, action, store, scene, Some(target))
}
