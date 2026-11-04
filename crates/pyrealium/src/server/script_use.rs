//! The use-object script call (`script::Request::UseObject`).
use super::*;

pub(super) fn on_use_object(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::equipment::UseRequest,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if request.item>>48==0x8190 || srv.store.characters[&character].inventory.items().any(|i|i.id==request.item&&(crate::harvesting::map(i.definition).is_some()||crate::harvesting::tool(i.definition).is_some())){
        if request.owner!=character{return Err(invalid("Gathering owner mismatch"))}
        match use_gathering(peer,character,request.item,request.target,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,&srv.corpse_loot,srv.bootstrap.as_mut().and_then(|b|b.geometry.as_mut()),srv.now_ms){
            Ok(updates)=>outbox.deliveries.extend(updates),Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
        }return Ok(replies);
    }
    if let Some(assets) = &srv.trade_assets {
        let map = trade_actions::is_map(&srv.store.characters[&character].inventory, request.item);
        if request.owner == character && request.target == 0 && (map || peer.deposits.contains_key(&request.item)) {
            let result = if !srv.scene.can_observe(character, character) || !srv.store.characters[&character].progression.alive() {
                Err(invalid("Use tradeskills with your living character."))
            } else if map {
                trade_actions::use_map(peer, character, request.item, assets, &mut srv.store, &mut srv.scene, &srv.inventory_metadata, &srv.examination_names)
            } else {
                trade_actions::mine(peer, character, request.item, assets, &mut srv.store, &mut srv.scene, &srv.inventory_metadata)
            };
            match result {
                Ok(updates) => outbox.deliveries.extend(updates),
                Err(error) => peer.queue_ordered_batch(vec![script::chat(&error.to_string(), 1).map_err(other)?])?,
            }
            return Ok(replies);
        }
        // A hunting knife used on a corpse (`butchery.rs`).
        if request.owner == character && request.target != 0 && trade_actions::is_knife(&srv.store.characters[&character].inventory, request.item) {
            let result = if !srv.scene.can_observe(character, character) || !srv.store.characters[&character].progression.alive() {
                Err(invalid("Butcher with your living character."))
            } else {
                trade_actions::butcher(peer, character, request.target, assets, &mut srv.store, &mut srv.scene, &srv.inventory_metadata)
            };
            match result {
                Ok(updates) => {
                    srv.corpse_loot.butchered(request.target, character);
                    outbox.deliveries.extend(updates)
                }
                Err(error) => peer.queue_ordered_batch(vec![script::chat(&error.to_string(), 1).map_err(other)?])?,
            }
            return Ok(replies);
        }
    }
    if srv.corpse_loot.contains(request.item,srv.now_ms) || srv.scene.examination_content(character,request.item)
        .is_some_and(|o|matches!(o.service,Some(crate::services::Service::Chest{..}))){
        if request.owner!=character{return Err(invalid("Container owner mismatch"));}
        match corpse_action(peer,character,crate::loot::Request::Open(request.item),&mut srv.corpse_loot,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,srv.now_ms){
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Loot rejected: {error}"),1).map_err(other)?])?,
        }
        return Ok(replies);
    }
    if request.item>>48==0x8180 {
        if !srv.config.initialize_ui || request.owner!=character || request.target!=0 {return Err(invalid("Invalid ground item pickup."));}
        match pickup_ground_item(peer,character,request.item,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,srv.now_ms) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Pickup rejected: {error}"),1).map_err(other)?])?,
        }
        return Ok(replies);
    }
    // Quest emotes (`quest_emotes`): a world object with a Use set that answers now. The
    // quest tables go first; the built-in quests below answer only what they do not.
    if srv.shelter_gameplay && srv.scene.examination_content(character,request.item).is_some_and(|o|crate::quest_emotes::tables().is_some_and(|t|
        t.answers(crate::quest_emotes::Trigger::Use,o.entity,request.item,None,&srv.store.characters[&character].quests,crate::recovered_quests::now()))) {
        if request.owner != character {return Err(invalid("Quest use owner mismatch."));}
        match use_emote(peer,character,request.item,crate::quest_emotes::Trigger::Use,None,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,&mut srv.encounters,&mut srv.bootstrap,srv.now_ms) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,srv.now)?),
        }
        return Ok(replies);
    }
    let quest_target=if request.target!=0 && srv.store.characters[&character].inventory.items().any(|i|i.id==request.item&&i.definition==crate::recovered_quests::BAG) {request.target}else{request.item};
    if srv.quest_gameplay && srv.scene.examination_content(character,quest_target).is_some_and(|o|crate::recovered_quests::delivery_npc(o.entity)) {
        if request.owner!=character{return Err(invalid("Quest use owner mismatch"));}
        match use_recovered_delivery(peer,character,quest_target,&mut srv.store,&mut srv.scene,&srv.inventory_metadata) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
        }
        return Ok(replies);
    }
    let arwic_target=if request.target!=0 && srv.store.characters[&character].inventory.items().any(|i|
        i.id==request.item && matches!(i.definition,crate::arwic_mines::HAMMER|crate::arwic_mines::SWORD) && matches!(i.location,crate::inventory::Location::Bag(_))) {request.target}else{request.item};
    if srv.quest_gameplay && srv.scene.examination_content(character,arwic_target).is_some_and(|o|o.entity==crate::arwic_mines::NPC) {
        if request.owner!=character || (request.target!=0 && arwic_target!=request.target) {return Err(invalid("Quest use owner or target mismatch."));}
        let result=(|| {
            let event=npc_use_completion(character,arwic_target,&srv.scene)?;
            peer.admit_npc_dialogue(srv.now_ms)?;
            arwic_exchange(peer,character,crate::arwic_mines::Action::Talk,vec![event],&mut srv.store,&mut srv.scene,&srv.inventory_metadata)
        })();
        match result {Ok(updates)=>outbox.deliveries.extend(updates),Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,}
        return Ok(replies);
    }
    // A bag item used on a world object with quest emotes for it (`ItemUsedOn`).
    let used_on=(request.target!=0).then(||srv.store.characters[&character].inventory.items().find(|i|i.id==request.item && matches!(i.location,crate::inventory::Location::Bag(_))).map(|i|i.definition)).flatten()
        .filter(|_|srv.scene.examination_content(character,request.target).is_some_and(|o|crate::quest_emotes::tables().is_some_and(|t|t.reacts(o.entity,request.target,crate::quest_emotes::Trigger::ItemUsedOn))));
    if request.target!=0 && !(srv.shelter_gameplay && used_on.is_some()) {
        return Err(invalid("Targeted use is only supported for a quest item on its quest object."));
    }
    let service_entity = srv.scene.examination_content(character, request.item)
        .map(|object| object.entity);
    if srv.quest_gameplay && srv.store.characters[&character].inventory.items().any(|i|
        i.id==request.item && i.definition==crate::arwic_mines::GUIDE && matches!(i.location,crate::inventory::Location::Bag(_))) {
        if request.owner!=character || !srv.store.characters[&character].progression.alive() {return Err(invalid("Read a book from your living character's backpack."));}
        peer.admit_npc_dialogue(srv.now_ms)?;
        peer.queue_ordered_batch(vec![crate::arwic_mines::read_page(peer.guide_page).map_err(other)?])?;
        peer.guide_page=(peer.guide_page+1)%crate::arwic_mines::content().guide_pages.len();
        return Ok(replies);
    }
    if srv.quest_gameplay && service_entity.is_some_and(crate::exploration_quests::supported) {
        if request.owner!=character || request.target!=0 {return Err(invalid("Quest use owner or target mismatch."));}
        match use_exploration_npc(peer,character,request.item,&mut srv.store,&mut srv.scene,srv.now_ms) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
        }
        return Ok(replies);
    }
    if srv.quest_gameplay && service_entity.is_some_and(crate::hunting_quests::supported) {
        if request.owner!=character || request.target!=0 {return Err(invalid("Quest use owner or target mismatch."));}
        match use_hunting_npc(peer,character,request.item,&mut srv.store,&mut srv.scene,srv.now_ms) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
        }
        return Ok(replies);
    }
    if srv.scene.examination_content(character,request.item).is_some_and(|o|o.service==Some(crate::services::Service::Sign)) {
        if request.owner!=character || !srv.store.characters[&character].progression.alive(){return Err(invalid("Use the sign with your living character."));}
        if !srv.scene.combat_distance(character,request.item).is_some_and(|d|d<=4.) {
            peer.queue_ordered_batch(vec![script::chat("Move within four metres to read this sign.",1).map_err(other)?])?;
        } else if let Some(message)=service_entity.and_then(|id|srv.examination_names.description_chat(id)) {
            peer.queue_ordered_batch(vec![message.map_err(other)?])?;
        } else {
            peer.queue_ordered_batch(vec![examine_target(character,request.item,&srv.store,&srv.scene,&srv.examination_names,&srv.corpse_loot,srv.now_ms,&effect_lines(&srv.encounters,&srv.abilities,request.item,srv.now_ms),srv.encounters.as_ref().and_then(|e|e.hit_percent(&srv.store,character,request.item)))?])?;
        }
        return Ok(replies);
    }
    if srv.scene.examination_content(character,request.item).is_some_and(|o|matches!(o.service,Some(crate::services::Service::Door{..}))) {
        let result=(||->io::Result<Vec<crate::scene::Delivery>>{
            if request.owner!=character || !srv.store.characters[&character].progression.alive() {return Err(invalid("Use a door with your living character."));}
            if peer.last_world_use_ms.is_some_and(|last|srv.now_ms.saturating_sub(last)<750) {return Err(invalid("Wait for the door to finish moving."));}
            let updates=srv.scene.use_door(character,request.item)?;peer.last_world_use_ms=Some(srv.now_ms);Ok(updates)
        })();
        match result {Ok(updates)=>outbox.deliveries.extend(updates),Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,}
        return Ok(replies);
    }
    if service_entity == Some(TRAINING_MENTOR_ENTITY)
        && srv.training_quest_enabled
    {
        if request.owner != character {
            return Err(invalid(
                "Mentor use owner mismatch.",
            ));
        }
        match use_training_mentor(
            peer,
            character,
            request.item,
            &mut srv.store,
            &mut srv.scene,
            &srv.inventory_metadata,
            srv.now_ms,
        ) {
            Ok(updates) => outbox.deliveries.extend(updates),
            Err(error) => replies.extend(
                peer.send_event(
                    &script::chat(&error.to_string(), 1)
                        .map_err(other)?,
                    srv.now,
                )?,
            ),
        }
        return Ok(replies);
    }
    if srv.scene.examination_content(character,request.item).is_some_and(|o|o.service==Some(crate::services::Service::Vendor))
        && srv.merchant_catalog.is_some()
    {
        if request.owner != character {
            return Err(invalid(
                "Merchant use owner mismatch.",
            ));
        }
        match merchant_action(
            peer,
            character,
            crate::merchants::Action::List,
            &mut srv.store,
            &mut srv.scene,
            &srv.inventory_metadata,
            srv.merchant_catalog.as_ref().ok_or_else(|| {
                invalid("Merchant catalog is not enabled.")
            })?,
            &mut srv.next_offer,
            srv.now_ms,
            Some(request.item),
            srv.trade_assets.as_ref(),
            &srv.examination_names,
        ) {
            Ok(updates) => outbox.deliveries.extend(updates),
            Err(error) => replies.extend(
                peer.send_event(
                    &script::chat(&error.to_string(), 1)
                        .map_err(other)?,
                    srv.now,
                )?,
            ),
        }
        return Ok(replies);
    }
    // A bag item a quest's UseItem set answers for (`quest_emotes`).
    let used=srv.store.characters[&character].inventory.items().find(|i|i.id==request.item&&matches!(i.location,crate::inventory::Location::Bag(_))).map(|i|i.definition);
    if let Some(definition)=used.filter(|d|crate::quest_emotes::tables().is_some_and(|t|t.answers(crate::quest_emotes::Trigger::UseItem,*d,0,None,&srv.store.characters[&character].quests,crate::recovered_quests::now()))) {
        if request.owner!=character{return Err(invalid("Quest item owner mismatch"));}
        match trigger_emotes(peer,character,crate::quest_emotes::Trigger::UseItem,&[definition],&mut srv.store,&mut srv.scene,&srv.inventory_metadata,&mut srv.encounters,srv.now_ms) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,srv.now)?),
        }
        return Ok(replies);
    }
    if srv.store.characters[&character].inventory.items().any(|i|i.id==request.item&&crate::recovered_quests::use_profile(i.definition).is_some()) {
        if request.owner!=character{return Err(invalid("Quest elixir owner mismatch"));}
        if let Err(error)=use_recovered_elixir(peer,character,request.item,&mut srv.store,&mut srv.scene) {
            replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,srv.now)?);
        }
        return Ok(replies);
    }
    if srv.store.characters[&character].inventory.items().any(
        |item| {
            item.id == request.item
                && matches!(
                    item.definition,
                    crate::item_services::POTION_DEFINITION
                        | crate::crafting::VIGOR_OUTPUT
                )
        },
    ) {
        if !srv.config.initialize_ui
            || !srv.starter_equipment
            || request.owner != character
        {
            return Err(invalid(
                "Training potion use is not enabled or not owned by this character.",
            ));
        }
        match use_training_potion(
            peer,
            character,
            request.item,
            &mut srv.store,
            &mut srv.scene,
            srv.now_ms,
        ) {
            Ok(updates) => outbox.deliveries.extend(updates),
            Err(error) => replies.extend(
                peer.send_event(
                    &script::chat(&error.to_string(), 1)
                        .map_err(other)?,
                    srv.now,
                )?,
            ),
        }
        return Ok(replies);
    }
    if srv.shelter_gameplay && used_on.is_some() {
        if request.owner!=character {return Err(invalid("Item owner mismatch"));}
        match use_emote(peer,character,request.target,crate::quest_emotes::Trigger::ItemUsedOn,Some(request.item),&mut srv.store,&mut srv.scene,&srv.inventory_metadata,&mut srv.encounters,&mut srv.bootstrap,srv.now_ms) {
            Ok(updates)=>outbox.deliveries.extend(updates),
            Err(error)=>replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,srv.now)?),
        }
        return Ok(replies);
    }
    if let Some(route) = srv.portal_routes.get(request.item) {
        if request.owner != character {
            return Err(invalid(
                "portal owner differs from active character",
            ));
        }
        match use_portal(
            peer,
            character,
            request.item,
            route,
            &mut srv.store,
            &mut srv.scene,
            &mut srv.encounters,
            &mut srv.bootstrap,
            srv.now_ms,
        ) {
            Ok(updates) => outbox.deliveries.extend(updates),
            Err(error) => replies.extend(
                peer.send_event(
                    &script::chat(&error.to_string(), 1)
                        .map_err(other)?,
                    srv.now,
                )?,
            ),
        }
        return Ok(replies);
    }
    if srv.config.portals
        && srv.scene
            .examination_content(character, request.item)
            .is_some_and(|object| {
                matches!(
                    object.entity,
                    0x4700084d
                        | 0x47001451
                        | 0x47001454
                        | 0x4700369a
                )
            })
    {
        replies.extend(peer.send_event(&script::chat("This portal's destination is unavailable.",1).map_err(other)?,srv.now)?);
        return Ok(replies);
    }
    if srv.config.lifestones
        && srv.scene
            .examination_content(character, request.item)
            .is_some_and(|object| {
                object.entity == 0x470007de
            })
    {
        let result = (|| -> io::Result<Vec<Vec<u8>>> {
            if request.owner != character
                || peer.combat
                || !srv.store.characters[&character]
                    .progression
                    .alive()
            {
                return Err(invalid(
                    "Bind while alive and out of combat.",
                ));
            }
            if peer.last_world_use_ms.is_some_and(|last| {
                srv.now_ms.saturating_sub(last) < 1000
            }) {
                return Err(invalid(
                    "Wait before using the lifestone again.",
                ));
            }
            let player = srv.scene
                .player_placement(character)
                .ok_or_else(|| {
                    invalid("Player is not ready.")
                })?;
            let stone = srv.scene
                .content_placement(request.item)
                .ok_or_else(|| {
                    invalid("Lifestone is not available.")
                })?;
            let distance: f64 = player
                .coordinates
                .into_iter()
                .zip(stone.coordinates)
                .map(|(a, b)| {
                    (f64::from(a) - f64::from(b)).powi(2)
                })
                .sum();
            if player.cell >> 16 != stone.cell >> 16 || distance > 16.0
            {
                return Err(invalid(
                    "Move within four meters of the lifestone.",
                ));
            }
            let saved = &srv.store.characters[&character];
            let skills = saved
                .skills
                .with_lifestone_recall(
                    &saved.progression,
                    saved
                        .creation
                        .as_ref()
                        .map_or(1, |request| request.race),
                )
                .map_err(other)?;
            let mut events = vec![crate::ui::skill_update(crate::skills::LIFESTONE_RECALL,&skills,saved.skill_used.get(&crate::skills::LIFESTONE_RECALL).copied()).map_err(other)?,crate::ui::lifestone_bound(character,request.item,player).map_err(other)?,script::chat("You are bound to this lifestone. Recall is available in your skill list; @recall also returns under development travel rules.",1).map_err(other)?];
            if saved.quests.journey_phase() == 1 {
                events.extend(saved.quests.apply(&crate::quests::Command::BoundLifestone).map_err(other)?.updates_since(&saved.quests).map_err(other)?);
            }
            let lifestone = srv.scene.examination_content(character, request.item).map_or(0, |o| o.entity);
            peer.validate_scene_batch(&events)?;
            srv.store.bind_lifestone(
                &peer.session.account,
                character,
                player,
            )?;
            // The quests' Bind sets (`quest_emotes`).
            let bound = object_quests(&mut srv.store, character, crate::quest_emotes::Trigger::Bind, lifestone, request.item);
            if !bound.is_empty() { peer.queue_ordered_batch(bound)?; }
            peer.last_world_use_ms = Some(srv.now_ms);
            Ok(events)
        })(
        );
        match result {
            Ok(events) => {
                peer.queue_ordered_batch(events)?
            }
            Err(error) => replies.extend(
                peer.send_event(
                    &script::chat(&error.to_string(), 1)
                        .map_err(other)?,
                    srv.now,
                )?,
            ),
        }
        return Ok(replies);
    }
    if srv.quest_gameplay && service_entity.is_some_and(|d|crate::npc_dialogue::profiles().contains_key(&d))
        && srv.encounters.as_ref().is_none_or(|e|!e.is_creature(request.item)) {
        if request.owner!=character {return Err(invalid("NPC use owner mismatch."));}
        if let Err(error)=use_npc_dialogue(peer,character,request.item,&srv.store,&srv.scene,srv.now_ms) {
            peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?;
        }
        return Ok(replies);
    }
    if service_entity.is_some() {
        if request.owner!=character || !srv.store.characters[&character].progression.alive(){return Err(invalid("Use the object with your living character."));}
        if !srv.scene.combat_distance(character,request.item).is_some_and(|d|d<=4.) {
            peer.queue_ordered_batch(vec![script::chat("Move within four metres to use this object.",1).map_err(other)?])?;
        }else{
            let guidance=crate::professions::catalog().workshops.iter().find(|w|Some(w.id)==service_entity).map(|w|match w.effect{
                0x6f002ee6=>"Refining workshop: open Skills > Craft, select Refining, then choose a recipe and quantity. Stay within eight meters while crafting.",
                0x6f002ee7=>"Spellbinding workshop: enchantment extraction and totem application are not implemented yet. Materials will not be consumed.",
                _=>"Crafting workshop: open Skills > Craft and choose a profession. Select a recipe and level, fill its ingredients, then craft while within eight meters. Earn crafting XP and use Raise to advance.",
            }).unwrap_or("Examining this object. Additional scripted actions are not available yet.");
            peer.queue_ordered_batch(vec![examine_target(character,request.item,&srv.store,&srv.scene,&srv.examination_names,&srv.corpse_loot,srv.now_ms,&effect_lines(&srv.encounters,&srv.abilities,request.item,srv.now_ms),srv.encounters.as_ref().and_then(|e|e.hit_percent(&srv.store,character,request.item)))?,script::chat(guidance,1).map_err(other)?])?;
        }
        return Ok(replies);
    }
    let result = if srv.config.initialize_ui {
        change_hand_item(
            peer,
            character,
            &request,
            HandChange::Toggle,
            &mut srv.store,
            &mut srv.scene,
            &srv.inventory_metadata,
            srv.now_ms,
        )
    } else {
        Err(invalid("Inventory is not initialized."))
    };
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
        Err(error) => replies.extend(
            peer.send_event(
                &script::chat(
                    &format!("Item use rejected: {error}"),
                    1,
                )
                .map_err(other)?,
                srv.now,
            )?,
        ),
    }
    Ok(replies)
}
