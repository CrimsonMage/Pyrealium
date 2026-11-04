//! Item script calls: crafting, examining, equipping, unequipping, dropping and moving items.
use super::*;

pub(super) fn on_craft_raise(
    srv: &mut Server,
    peer: &mut Peer,
    character: u64,
    skill: u32,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    // Development tradeskills profile (PYREALIUM_DEV_TRADESKILLS) keeps its own per-connection ranks.
    if srv.trade_assets.is_some() {
        trade_actions::raise(peer, skill)?;
        return Ok(replies);
    }
    let result=(||->io::Result<()>{
        if !(srv.config.training_kit||srv.quest_gameplay)||!srv.config.initialize_ui||peer.combat||!srv.scene.can_observe(character,character){return Err(invalid("Crafting training requires a ready player in peace stance."))}
        let change=crate::professions::Change{skill,xp:0,time:crate::clock::now(),raise:true,activity:None};
        let next=srv.store.characters[&character].professions.apply(&change)?;
        let event=crate::crafting::skill_update(skill,&next).map_err(other)?;peer.validate_scene_batch(&[event.clone()])?;
        srv.store.profession_transaction(&peer.session.account,character,&[],&[],&change)?;
        peer.queue_ordered_batch(vec![event])?;Ok(())
    })();
    if let Err(e)=result{peer.queue_ordered_batch(vec![script::chat(&e.to_string(),1).map_err(other)?])?;}
    Ok(replies)
}

pub(super) fn on_craft_validate(
    srv: &mut Server,
    peer: &mut Peer,
    character: u64,
    request: crate::crafting::ValidateRequest,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let saved = &srv.store.characters[&character];
    let allowed = (srv.config.training_kit || srv.quest_gameplay)
        && srv.config.initialize_ui
        && srv.scene.can_observe(character, character)
        && saved.progression.alive()
        && !peer.combat
        && saved.quests.arwic.allowed(request.recipe)
        && match (&srv.trade_assets, crate::crafting::recipe(request.recipe)) {
            (Some(assets), None) => trade_actions::validates(assets, &saved.inventory, &request),
            _ => crate::professions::allowed(&request,&saved.inventory),
        };
    peer.queue_ordered_batch(vec![
        crate::crafting::validation_reply(
            &request, allowed,
        )
        .map_err(other)?,
    ])?;
    Ok(replies)
}

pub(super) fn on_craft_execute(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::crafting::ExecuteRequest,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let before: std::collections::BTreeSet<u64> = srv.store.characters[&character].inventory.items().map(|i| i.id).collect();
    let result = if (srv.config.training_kit || srv.quest_gameplay)
        && srv.config.initialize_ui
    {
        match (&srv.trade_assets, crate::crafting::recipe(request.recipe)) {
            (Some(assets), None) => trade_actions::craft(
                peer,
                character,
                &request,
                assets,
                &mut srv.store,
                &mut srv.scene,
                &srv.inventory_metadata,
                srv.now_ms,
            ),
            _ => craft_item(
            peer,
            character,
            &request,
            &mut srv.store,
            &mut srv.scene,
            &srv.inventory_metadata,
            srv.now_ms,
            srv.bootstrap.as_mut().and_then(|b|b.geometry.as_mut()),
        ),
        }
    } else {
        Err(invalid("Crafting is not enabled in this server profile."))
    };
    // A craft that made an item a quest asks for (`Crafted` sets).
    let result = result.and_then(|mut updates| {
        let made: Vec<u32> = srv.store.characters[&character].inventory.items()
            .filter(|i| !before.contains(&i.id)).map(|i| i.definition).collect();
        // The craft is committed; a quest step that cannot follow (a full bag) says so.
        match crafted_emotes(peer, character, &made, &mut srv.store, &mut srv.scene, &srv.inventory_metadata, &mut srv.encounters, srv.now_ms) {
            Ok(more) => updates.extend(more),
            Err(error) => peer.queue_ordered_batch(vec![script::chat(&error.to_string(), 1).map_err(other)?])?,
        }
        Ok(updates)
    });
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
        Err(error) => peer.queue_ordered_batch(vec![
            crate::crafting::completed(
                request.recipe,
                false,
            )
            .map_err(other)?,
            script::chat(&error.to_string(), 1)
                .map_err(other)?,
        ])?,
    }
    Ok(replies)
}

pub(super) fn on_examine(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    target: u64,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if !srv.config.initialize_ui {
        return Err(invalid(
            "examination requires initialized player UI",
        ));
    }
    if peer.last_examine_ms.is_some_and(|last| {
        srv.now_ms.saturating_sub(last) < 100
    }) {
        return Err(invalid(
            "examination request cooldown",
        ));
    }
    peer.last_examine_ms = Some(srv.now_ms);
    match examine_target(
        character,
        target,
        &srv.store,
        &srv.scene,
        &srv.examination_names,
        &srv.corpse_loot,
        srv.now_ms,
        &effect_lines(&srv.encounters, &srv.abilities, target, srv.now_ms),
        srv.encounters.as_ref().and_then(|e| e.hit_percent(&srv.store, character, target)),
    ) {
        Ok(packet) => {
            outbox.deliveries.push(crate::scene::Delivery {
                recipient: character,
                data: packet.into(),
            })
        }
        Err(error) => replies.extend(
            peer.send_event(
                &script::chat(&error.to_string(), 1)
                    .map_err(other)?,
                srv.now,
            )?,
        ),
    }
    Ok(replies)
}

pub(super) fn on_equip(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::equipment::EquipRequest,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let result =
        (|| -> io::Result<Vec<crate::scene::Delivery>> {
            if !srv.config.initialize_ui {
                return Err(invalid(
                    "Inventory is not initialized.",
                ));
            }
            let action = HandChange::Equip {
                slot: u16::try_from(request.slot).map_err(
                    |_| invalid("Invalid equipment slot."),
                )?,
                source: u16::try_from(request.source)
                    .map_err(|_| {
                        invalid(
                            "Invalid backpack position.",
                        )
                    })?,
            };
            change_hand_item(
                peer,
                character,
                &crate::equipment::UseRequest {
                    target:0,
                    owner: request.owner,
                    item: request.item,
                },
                action,
                &mut srv.store,
                &mut srv.scene,
                &srv.inventory_metadata,
                srv.now_ms,
            )
        })(
        );
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
        Err(error) => {
            peer.queue_ordered_batch(vec![script::chat(
                &format!("Equip rejected: {error}"),
                1,
            )
            .map_err(other)?])?
        }
    }
    Ok(replies)
}

pub(super) fn on_unequip(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::equipment::UnEquipRequest,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let result =
        (|| -> io::Result<Vec<crate::scene::Delivery>> {
            if !srv.config.initialize_ui {
                return Err(invalid(
                    "Inventory is not initialized.",
                ));
            }
            let slot = u16::try_from(request.slot)
                .map_err(|_| {
                    invalid("Invalid equipment slot.")
                })?;
            let destination =
                u16::try_from(request.destination)
                    .map_err(|_| {
                        invalid(
                            "Invalid backpack position.",
                        )
                    })?;
            let item = crate::equipment::UseRequest {
                target:0,
                owner: request.owner,
                item: request.item,
            };
            change_hand_item(
                peer,
                character,
                &item,
                HandChange::Unequip { slot, destination },
                &mut srv.store,
                &mut srv.scene,
                &srv.inventory_metadata,
                srv.now_ms,
            )
        })(
        );
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
        Err(error) => replies.extend(
            peer.send_event(
                &script::chat(
                    &format!("Unequip rejected: {error}"),
                    1,
                )
                .map_err(other)?,
                srv.now,
            )?,
        ),
    }
    Ok(replies)
}

pub(super) fn on_inventory_drop(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::inventory_request::DropRequest,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if !srv.config.initialize_ui {return Err(invalid("Inventory is not initialized."));}
    match drop_ground_item(peer,character,&request,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,srv.now_ms) {
        Ok(updates)=>outbox.deliveries.extend(updates),
        Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Drop rejected: {error}"),1).map_err(other)?])?,
    }
    Ok(replies)
}

pub(super) fn on_inventory_move(
    srv: &mut Server,
    peer: &mut Peer,
    character: u64,
    request: crate::inventory_request::MoveRequest,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if peer.last_inventory_ms.is_some_and(|last| {
        srv.now_ms.saturating_sub(last) < 100
    }) {
        return Err(invalid("inventory action cooldown"));
    }
    let partial = srv.config.initialize_ui && srv.store.characters[&character].inventory.items().any(|i|
        i.id==request.item && i.location==crate::inventory::Location::Bag(request.source) && request.quantity<i.quantity);
    let merge = srv.config.initialize_ui && merge_target(&srv.store.characters[&character].inventory,&request).is_some();
    if merge {
        if let Err(error)=merge_stacks(peer,character,&request,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,srv.now_ms) {
            replies.extend(peer.send_event(&script::chat(&format!("Item move rejected: {error}"),1).map_err(other)?,srv.now)?);
        }
    } else if partial {
        if let Err(error)=split_stack(peer,character,&request,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,srv.now_ms) {
            replies.extend(peer.send_event(&script::chat(&format!("Item move rejected: {error}"),1).map_err(other)?,srv.now)?);
        }
    } else {
    let bag_items = srv.store.characters[&character].inventory.items()
        .filter(|i|matches!(i.location,crate::inventory::Location::Bag(_))).count();
    let request = request.clamped(bag_items);
    let result = (|| -> io::Result<(crate::inventory::Command, Vec<u8>)> {
        if !srv.config.initialize_ui { return Err(invalid("Inventory is not initialized.")); }
        let saved = &srv.store.characters[&character];
        if !saved.progression.alive() { return Err(invalid("You cannot move items while dead.")); }
        let command = request.checked_command(character, &saved.inventory).map_err(other)?;
        let candidate = saved.inventory.apply(&command).map_err(other)?;
        let item = candidate.items().find(|item| item.id == request.item).ok_or_else(||invalid("Item is not owned."))?;
        let metadata = srv.inventory_metadata.get(&item.definition).and_then(|entry| *entry).ok_or_else(||invalid("Bag sorting is unavailable for this item definition."))?;
        let acknowledgment = request.acknowledgment(item, metadata).map_err(other)?;
        Ok((command, acknowledgment))
    })();
    match result {
        Ok((command, acknowledgment)) => {
            let events = vec![acknowledgment];
            peer.validate_scene_batch(&events)?;
            srv.store.inventory_command(
                &peer.session.account,
                character,
                command,
            )?;
            peer.last_inventory_ms = Some(srv.now_ms);
            peer.queue_ordered_batch(events)?;
        }
        Err(error) => replies.extend(
            peer.send_event(
                &script::chat(
                    &format!("Item move rejected: {error}"),
                    1,
                )
                .map_err(other)?,
                srv.now,
            )?,
        ),
    }
    }
    Ok(replies)
}
