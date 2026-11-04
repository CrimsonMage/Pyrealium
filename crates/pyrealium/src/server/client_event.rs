//! One decoded client event of a connected peer: the world and character-list events, and
//! the script calls (`script_call`).
use super::*;

pub(super) fn client_event(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    addr: SocketAddr,
    event: world::ClientEvent,
) -> io::Result<Vec<Vec<u8>>> {
    let mut replies = Vec::new();
    let response = match event {
        world::ClientEvent::ScriptCall {
            function_id,
            arguments,
        } => {
            return script_call(srv, peer, outbox, function_id, arguments, replies);
        }
        world::ClientEvent::DeleteCharacter {
            account,
            character_id,
        } => {
            if peer.session.phase != Phase::CharacterSelection
                || account != peer.session.account.as_bytes()
                || srv.active_characters.contains_key(&character_id)
            {
                return Err(invalid(
                    "deletion outside owned inactive selection",
                ));
            }
            let list = world::character_set_for(&peer.client_version, selection_list_excluding(
                &srv.store, &peer.session.account, srv.bootstrap.as_ref(),
                srv.config.visual_did, Some(character_id),
            )?);
            let characters: Vec<_> = srv.store.characters.values()
                .filter(|c| c.account == peer.session.account && !c.deleted && c.id != character_id)
                .map(|c| world::MinimalCharacter { id: c.id, name: c.name.clone() })
                .collect();
            let minimal = world::minimal_character_set(peer.session.account.as_bytes(), 5, &characters).map_err(other)?;
            // Prepare all reliable output before persisting deletion. On failure,
            // restore the transport's counters, cipher and pending packet window.
            let saved_transport = peer.transport.clone();
            let saved_blob = peer.blob;
            let prepared = (|| -> io::Result<Vec<Vec<u8>>> {
                let mut packets = peer.send_event(&world::character_deleted(character_id), srv.now)?;
                packets.extend(peer.send_event(&minimal, srv.now)?);
                packets.extend(peer.send_event(&list, srv.now)?);
                srv.store.delete(&peer.session.account, character_id)?;
                Ok(packets)
            })();
            match prepared {
                Ok(packets) => replies.extend(packets),
                Err(error) => {
                    peer.transport = saved_transport;
                    peer.blob = saved_blob;
                    return Err(error);
                }
            }
            return Ok(replies);
        }
        world::ClientEvent::Position(update) => {
            if let Phase::InWorld { character }
            | Phase::Entering { character } = peer.session.phase
            {
                if !srv.store.characters[&character].progression.alive() {
                    return Err(invalid(
                        "dead character movement rejected",
                    ));
                }
            }
            let character = match peer.session.phase {
                Phase::Entering { character }
                | Phase::InWorld { character } => character,
                _ => return Err(invalid("movement outside world session")),
            };
            let motion = peer
                .motion
                .as_mut()
                .ok_or_else(|| invalid("missing world placement"))?;
            if update.placement.cell != motion.placement.cell
                && !srv.bootstrap
                    .as_mut()
                    .is_some_and(|p| p.contains_cell(update.placement.cell))
            {
                return Err(invalid(
                    "movement references unavailable cell",
                ));
            }
            srv.scene.prepare_cell(update.placement.cell)?;
            motion.accept(update).map_err(other)?;
            let fall_damage = motion.fall.resolve_landing(
                update.placement.coordinates[2],
                srv.store.characters[&character].progression.level().health,
                ||srv.bootstrap.as_mut().is_some_and(|b|b.terrain_contact(update.placement)),
            );
            srv.store.set_placement(character, motion.placement)?;
            outbox.deliveries.extend(
                srv.scene
                    .update(character, motion.position_stamp, &update)
                    .map_err(other)?,
            );
            if fall_damage>0 && matches!(peer.session.phase,Phase::InWorld{..}) {
                let old=srv.store.characters[&character].progression.clone();
                srv.store.progression_command(&peer.session.account,character,crate::progression::Command::Damage(fall_damage))?;
                let state=srv.store.characters[&character].progression.clone();
                outbox.deliveries.extend(srv.scene.player_stats(character,&state));
                let mut events=state.private_changes(&old);
                events.push(script::chat(&format!("You take {} fall damage.",old.health-state.health),1).map_err(other)?);
                if !state.alive(){
                    if let Some(runtime)=&mut srv.encounters {runtime.stop(character);}
                    peer.combat=false;peer.target=None;
                    if let Some(request)=peer.pending_skill.take(){events.push(script::skill_finished(request).map_err(other)?);}
                    events.push(script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?);
                    events.push(crate::ui::death_notice("You died from a fall.").map_err(other)?);
                    events.push(crate::ui::lock_player(true).map_err(other)?);
                    events.extend(personal_quests_logged(&mut srv.store, character, crate::quest_emotes::Trigger::PlayerDeath));
                    outbox.deliveries.extend(player_body_alive(peer, &mut srv.scene, character, srv.store.characters[&character].instance, false));
                }
                outbox.deliveries.extend(publish_health_story(peer,&mut srv.scene,character,srv.store.characters[&character].instance,character,-((old.health-state.health) as i32)));
                peer.queue_ordered_batch(events)?;
            }
            // The player walked into a portal: it travels as a use does.
            // A refused travel (level, combat, the five-second wait) says
            // nothing here, as the body keeps touching the portal.
            if matches!(peer.session.phase, Phase::InWorld { .. })
                && srv.store.characters[&character].progression.alive()
            {
                let saved = &srv.store.characters[&character];
                let visual = saved
                    .creation
                    .as_ref()
                    .and_then(|c| crate::appearance::body(c.race, c.sex))
                    .map(|body| body.visual)
                    .or(srv.config.visual_did);
                let placement = peer.motion.as_ref().map(|m| m.placement);
                let touched = placement
                    .zip(visual)
                    .zip(srv.portal_contacts.as_mut())
                    .and_then(|((placement, visual), contacts)| {
                        contacts.touched(visual, placement)
                    });
                if let Some((portal, route)) = touched
                    .and_then(|portal| Some((portal, srv.portal_routes.get(portal)?)))
                {
                    if let Ok(updates) = use_portal(
                        peer,
                        character,
                        portal,
                        route,
                        &mut srv.store,
                        &mut srv.scene,
                        &mut srv.encounters,
                        &mut srv.bootstrap,
                        srv.now_ms,
                    ) {
                        outbox.deliveries.extend(updates);
                    }
                }
            }
            // Quest emotes' Enter sets (`quest_emotes`): the player came within the
            // radius of their world object, and a set answers for this player now.
            if srv.shelter_gameplay && matches!(peer.session.phase, Phase::InWorld { .. })
                && srv.store.characters[&character].progression.alive()
            {
                let hit = crate::quest_emotes::tables().and_then(|t| {
                    let me = srv.scene.player_placement(character)?;
                    let book = &srv.store.characters[&character].quests;
                    let now = crate::clock::now();
                    t.enter_points().find(|(iid, range)| {
                        srv.scene.examination_content(character, *iid).is_some_and(|o| {
                            me.distance(o.placement).is_some_and(|d| d <= *range)
                                && t.answers(crate::quest_emotes::Trigger::Enter, o.entity, *iid, None, book, now)
                        })
                    }).map(|(iid, _)| iid)
                });
                if let Some(point) = hit {
                    if let Ok(updates) = use_emote(peer, character, point, crate::quest_emotes::Trigger::Enter, None,
                        &mut srv.store, &mut srv.scene, &srv.inventory_metadata, &mut srv.encounters, &mut srv.bootstrap, srv.now_ms) {
                        outbox.deliveries.extend(updates);
                    }
                }
            }
            return Ok(replies);
        }
        world::ClientEvent::LookAt { target } => {
            let character = match peer.session.phase {
                Phase::InWorld { character } => character,
                _ => {
                    return Err(invalid(
                        "target look outside ready world session",
                    ));
                }
            };
            outbox.deliveries
                .extend(srv.scene.look_at(character, target).map_err(other)?);
            return Ok(replies);
        }
        world::ClientEvent::RoomChat(request) => {
            // `room_chat`: the line to every member of the room, the
            // sender too, then the sender's response.
            let Phase::InWorld { character } = peer.session.phase else {
                return Err(invalid("room chat outside the world"));
            };
            if peer.last_chat_ms.is_some_and(|last| srv.now_ms.saturating_sub(last) < 500) {
                return Err(invalid("chat rate limit"));
            }
            let speaker = srv.store.characters.get(&character).map(|c| c.name.clone()).ok_or_else(|| invalid("unknown speaker"))?;
            let online: Vec<u64> = srv.active_characters.keys().copied().collect();
            match crate::room_chat::relay(character, &speaker, &request, &srv.groups, &online).map_err(other)? {
                Some(out) => {
                    peer.last_chat_ms = Some(srv.now_ms);
                    outbox.deliveries.extend(out.into_iter().map(|(recipient, data)| crate::scene::Delivery { recipient, data: data.into() }));
                }
                None => eprintln!("Room chat from {character} to room {} dropped.", request.room),
            }
            return Ok(replies);
        }
        world::ClientEvent::ForceCreate { target } => {
            // Answered only in the world; see `Scene::force_create`.
            if let Phase::InWorld { character } = peer.session.phase {
                outbox.deliveries.extend(srv.scene.force_create(character, target));
            }
            return Ok(replies);
        }
        world::ClientEvent::Look { yaw, pitch } => {
            if peer.motion.is_none() {
                return Err(invalid("look outside world session"));
            }
            let character = match peer.session.phase {
                Phase::Entering { character }
                | Phase::InWorld { character } => character,
                _ => return Err(invalid("look outside world session")),
            };
            outbox.deliveries
                .extend(srv.scene.look(character, yaw, pitch).map_err(other)?);
            return Ok(replies);
        }
        world::ClientEvent::CreateCharacter { account, request } => {
            if account != peer.session.account.as_bytes()
                || peer.session.phase != Phase::CharacterSelection
            {
                return Err(invalid(
                    "character creation outside owned selection session",
                ));
            }
            if let Some(profile) =
                srv.bootstrap.as_ref().filter(|p| p.supports(&request))
            {
                if srv.store
                    .characters
                    .values()
                    .any(|c| c.name.eq_ignore_ascii_case(&request.name))
                {
                    world::reject_character_creation(
                        world::CreationRejection::NameInUse,
                    )
                } else if srv.store
                    .characters
                    .values()
                    .filter(|c| {
                        c.account == peer.session.account && !c.deleted
                    })
                    .count()
                    >= 5
                {
                    world::reject_character_creation(
                        world::CreationRejection::NoMoreSlots,
                    )
                } else {
                    match srv.store
                        .create_native(&peer.session.account, &request)
                    {
                        Ok(character) => {
                            eprintln!(
                                "created character id={} with persisted native choices; racial appearance profile",
                                character.id
                            );
                            world::confirm_character_creation(
                                &profile.selection(character.id, &request),
                            )
                            .map_err(other)?
                        }
                        Err(error) => {
                            eprintln!(
                                "character persistence rejected: {error}"
                            );
                            world::reject_character_creation(
                                world::CreationRejection::Corrupt,
                            )
                        }
                    }
                }
            } else {
                if srv.bootstrap.is_none() {
                    eprintln!(
                        "character creation refused: server has no game data. Start with --game-data. Requested entity={:#x} race={} sex={:#x} start={:#x}",
                        request.entity_did,
                        request.race,
                        request.sex,
                        request.start_area
                    );
                } else {
                    eprintln!(
                        "unsupported creation profile: entity={:#x} race={} sex={:#x} start={:#x}",
                        request.entity_did,
                        request.race,
                        request.sex,
                        request.start_area
                    );
                }
                world::reject_character_creation(
                    world::CreationRejection::Undefined,
                )
            }
        }
        world::ClientEvent::EnterWorld {
            character_id,
            account,
        } => {
            let owned = account == peer.session.account.as_bytes()
                && srv.store.characters.get(&character_id).is_some_and(|c| {
                    c.account == peer.session.account && !c.deleted
                });
            let error = if owned {
                if peer.session.phase != Phase::CharacterSelection {
                    return Err(invalid("world entry outside selection"));
                }
                if srv.active_characters.contains_key(&character_id) {
                    return Err(invalid("character already active"));
                }
                if let Some(profile) = &mut srv.bootstrap {
                    if !profile.supports_client(&peer.client_version) {
                        eprintln!(
                            "world entry refused: client {} cannot load this DAT bootstrap's GameplayOptionsProfile; use a matching client/data build",
                            peer.client_version
                        );
                        replies.extend(peer.send_event(
                        &world::character_error(
                            world::CharacterError::CouldNotPlaceCharacter,
                        ),
                        srv.now,
                    )?);
                        return Ok(replies);
                    }
                    if let Some(request) = srv.store
                        .characters
                        .get(&character_id)
                        .and_then(|c| c.creation.as_ref())
                        .filter(|r| profile.supports(r))
                        .cloned()
                    {
                        let saved_placement = srv.store
                            .placement(character_id)
                            .unwrap_or(profile.start(&request));
                        let placement=if srv.store.characters[&character_id].progression.alive() &&
                            srv.scene.content_placement(0x8140000000030048).is_some() {
                            crate::shelter::recover_vault_arrival(saved_placement)
                        } else {saved_placement};
                        if placement!=saved_placement {
                            srv.store.relocate(&peer.session.account,character_id,placement)?;
                            eprintln!("Recovered character {character_id} from obstructed Vault arrival");
                        }
                        if !profile.contains_cell(placement.cell) {
                            return Err(invalid(
                                "saved placement references unavailable cell",
                            ));
                        }
                        if srv.starter_equipment {
                            match srv.store.grant_starter_gear(&peer.session.account, character_id) {
                                Ok(true) => eprintln!("Racial starter gear saved for character {character_id}"),
                                Ok(false) => {},
                                Err(error) => eprintln!("Starter gear deferred for character {character_id}: {error}"),
                            }
                            if request.race == 1 && srv.store.characters[&character_id].training_kit {
                                srv.store.ensure_training_attack(&peer.session.account, character_id)?;
                            }
                        }
                        if srv.config.training_kit {
                            let saved = &srv.store.characters[&character_id];
                            if saved.training_kit
                                && !saved.training_supplies
                                && saved.progression.alive()
                                && saved.inventory.dense_bags()
                                && saved
                                    .inventory
                                    .items()
                                    .filter(|item| {
                                        matches!(
                                            item.location,
                                            crate::inventory::Location::Bag(
                                                _
                                            )
                                        )
                                    })
                                    .count()
                                    + crate::training_kit::SUPPLIES_COUNT
                                    <= crate::inventory::BAG_SLOTS as usize
                            {
                                if srv.store.grant_training_supplies(
                                    &peer.session.account,
                                    character_id,
                                )? {
                                    eprintln!(
                                        "Development training supplies saved for character {character_id}"
                                    );
                                }
                            }
                        }
                        if srv.config.lifestones {
                            let saved = &srv.store.characters[&character_id];
                            if saved.binding.is_some()
                                && saved.progression.alive()
                                && saved
                                    .skills
                                    .get(crate::skills::LIFESTONE_RECALL)
                                    .is_none()
                                && saved.skills.entries().count() < 128
                            {
                                srv.store.ensure_lifestone_recall(
                                    &peer.session.account,
                                    character_id,
                                )?;
                            }
                        }
                        if srv.store.characters[&character_id].skills.entries().count() + 3 <= 128
                            && srv.store.ensure_starter_skills(
                                &peer.session.account,
                                character_id,
                            )?
                        {
                            eprintln!(
                                "Starter skills saved for character {character_id}"
                            );
                        }
                        // Development profile (PYREALIUM_DEV_TRADESKILLS): the five starter tools
                        // (`wsl_toolnew_*_starter`) once, so a character can work a deposit
                        // before Toolmaking is trained. Policy: retail's tool source is unknown.
                        if let Some(assets) = &srv.trade_assets {
                            for material in &crate::tradeskills::MATERIALS {
                                let saved = &srv.store.characters[&character_id];
                                if !saved.progression.alive()
                                    || !saved.inventory.dense_bags()
                                    || saved.inventory.items().any(|i| i.definition == material.starter_tool)
                                {
                                    continue;
                                }
                                let Some(slot) = saved.inventory.free_slot() else { break };
                                let id = srv.store.next_item_id()?;
                                if let Some(item) = assets.make(material.starter_tool, id, slot, 1) {
                                    srv.store.inventory_command(&peer.session.account, character_id, crate::inventory::Command::Grant(item))?;
                                }
                            }
                        }
                        // Development knob (PYREALIUM_DEV_CRAFT_SUPPLIES=1): 100 coal (`wsl_coal`) and 100
                        // flawless pyreal motes, for tests that skip the traders. The DAT's source
                        // is the raw-material traders' stores (`store_cragstone.wc` and others sell
                        // coal at half a coin, `store_cavendo.wc` pyreal at 50: `Assets::trader_store`).
                        // (`700005ec` is a craft trait record, not a loot table.)
                        if let (Some(assets), Some(_)) = (&srv.trade_assets, std::env::var_os("PYREALIUM_DEV_CRAFT_SUPPLIES")) {
                            for definition in [0x4700_2ad4u32, 0x4700_10fa] {
                                let saved = &srv.store.characters[&character_id];
                                if !saved.progression.alive()
                                    || !saved.inventory.dense_bags()
                                    || saved.inventory.items().any(|i| i.definition == definition)
                                {
                                    continue;
                                }
                                let Some(slot) = saved.inventory.free_slot() else { break };
                                let id = srv.store.next_item_id()?;
                                if let Some(item) = assets.make(definition, id, slot, 100) {
                                    srv.store.inventory_command(&peer.session.account, character_id, crate::inventory::Command::Grant(item))?;
                                }
                            }
                        }
                        // Development knob for the live client tests (never set by a player's
                        // server): a new character with no experience starts with this much.
                        if let Some(xp) = std::env::var("PYREALIUM_DEV_START_XP").ok().and_then(|v| v.parse::<u64>().ok()) {
                            if xp > 0 && srv.store.characters[&character_id].progression.xp == 0 {
                                srv.store.progression_command(
                                    &peer.session.account,
                                    character_id,
                                    crate::progression::Command::AwardXp(xp),
                                )?;
                                if let Some(damage) = std::env::var("PYREALIUM_DEV_START_DAMAGE").ok().and_then(|v| v.parse::<u32>().ok()).filter(|d| *d > 0) {
                                    srv.store.progression_command(
                                        &peer.session.account,
                                        character_id,
                                        crate::progression::Command::Damage(damage),
                                    )?;
                                }
                            }
                        }
                        // Development knob (never set by a player's server): a character with no
                        // gold starts with this much, for the live tests that buy a trader's stock.
                        if let Some(gold) = std::env::var("PYREALIUM_DEV_START_GOLD").ok().and_then(|v| v.parse::<u32>().ok()) {
                            if gold > 0 && srv.store.characters[&character_id].progression.gold == 0 {
                                srv.store.progression_command(
                                    &peer.session.account,
                                    character_id,
                                    crate::progression::Command::AwardGold(gold),
                                )?;
                            }
                        }
                        if srv.training_quest_enabled
                            && srv.store.characters[&character_id]
                                .quests
                                .get()
                                .is_none()
                            && srv.store.characters[&character_id]
                                .progression
                                .alive()
                        {
                            srv.store.quest_command(
                                &peer.session.account,
                                character_id,
                                crate::quests::Command::Accept,
                            )?;
                        }
                        let instance = srv.store.begin_instance(
                            &peer.session.account,
                            character_id,
                        )?;
                        let mut events = profile
                            .events_at_state(
                                character_id,
                                &request,
                                placement,
                                instance,
                                &srv.store.characters[&character_id]
                                    .progression,
                            )
                            .map_err(other)?;
                        let visual=crate::starter_gear::appearance(&srv.store.characters[&character_id]).map_err(other)?;
                        let old_visual=crate::appearance::visual_len(&events[1][12..]).ok_or_else(||invalid("Invalid player appearance"))?;
                        events[1].splice(12..12+old_visual,visual);
                        let owner = peer.session.account.clone();
                        peer.session
                            .request_enter(character_id, &owner, true)
                            .map_err(other)?;
                        peer.ordered_stamp = 0;
                        peer.clear_scene_outbox();
                        peer.target = None;
                        peer.combat = false;
                        peer.pending_skill = None;
                        peer.motion =
                            Some(Motion::new(placement, instance));
                        srv.active_characters.insert(character_id, addr);
                        srv.scene
                            .enter_hidden(
                                crate::scene::Body::new(
                                    character_id,
                                    placement,
                                    events[1].clone(),
                                )
                                .map_err(other)?
                                .with_progression(
                                    &srv.store.characters[&character_id]
                                        .progression,
                                )
                                .with_hands(srv.store.characters[&character_id].inventory.items()
                                    .filter(|item| matches!(item.location, crate::inventory::Location::Equipped(_)))
                                    .filter(|item| srv.inventory_metadata.get(&item.definition).and_then(|entry| *entry).and_then(|entry| entry.equipment).is_some_and(|hand| hand.hand_slot(item).is_some()))
                                    .map(|item| inventory_spawn(item, character_id, instance, instance, &srv.inventory_metadata))
                                    .collect::<io::Result<Vec<_>>>()?)
                                .map_err(other)?,
                            )
                            .map_err(other)?;
                        for (index, event) in events.iter().enumerate() {
                            replies.extend(peer.send_world_event(
                                event,
                                index == 2,
                                srv.now,
                            )?);
                        }
                        eprintln!(
                            "world bootstrap sent for character {character_id}"
                        );
                        return Ok(replies);
                    }
                }
                world::CharacterError::CouldNotPlaceCharacter
            } else {
                world::CharacterError::CharacterNotOwned
            };
            world::character_error(error)
        }
        world::ClientEvent::LogOffCharacter { character_id } => {
            if !matches!(peer.session.phase, Phase::Entering { character } | Phase::InWorld { character } if character == character_id)
            {
                return Err(invalid(
                    "logout does not own the active character",
                ));
            }
            srv.store.save()?;
            replies.extend(peer.send_event(&world::char_exit_game(), srv.now)?);
            peer.session.leave_world(character_id).map_err(other)?;
            peer.clear_scene_outbox();
            peer.motion = None;
            peer.target = None;
            peer.combat = false;
            srv.active_characters.remove(&character_id);
            departed(&mut srv.store, character_id);
            outbox.deliveries.extend(srv.scene.remove(character_id));
            let list = world::character_set_for(
                &peer.client_version,
                selection_list(
                    &srv.store,
                    &peer.session.account,
                    srv.bootstrap.as_ref(),
                    srv.config.visual_did,
                )?,
            );
            replies.extend(peer.send_event(&list, srv.now)?);
            return Ok(replies);
        }
        world::ClientEvent::SceneRenderingComplete => {
            let first =
                matches!(peer.session.phase, Phase::Entering { .. });
            if first {
                // Policy (`crate::story`): a character with no scene earned is given the Prologue's.
                if let Phase::Entering { character } = peer.session.phase {
                    if srv.store.characters.get(&character).is_some_and(|c| c.progression.alive() && c.quests.scenes().next().is_none()) {
                        for scene in crate::story::PROLOGUE {
                            srv.store.quest_command(&peer.session.account, character, crate::quests::Command::Scene(scene))?;
                        }
                    }
                    // The quests' EnterWorld sets (`quest_emotes`).
                    let entered = personal_quests_logged(&mut srv.store, character, crate::quest_emotes::Trigger::EnterWorld);
                    if !entered.is_empty() { peer.queue_ordered_batch(entered)?; }
                }
                // The quest book's effects join the ledger the login lists.
                if let (Phase::Entering { character }, Some(runtime)) = (&peer.session.phase, srv.encounters.as_mut()) { let character = *character;
                    if let Some(c) = srv.store.characters.get(&character) {
                        let unix = crate::clock::now();
                        runtime.sync_quest_effects(character, &c.quests.quest_effects(unix), unix, srv.now_ms, false);
                    }
                }
                if let Some(packet) = srv.initial_ui.as_deref() {
                    let saved = match peer.session.phase {
                        Phase::Entering { character } => {
                            srv.store.characters.get(&character).filter(|c| {
                                c.ui.is_some()
                                    || !c.hotbar.is_empty()
                                    || c.inventory.items().next().is_some()
                                    || c.progression != crate::progression::Progression::default()
                                    || !c.skills.is_empty()
                                    || c.quests.get().is_some()
                                    || c.quests.scenes().next().is_some()
                            })
                        }
                        _ => None,
                    };
                    if let Some(saved) = saved {
                        let training_bar = saved
                            .creation
                            .as_ref()
                            .map_or_else(
                                crate::preferences::Hotbar::training_attack,
                                |request| {
                                    crate::preferences::Hotbar::starter(
                                        request.race,
                                        request.sex,
                                    )
                                },
                            );
                        let hotbar = if srv.config.training_kit
                            && saved.hotbar.is_empty()
                        {
                            &training_bar
                        } else {
                            &saved.hotbar
                        };
                        // The effects still on the character, with the time they were cast at
                        // on the world's clock.
                        let login_effects: Vec<crate::ui::EffectView> = srv.encounters
                            .as_ref()
                            .map(|e| e.applied_to(saved.id, srv.now_ms))
                            .unwrap_or_default()
                            .into_iter()
                            .map(|a| crate::ui::EffectView {
                                eid: a.eid,
                                did: a.did,
                                time_cast: srv.world_clock.seconds() - srv.now_ms.saturating_sub(a.cast_ms) as f64 / 1000.0,
                                duration: a.duration_ms as f32 / 1000.0,
                                power: a.power,
                                pulsed: a.pulsed,
                            })
                            .collect();
                        let restored = crate::ui::player_login_with_chat(
                            saved.ui.as_ref(),
                            Some(hotbar),
                            Some(&saved.inventory),
                            Some(&saved.progression),
                            Some(&saved.skills),
                            Some(&saved.quests),
                            saved.gameplay_options.as_ref(),
                            saved.chat_options.as_ref(),
                        )
                        .map_err(other)?;
                        let instance=peer.motion.as_ref().ok_or_else(||invalid("inventory bootstrap without avatar lifetime"))?.instance;
                        let mut events = saved
                            .inventory
                            .items()
                            .map(|item| {
                                inventory_spawn(
                                    item,
                                    saved.id,
                                    instance,
                                    instance,
                                    &srv.inventory_metadata,
                                )
                            })
                            .collect::<io::Result<Vec<_>>>()?;
                        events.push(restored);
                        // The effects still on the character join the client's registry as they joined it the first time (`WM_Effect` 0x17), after the login's empty one.
                        for effect in &login_effects {
                            events.push(crate::ui::effect_added(effect).map_err(other)?);
                        }
                        // The scenes earned join the client's act registry as they joined it the first time (`WM_Quest` 0x23), after the login's empty one.
                        for scene in saved.quests.scenes() {
                            events.push(crate::story::granted(scene).map_err(other)?);
                            eprintln!("story: scene {scene:08x} granted to character {}", saved.id);
                        }
                        peer.queue_event_batch(events)?;
                    } else {
                        peer.queue_scene(Arc::from(packet))?;
                    }
                }
            }
            peer.session.scene_complete().map_err(other)?;
            if let Phase::InWorld { character } = peer.session.phase {
                outbox.deliveries
                    .extend(srv.scene.activate(character).map_err(other)?);
                if first {
                    outbox.deliveries.push(crate::scene::Delivery {
                        recipient: character,
                        data: script::chat(&crate::licensing::notice(), 1)
                            .map_err(other)?
                            .into(),
                    });
                }
                if first && srv.config.initialize_ui {
                    // The channel table, as retail sent it at login (`room_chat::channel_table`).
                    outbox.deliveries.push(crate::scene::Delivery {
                        recipient: character,
                        data: crate::room_chat::channel_table(crate::room_chat::WORLD).map_err(other)?.into(),
                    });
                }
                if first && srv.config.map_travel {
                    peer.queue_ordered_batch(vec![script::chat(crate::map_travel::CAPABILITY, 1).map_err(other)?])?;
                }
                if first && srv.config.initialize_ui {
                    if let Some(assets) = &srv.trade_assets {
                        outbox.deliveries.extend(trade_actions::load(peer, character, &mut srv.store, &mut srv.scene, assets)?);
                    }
                    let packets = match &srv.trade_assets {
                        Some(assets) => trade_actions::registry(peer, assets)?,
                        None => crate::crafting::initialization_with_quests(srv.config.training_kit || srv.quest_gameplay, &srv.store.characters[&character].professions, &srv.store.characters[&character].quests.arwic)
                            .map_err(other)?,
                    };
                    for packet in packets {
                        outbox.deliveries.push(crate::scene::Delivery {
                            recipient: character,
                            data: packet.into(),
                        });
                    }
                }
                if first && srv.config.training_kit {
                    outbox.deliveries.push(crate::scene::Delivery { recipient: character, data: script::chat(
                        "Training: equip the sword in your bag. Click an Idol Spawn, stand within three metres, and enter combat. Your attacks repeat until you leave combat. The first hotbar button swings once with the training attack skill. Speak to Pectar for the three-kill reward. Tebor opens the shop. A once-only supply pack provides two health potions, one vigor potion, 1 Mucor Mushroom and 100 Tallow. Use Crafting to make another potion. Lifestones bind your return; portals lead outside and back.", 1).map_err(other)?.into() });
                }
                eprintln!(
                    "Scene ready for character {character}: cell={:08x}, visible content={}",
                    srv.scene
                        .player_placement(character)
                        .map_or(0, |point| point.cell),
                    srv.scene.visible_content_count(character).unwrap_or(0)
                );
                if first
                    && !srv.store.characters[&character].progression.alive()
                {
                    for wire in [
                        crate::ui::death_notice("You have fallen.")
                            .map_err(other)?,
                        crate::ui::lock_player(true).map_err(other)?,
                    ] {
                        outbox.deliveries.push(crate::scene::Delivery {
                            recipient: character,
                            data: wire.into(),
                        });
                    }
                    outbox.deliveries.extend(player_body_alive(peer, &mut srv.scene, character, srv.store.characters[&character].instance, false));
                }
            }
            eprintln!("client reported SceneRenderingComplete");
            return Ok(replies);
        }
    };
    replies.extend(peer.send_event(&response, srv.now)?);
    Ok(replies)
}
