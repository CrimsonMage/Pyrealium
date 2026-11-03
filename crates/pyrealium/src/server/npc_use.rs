//! Quest emotes on world objects: Use, Ask and Talk sets and their committed outcomes.
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
    let used=item.map(|id|saved.inventory.items().find(|i|i.id==id && matches!(i.location,crate::inventory::Location::Bag(_))).map(|i|i.definition)
        .ok_or_else(||invalid("Use an item from your backpack."))).transpose()?;
    run_emote(peer,character,target,trigger,used,store,scene,metadata,encounters,bootstrap,now)
}

/// `@ask <topic>` (and `@mentor <topic>`): the nearest NPC within four metres with Ask sets
/// answers the topic (`quest_emotes::topic_key`).
#[allow(clippy::too_many_arguments)]
pub(super) fn ask_emote(peer:&mut Peer,character:u64,topic:&str,store:&mut Store,
    scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,
    encounters:&mut Option<crate::encounters::Encounters>,bootstrap:&mut Option<crate::bootstrap::Bootstrap>,now:u64)->io::Result<Vec<crate::scene::Delivery>> {
    let tables=crate::quest_emotes::installed()?;
    let target=scene.nearby_object(character,|entity,iid|tables.reacts(entity,iid,crate::quest_emotes::Trigger::Ask))
        .ok_or_else(||invalid("No one within four metres answers questions."))?;
    run_emote(peer,character,target,crate::quest_emotes::Trigger::Ask,Some(crate::quest_emotes::topic_key(topic)),store,scene,metadata,encounters,bootstrap,now)
}

/// Runs `trigger` on the object for the player, `used` being the set's item (an item's
/// definition, or an Ask topic's key).
#[allow(clippy::too_many_arguments)]
fn run_emote(peer:&mut Peer,character:u64,target:u64,trigger:crate::quest_emotes::Trigger,used:Option<u32>,store:&mut Store,
    scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,
    encounters:&mut Option<crate::encounters::Encounters>,bootstrap:&mut Option<crate::bootstrap::Bootstrap>,now:u64)->io::Result<Vec<crate::scene::Delivery>> {
    let saved=&store.characters[&character];
    if peer.combat || !saved.progression.alive() {return Err(invalid("Do that while alive and out of combat."));}
    let object=scene.examination_content(character,target).ok_or_else(||invalid("That is not visible."))?;
    let roll=(now % 1000) as f32/1000.;
    let outcome=crate::quest_emotes::installed()?.run(trigger,object.entity,target,used,&saved.quests,&saved.inventory,&saved.progression,store.next_item_id()?,roll,crate::clock::now(),&crate::quest_emotes::item)?
        .ok_or_else(||invalid("Nothing happens."))?;
    let player=scene.player_placement(character).ok_or_else(||invalid("Player is not ready."))?;
    let distance=player.distance(object.placement).filter(|d|*d<=outcome.range)
        .ok_or_else(||invalid(&format!("Move within {} metres.",outcome.range)))?;
    let events=vec![crate::interaction::success(character,target,player,distance).map_err(other)?];
    if outcome.moveto.is_some() && peer.last_world_use_ms.is_some_and(|last|now.saturating_sub(last)<5000) {return Err(invalid("Wait a few seconds."));}
    peer.admit_npc_dialogue(now)?;
    let speaker=object.name.clone().unwrap_or_default();
    commit_emote(peer,character,&outcome,events,Some((target,&speaker)),store,scene,metadata,encounters,bootstrap,now)
}

/// The client lines of a committed outcome beyond its items and stages: the NPC's Say lines
/// (speech from `speaker` when one stands there, and the dialogue window), and the scenes and
/// recipes the quest book gained.
pub(super) fn outcome_lines(outcome:&crate::quest_emotes::Outcome,speaker:Option<(u64,&str)>,before:&crate::quests::Book,after:&crate::quests::Book)
    ->io::Result<Vec<Vec<u8>>> {
    let mut events=Vec::new();
    for (name,text) in &outcome.says {
        let name=name.as_deref().or(speaker.map(|(_,n)|n)).unwrap_or("");
        if let Some((iid,_))=speaker {events.push(crate::interaction::speech(iid,name,text).map_err(other)?);}
        events.push(crate::interaction::dialogue(name,text).map_err(other)?);
    }
    for scene in after.scenes().filter(|s|!before.scenes().any(|b|b==*s)) {events.push(crate::story::granted(scene).map_err(other)?);}
    for recipe in outcome.quests.iter().filter_map(|q|if let crate::quests::Command::Recipe(r)=q {Some(*r)} else {None}).filter(|r|!before.knows(*r)) {
        events.push(crate::crafting::learned_recipe(recipe).map_err(other)?);
        events.push(crate::crafting::shown_recipe(recipe).map_err(other)?);
    }
    Ok(events)
}

/// Commits a quest emote outcome for the player (alive, out of combat): its lines, items,
/// experience, effects, escort and move, as one record; a set that paused for a delay is queued
/// (`quest_emotes::pending`). `events` go first (the use's own answer).
#[allow(clippy::too_many_arguments)]
pub(super) fn commit_emote(peer:&mut Peer,character:u64,outcome:&crate::quest_emotes::Outcome,mut events:Vec<Vec<u8>>,speaker:Option<(u64,&str)>,store:&mut Store,
    scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,
    encounters:&mut Option<crate::encounters::Encounters>,bootstrap:&mut Option<crate::bootstrap::Bootstrap>,now:u64)->io::Result<Vec<crate::scene::Delivery>> {
    // A DAT line from an NPC is its speech; with no speaker it is a chat line.
    for (table,key) in &outcome.tells {events.push(match speaker {
        Some((iid,name)) if !name.is_empty() => crate::interaction::speech_localized(iid,name,*table,*key),
        _ => crate::quest_emotes::tell(*table,*key),
    }.map_err(other)?);}
    let before=store.characters[&character].quests.clone();
    let after=outcome.quests.iter().try_fold(before.clone(),|b,c|b.apply(c)).map_err(other)?;
    events.extend(outcome_lines(outcome,speaker,&before,&after)?);
    let prepared=outcome.moveto.map(|to|prepare_forced_move(peer,character,to,scene,bootstrap)).transpose()?;
    if let Some(prepared)=&prepared {
        events.push(prepared.wire.clone());
        events.push(script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?);
    }
    let mut updates=item_service_transaction_profile(peer,character,&outcome.items,&outcome.changes,events,store,scene,metadata,false,false,Some(outcome),None)?;
    crate::quest_emotes::instances::committed(character,outcome);
    // The quest's effects (ApplyEffect / RemoveEffect) reach the client's strip (`WM_Effect`);
    // an escort starts or ends.
    if let Some(runtime)=encounters.as_mut() {
        let unix=crate::clock::now();
        runtime.sync_quest_effects(character,&store.characters[&character].quests.quest_effects(unix),unix,now,true);
        if let Some(entity)=outcome.escort {
            if runtime.start_escort(character,entity,scene).is_none() {events_note(peer,"Your escort is not nearby.")?;}
        }
        if outcome.end_escort {runtime.end_escort(character);}
        if let Some((iid,seconds))=outcome.hold {runtime.start_hold(character,iid,seconds,now);}
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
    let unix=crate::clock::now();
    for definition in made {
        let saved=&store.characters[&character];
        if !tables.answers(trigger,*definition,0,None,&saved.quests,unix) {continue;}
        let Some(outcome)=tables.run(trigger,*definition,0,None,&saved.quests,&saved.inventory,&saved.progression,
            store.next_item_id()?,(now % 1000) as f32/1000.,unix,&crate::quest_emotes::item)? else {continue};
        let mut events=Vec::new();
        for (table,key) in &outcome.tells {events.push(crate::quest_emotes::tell(*table,*key).map_err(other)?);}
        let before=saved.quests.clone();
        let after=outcome.quests.iter().try_fold(before.clone(),|b,c|b.apply(c)).map_err(other)?;
        events.extend(outcome_lines(&outcome,None,&before,&after)?);
        updates.extend(item_service_transaction_profile(peer,character,&outcome.items,&outcome.changes,events,store,scene,metadata,false,false,Some(&outcome),None)?);
        crate::quest_emotes::instances::committed(character,&outcome);
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



