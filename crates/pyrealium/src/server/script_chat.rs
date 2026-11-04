//! Chat script calls: emotes, says and tells.
use super::*;

pub(super) fn on_emote(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    token: u32,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let result = (|| -> io::Result<()> {
        let saved = &srv.store.characters[&character];
        if !srv.config.initialize_ui
            || !srv.scene.can_observe(character, character)
            || !saved.progression.alive()
            || peer.combat
        {
            return Err(invalid(
                "Emotes require a ready, living character out of combat.",
            ));
        }
        let (next, wire) = peer
            .emotes
            .prepare(
                character,
                saved.instance,
                srv.now_ms,
                token,
            )
            .map_err(other)?;
        peer.validate_scene_batch(&[wire.clone()])?;
        let updates = srv.scene
            .broadcast(character, wire)
            .map_err(other)?;
        peer.emotes = next;
        outbox.deliveries.extend(updates);
        Ok(())
    })(
    );
    if let Err(error) = result {
        replies.extend(
            peer.send_event(
                &script::chat(&error.to_string(), 1)
                    .map_err(other)?,
                srv.now,
            )?,
        );
    }
    Ok(replies)
}

pub(super) fn on_chat(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: script::Request,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if peer.last_chat_ms.is_some_and(|last| {
        srv.now_ms.saturating_sub(last) < 500
    }) {
        return Err(invalid("chat rate limit"));
    }
    let name = &srv.store.characters[&character].name;
    match request {
        script::Request::Say(text) => {
            if matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "@source" | "@license"
            ) {
                peer.queue_ordered_batch(vec![
                    script::chat(
                        &crate::licensing::notice(),
                        1,
                    )
                    .map_err(other)?,
                ])?;
                return Ok(replies);
            }
            if let Some(action) =
                crate::merchants::parse_command(&text)
            {
                peer.last_chat_ms = Some(srv.now_ms);
                let result = action.map_err(io::Error::other).and_then(|action| {
                    if srv.merchant_catalog.is_none() { return Err(invalid("Vendors are not enabled for this content profile.")); }
                    merchant_action(peer, character, action, &mut srv.store, &mut srv.scene, &srv.inventory_metadata, srv.merchant_catalog.as_ref().ok_or_else(|| invalid("Merchant catalog is not enabled."))?, &mut srv.next_offer, srv.now_ms, None, srv.trade_assets.as_ref(), &srv.examination_names)
                });
                match result {
                    Ok(updates) => {
                        outbox.deliveries.extend(updates)
                    }
                    Err(error) => replies.extend(
                        peer.send_event(
                            &script::chat(
                                &error.to_string(),
                                1,
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    ),
                }
                return Ok(replies);
            }
            let words =
                text.split_whitespace().collect::<Vec<_>>();
            if text.trim().eq_ignore_ascii_case("@talk") && srv.quest_gameplay {
                peer.last_chat_ms=Some(srv.now_ms);
                let result=(|| -> io::Result<()> {
                    let target=peer.target.ok_or_else(||invalid("Select an NPC to speak with."))?;
                    if srv.encounters.as_ref().is_some_and(|e|e.is_creature(target)) {
                        return Err(invalid("That creature cannot hold a conversation."));
                    }
                    use_npc_dialogue(peer,character,target,&srv.store,&srv.scene,srv.now_ms)
                })();
                if let Err(error)=result {
                    peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?;
                }
                return Ok(replies);
            }
            if text.trim().eq_ignore_ascii_case("@spawns") {
                peer.last_chat_ms = Some(srv.now_ms);
                let lines=if !srv.config.map_travel {
                    vec!["Spawn surveying is available only with local test map travel.".into()]
                } else if let Some((runtime,point))=srv.encounters.as_ref().zip(srv.scene.player_placement(character)) {
                    runtime.outdoor_survey(&srv.scene,point)
                } else {
                    vec!["No encounter population is loaded.".into()]
                };
                for line in lines {
                    replies.extend(peer.send_event(&script::chat(&line,1).map_err(other)?,srv.now)?);
                }
                return Ok(replies);
            }
            if words.first().is_some_and(|word| word.eq_ignore_ascii_case("@maptravel")) {
                peer.last_chat_ms = Some(srv.now_ms);
                let result = (|| -> io::Result<Vec<crate::scene::Delivery>> {
                    let landscape = srv.map_landscape.as_mut().ok_or_else(|| invalid("Map travel is disabled on this server."))?;
                    let land = crate::map_travel::point(&words)?;
                    let heading = srv.scene.player_placement(character).ok_or_else(|| invalid("Player is not ready."))?.heading;
                    let destination = landscape.destination(land, heading)?;
                    map_travel_character(peer, character, destination, &mut srv.store, &mut srv.scene, &mut srv.encounters, &mut srv.bootstrap, srv.now_ms)
                })();
                match result {
                    Ok(updates) => outbox.deliveries.extend(updates),
                    Err(error) => replies.extend(peer.send_event(&script::chat(&error.to_string(), 1).map_err(other)?, srv.now)?),
                }
                return Ok(replies);
            }
            if words.first().is_some_and(|word| {
                word.eq_ignore_ascii_case("@craft")
            }) {
                peer.last_chat_ms = Some(srv.now_ms);
                let result = (|| -> io::Result<Vec<crate::scene::Delivery>> {
                    if !srv.config.initialize_ui || !(srv.config.training_kit || srv.quest_gameplay) {
                        return Err(invalid("Crafting is not enabled."));
                    }
                    let recipe = match words.as_slice() {
                        [_, name] if name.eq_ignore_ascii_case("health") => crate::crafting::HEALTH_RECIPE,
                        [_, name] if name.eq_ignore_ascii_case("vigor") => crate::crafting::VIGOR_RECIPE,
                        _ => return Err(invalid("Use @craft health or @craft vigor. Training recipes consume one Mucor Mushroom and 100 Tallow; no craft skill or chance roll is required.")),
                    };
                    let request = crate::crafting::select_inventory(recipe, &srv.store.characters[&character].inventory, true)
                        .map_err(|_| invalid("You need one Mucor Mushroom and a full 100-unit Tallow bundle in your bag."))?;
                    craft_item(peer, character, &request, &mut srv.store, &mut srv.scene, &srv.inventory_metadata, srv.now_ms, srv.bootstrap.as_mut().and_then(|b|b.geometry.as_mut()))
                })();
                match result {
                    Ok(updates) => {
                        outbox.deliveries.extend(updates)
                    }
                    Err(error) => replies.extend(
                        peer.send_event(
                            &script::chat(
                                &error.to_string(),
                                1,
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    ),
                }
                return Ok(replies);
            }
            if words.first().is_some_and(|word| {
                word.eq_ignore_ascii_case("@group")
            }) && srv.config.initialize_ui
            {
                peer.last_chat_ms = Some(srv.now_ms);
                let online = group_online(
                    &srv.active_characters,
                    &srv.store,
                    &srv.scene,
                );
                let verb =
                    words.get(1).copied().unwrap_or("");
                let name =
                    words.get(2..).unwrap_or(&[]).join(" ");
                let target = online
                    .values()
                    .find(|member| {
                        member
                            .name
                            .eq_ignore_ascii_case(&name)
                    })
                    .map(|member| member.id);
                let request = match verb
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "create" if !name.is_empty() => Some(
                        // The fellowship panel's defaults: looting method 1
                        // (`InitLootingMenu`), experience shared.
                        crate::groups::Request::Create {
                            name,
                            looting: 1,
                            social: false,
                        },
                    ),
                    "invite" => target.map(
                        crate::groups::Request::Invite,
                    ),
                    "accept" => target.map(
                        crate::groups::Request::Accept,
                    ),
                    "decline" => target.map(
                        crate::groups::Request::Decline,
                    ),
                    "dismiss" => target.map(
                        crate::groups::Request::Dismiss,
                    ),
                    "leave" if name.is_empty() => {
                        Some(crate::groups::Request::Leave)
                    }
                    "disband" if name.is_empty() => Some(
                        crate::groups::Request::Disband,
                    ),
                    _ => None,
                };
                if let Some(request) = request {
                    if outbox.group_requests.len() >= 4 {
                        return Err(invalid(
                            "Too many group requests in one packet.",
                        ));
                    }
                    outbox.group_requests
                        .push((character, request));
                } else {
                    replies.extend(peer.send_event(&script::chat("Fellowship commands: @group create NAME; invite/accept/decline/dismiss PLAYER; leave; disband. Use @g MESSAGE for group chat. Named players must be online.", 1).map_err(other)?, srv.now)?);
                }
                return Ok(replies);
            }
            if words.first().is_some_and(|word| {
                word.eq_ignore_ascii_case("@g")
            }) && srv.config.initialize_ui
            {
                peer.last_chat_ms = Some(srv.now_ms);
                let members = srv.groups.members(character);
                if members.is_empty() || words.len() < 2 {
                    replies.extend(peer.send_event(&script::chat("Join a group and use @g MESSAGE.", 1).map_err(other)?, srv.now)?);
                } else {
                    let packet: Arc<[u8]> = script::chat(
                        &format!(
                            "[Group] {name}: {}",
                            words[1..].join(" ")
                        ),
                        0x10,
                    )
                    .map_err(other)?
                    .into();
                    for recipient in members {
                        outbox.deliveries.push(
                            crate::scene::Delivery {
                                recipient,
                                data: packet.clone(),
                            },
                        );
                    }
                }
                return Ok(replies);
            }
            if words.first().is_some_and(|word| word.eq_ignore_ascii_case("@mentor")) && srv.config.initialize_ui && srv.config.training_kit {
                peer.last_chat_ms = Some(srv.now_ms);
                let result = (|| -> io::Result<()> {
                    let saved = &srv.store.characters[&character];
                    if peer.combat || !saved.progression.alive() { return Err(invalid("Speak to a mentor while alive and out of combat.")); }
                    let mentor = srv.scene.nearby_service(character, TRAINING_MENTOR_ENTITY, None).ok_or_else(|| invalid("Move within four metres of a training mentor."))?;
                    let race = saved.creation.as_ref().map_or(1, |request| request.race);
                    let text = crate::training_dialogue::topic(&words[1..].join(" "), race, &saved.quests).ok_or_else(|| invalid("Ask @mentor help, gear, combat, healing, supplies, crafting, travel or quests."))?;
                    peer.admit_npc_dialogue(srv.now_ms)?;
                    let events = mentor_interaction_events(character, mentor, &srv.scene, &text)?;
                    peer.queue_ordered_batch(events)?;
                    Ok(())
                })();
                if let Err(error) = result { replies.extend(peer.send_event(&script::chat(&error.to_string(), 1).map_err(other)?, srv.now)?); }
                return Ok(replies);
            }
            if words.first().is_some_and(|word| {
                word.eq_ignore_ascii_case("@quest")
            }) && srv.config.initialize_ui
            {
                peer.last_chat_ms = Some(srv.now_ms);
                if !srv.training_quest_enabled
                    && srv.store.characters[&character]
                        .quests
                        .get()
                        .is_none()
                {
                    replies.extend(peer.send_event(&script::chat("The training quest is not enabled on this server.",1).map_err(other)?,srv.now)?);
                    return Ok(replies);
                }
                let action = words[1..].join(" ");
                match quest_action(
                    peer, character, &action, &mut srv.store,
                    &mut srv.scene, None,
                ) {
                    Ok(updates) => {
                        outbox.deliveries.extend(updates)
                    }
                    Err(error) => replies.extend(
                        peer.send_event(
                            &script::chat(
                                &error.to_string(),
                                1,
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    ),
                }
                return Ok(replies);
            }
            if text.trim().eq_ignore_ascii_case("@recall")
                && srv.config.lifestones
            {
                peer.last_chat_ms = Some(srv.now_ms);
                match recall_character(
                    peer,
                    character,
                    &mut srv.store,
                    &mut srv.scene,
                    &mut srv.encounters,
                    &mut srv.bootstrap,
                    srv.now_ms,
                ) {
                    Ok(updates) => {
                        outbox.deliveries.extend(updates)
                    }
                    Err(error) => replies.extend(
                        peer.send_event(
                            &script::chat(
                                &error.to_string(),
                                1,
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    ),
                }
                return Ok(replies);
            }
            if text
                .trim()
                .eq_ignore_ascii_case("@resurrect")
                && srv.encounters.is_some()
            {
                peer.last_chat_ms = Some(srv.now_ms);
                match resurrect_character(
                    peer,
                    character,
                    &mut srv.store,
                    &mut srv.scene,
                    &mut srv.encounters,
                    srv.config.lifestones,
                    &mut srv.bootstrap,
                ) {
                    Ok(updates) => {
                        outbox.deliveries.extend(updates)
                    }
                    Err(error) => replies.extend(
                        peer.send_event(
                            &script::chat(
                                &error.to_string(),
                                1,
                            )
                            .map_err(other)?,
                            srv.now,
                        )?,
                    ),
                }
                return Ok(replies);
            }
            let packet = script::chat(
                &format!("{name}: {text}"),
                0x10,
            )
            .map_err(other)?;
            outbox.deliveries.extend(
                srv.scene
                    .broadcast(character, packet)
                    .map_err(other)?,
            );
        }
        script::Request::Tell { recipient, text } => {
            let recipient_id = srv.store
                .characters
                .values()
                .find(|c| {
                    !c.deleted
                        && c.name.eq_ignore_ascii_case(
                            &recipient,
                        )
                        && srv.active_characters
                            .contains_key(&c.id)
                        && srv.scene.can_observe(c.id, c.id)
                })
                .map(|c| c.id);
            if let Some(recipient_id) = recipient_id {
                let packet = script::chat(
                    &format!("{name} tells you: {text}"),
                    0x20,
                )
                .map_err(other)?;
                outbox.deliveries.push(crate::scene::Delivery {
                    recipient: recipient_id,
                    data: packet.into(),
                });
                replies.extend(peer.send_event(&script::chat(&format!("You tell {recipient}: {text}"), 0x20).map_err(other)?, srv.now)?);
            } else {
                replies.extend(
                    peer.send_event(
                        &script::chat(
                            "That player is not online.",
                            1,
                        )
                        .map_err(other)?,
                        srv.now,
                    )?,
                );
            }
        }
        _ => unreachable!(),
    }
    peer.last_chat_ms = Some(srv.now_ms);
    Ok(replies)
}
