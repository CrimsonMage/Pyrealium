//! Quest items and NPCs used in the world: elixirs, deliveries, training potions, quest actions.
use super::*;

pub(super) fn use_recovered_elixir(peer:&mut Peer,character:u64,item:u64,store:&mut Store,scene:&mut crate::scene::Scene)->io::Result<()> {
    if !scene.can_observe(character,character)||peer.combat{return Err(invalid("Leave combat before drinking a quest elixir."));}
    let c=&store.characters[&character];let account=c.account.clone();
    let owned=c.inventory.items().find(|i|i.id==item).ok_or_else(||invalid("Elixir not owned"))?;
    let q=crate::recovered_quests::use_profile(owned.definition).ok_or_else(||invalid("Unsupported quest elixir"))?;
    let instance=scene.inventory_instance(character,item).map_err(other)?;
    let mut quests=c.quests.clone();
    quests.recovered=quests.recovered.accept(q.id,c.progression.level().number,crate::recovered_quests::now())?;
    let mut events=vec![crate::item_services::removed(item).map_err(other)?,world::destroy_inventory_item(item,instance)];
    events.extend(quests.updates_since(&c.quests).map_err(other)?);
    events.push(script::chat(&format!("{} accepted. Defeat {} targets before the time limit.",q.title,q.goal),1).map_err(other)?);
    peer.validate_scene_batch(&events)?;
    store.recovered_elixir(&account,character,item)?;
    scene.inventory_removed(character,item).map_err(other)?;peer.queue_ordered_batch(events)?;Ok(())
}

pub(super) fn arwic_exchange(peer:&mut Peer,character:u64,action:crate::arwic_mines::Action,events:Vec<Vec<u8>>,store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>)->io::Result<Vec<crate::scene::Delivery>> {
    let saved=&store.characters[&character];let time=crate::recovered_quests::now();
    let plan=crate::arwic_mines::plan(&saved.quests,&saved.inventory,action,store.next_item_id()?,time)?;
    item_service_transaction_profile(peer,character,&plan.items,&plan.rewards,events,store,scene,metadata,false,false,None,None,Some((action,time)))
}

pub(super) fn use_exploration_npc(peer:&mut Peer,character:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid("Speak to the quest NPC while alive and out of combat."));
    }
    let completion=npc_use_completion(character,target,scene)?;
    let entity=scene.examination_content(character,target).ok_or_else(||invalid("Quest object is not visible."))?.entity;
    peer.admit_npc_dialogue(now)?;
    let saved=&store.characters[&character];let account=saved.account.clone();
    let time=crate::recovered_quests::now();
    let conversation=saved.quests.exploration.converse(entity,saved.progression.level().number,time)?;
    let mut quests=saved.quests.clone();quests.exploration=conversation.book;
    let state=if conversation.xp>0 {saved.progression.apply(&crate::progression::Command::AwardXp(conversation.xp)).map_err(other)?}
        else {saved.progression.clone()};
    let speaker=crate::exploration_quests::speaker(entity);
    let mut events=vec![completion,crate::interaction::speech(target,speaker,&conversation.text).map_err(other)?,
        crate::interaction::dialogue(speaker,&conversation.text).map_err(other)?];
    events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state)!=saved.skills.available_credits(&saved.progression) {
        events.push(crate::ui::skills_state(&saved.skills,&state,&saved.skill_used).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    store.exploration_conversation(&account,character,entity,time)?;
    let updates=scene.player_stats(character,&state);peer.queue_ordered_batch(events)?;Ok(updates)
}

pub(super) fn use_hunting_npc(peer:&mut Peer,character:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid("Speak to the quest NPC while alive and out of combat."));
    }
    let completion=npc_use_completion(character,target,scene)?;
    let entity=scene.examination_content(character,target).ok_or_else(||invalid("Quest object is not visible."))?.entity;
    peer.admit_npc_dialogue(now)?;
    let saved=&store.characters[&character];let account=saved.account.clone();
    let time=crate::recovered_quests::now();
    let conversation=saved.quests.hunting.converse(entity,&saved.progression,time)?;
    let mut quests=saved.quests.clone();quests.hunting=conversation.book;
    let state=if conversation.xp>0 {saved.progression.apply(&crate::progression::Command::AwardXp(conversation.xp)).map_err(other)?}
        else {saved.progression.clone()};
    let speaker=crate::hunting_quests::speaker(entity);
    let mut events=vec![completion,crate::interaction::speech(target,speaker,&conversation.text).map_err(other)?,
        crate::interaction::dialogue(speaker,&conversation.text).map_err(other)?];
    events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state)!=saved.skills.available_credits(&saved.progression) {
        events.push(crate::ui::skills_state(&saved.skills,&state,&saved.skill_used).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    store.hunting_conversation(&account,character,entity,time)?;
    let updates=scene.player_stats(character,&state);peer.queue_ordered_batch(events)?;Ok(updates)
}

pub(super) fn use_recovered_delivery(peer:&mut Peer,character:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>)->io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat||!store.characters[&character].progression.alive(){return Err(invalid("Speak to the quest NPC while alive and out of combat."));}
    let npc=scene.examination_content(character,target).ok_or_else(||invalid("Quest NPC is not visible."))?.entity;
    let completion=npc_use_completion(character,target,scene)?;
    let saved=&store.characters[&character];let account=saved.account.clone();let time=crate::recovered_quests::now();
    let first=store.next_item_id()?;
    let (items,changes,book)=saved.quests.recovered.delivery(&saved.inventory,npc,saved.progression.level().number,time,first)?;
    let (bag,state)=if items.is_empty()&&changes.is_empty(){(saved.inventory.clone(),saved.progression.clone())}else{
        crate::player_transaction::candidate(&saved.inventory,&saved.progression,&items,&changes,Some(first))?};
    let mut quests=saved.quests.clone();quests.recovered=book;
    let mut events=vec![completion];let mut granted=Vec::new();let mut removed=Vec::new();
    for command in &items {match command {
        crate::inventory::Command::Grant(item)=>{
            let info=metadata.get(&item.definition).and_then(|i|*i).ok_or_else(||invalid("Quest bag metadata unavailable"))?;
            let instance=scene.admit_inventory_grant(character,item.id).map_err(other)?;
            events.push(world::create_inventory_item(item,character,instance).map_err(other)?);
            events.push(crate::item_services::acquired(character,item,info).map_err(other)?);granted.push((item.id,instance));
        },
        crate::inventory::Command::ConsumeDense{item}=>{
            let instance=scene.inventory_instance(character,*item).map_err(other)?;
            events.push(crate::item_services::removed(*item).map_err(other)?);events.push(world::destroy_inventory_item(*item,instance));removed.push(*item);
        },_=>return Err(invalid("Unsupported delivery inventory transition"))
    }}
    events.extend(state.private_changes(&saved.progression));
    events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    events.push(script::chat(if npc==crate::recovered_quests::KANTEN {"Gambling Debt: deliver Kanten's Bag to Orven in Arwic (44.9N, 7.1W) within two hours."}
        else {"Gambling Debt complete. You receive 1,000 experience."},1).map_err(other)?);
    if saved.skills.available_credits(&state)!=saved.skills.available_credits(&saved.progression){events.push(crate::ui::skills_state(&saved.skills,&state,&saved.skill_used).map_err(other)?);}
    peer.validate_scene_batch(&events)?;
    store.recovered_delivery(&account,character,npc,time)?;
    debug_assert_eq!(store.characters[&character].inventory,bag);
    for item in removed {scene.inventory_removed(character,item).map_err(other)?;}
    for (item,instance) in granted {scene.inventory_granted(character,item,instance).map_err(other)?;}
    let updates=scene.player_stats(character,&state);peer.queue_ordered_batch(events)?;Ok(updates)
}

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

pub(super) fn quest_action(
    peer: &mut Peer,
    character: u64,
    action: &str,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    speaker: Option<u64>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    if !scene.can_observe(character, character) {
        return Err(invalid("Player is not ready."));
    }
    let saved = &store.characters[&character];
    if action.trim().eq_ignore_ascii_case("shelter") {
        let quest=crate::quest_emotes::installed()?.quest("shelter").ok_or_else(||invalid("no shelter quest"))?.journal_id;
        let text=crate::quest_emotes::installed()?.stage(quest,saved.quests.stage(quest)).map_or("",|(_,t)|t);
        peer.queue_ordered_batch(vec![script::chat(text,1).map_err(other)?])?;
        return Ok(Vec::new());
    }
    let command = match action.trim().to_ascii_lowercase().as_str() {
        "" | "status" => None,
        "start" => Some(crate::quests::Command::Accept),
        "supplies" => Some(crate::quests::Command::AcceptSupplies),
        "potion" => Some(crate::quests::Command::AcceptPotion),
        "potion claim" => Some(crate::quests::Command::ClaimPotion),
        "journey" => Some(crate::quests::Command::AcceptJourney),
        "journey claim" => Some(crate::quests::Command::ClaimJourney),
        "claim" => Some(crate::quests::Command::Claim),
        _ => return Err(invalid("Use @quest for status; start, claim, supplies, potion, potion claim, journey or journey claim for lessons.")),
    };
    if command.is_some() && (!saved.progression.alive() || peer.combat) {
        return Err(invalid(
            "Quest acceptance and rewards require a living player out of combat.",
        ));
    }
    let quests = match command.as_ref() {
        Some(command) => saved.quests.apply(command).map_err(io::Error::other)?,
        None => saved.quests.clone(),
    };
    let mut state = saved.progression.clone();
    if let Some(command) = &command {
        for reward in crate::quests::rewards(command) {
            state = state.apply(&reward).map_err(io::Error::other)?;
        }
    }
    let text = quests.guidance();
    let mut events = match speaker {
        Some(speaker) => {
            let race = saved.creation.as_ref().map_or(1, |request| request.race);
            mentor_interaction_events(character, speaker, scene, &crate::training_dialogue::greeting(race, &quests))?
        }
        None => vec![script::chat(&text, 1).map_err(other)?],
    };
    if quests.get().is_some() {
        events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    }
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state) != saved.skills.available_credits(&saved.progression)
    {
        events
            .push(crate::ui::skills_state(&saved.skills, &state, &saved.skill_used).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    if let Some(command) = command {
        store.quest_command(&peer.session.account, character, command)?;
    }
    let updates = scene.player_stats(character, &state);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
