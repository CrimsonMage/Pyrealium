//! A peer's script call (`script::Request`): decoded, then dispatched to its handler.
use super::*;

pub(super) fn script_call(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    function_id: u32,
    arguments: Vec<u8>,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let character = match peer.session.phase {
        Phase::InWorld { character } => character,
        _ => {
            return Err(invalid(
                "gameplay request outside ready world session",
            ));
        }
    };
    let request = match script::decode(function_id, &arguments) {
        Ok(request) => request,
        Err(error) => {
            if peer.unhandled_scripts.len() < 16
                && peer.unhandled_scripts.insert(function_id)
            {
                eprintln!(
                    "script request rejected: function={function_id:#010x} bytes={} reason={error}",
                    arguments.len()
                );
            }
            return Err(other(error));
        }
    };
    match request {
        script::Request::Loot(request)=>{
            let restore=peer.combat && matches!(request,crate::loot::Request::Take{..}|crate::loot::Request::Transmute{..}) && srv.store.characters[&character].progression.alive();
            if restore {
                if let Some(runtime)=&mut srv.encounters{runtime.stop(character);}
                peer.combat=false;
                peer.queue_ordered_batch(vec![script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?])?;
                outbox.deliveries.extend(training_combat_visual(peer,character,&srv.store,&mut srv.scene,false,false,false)?);
            }
            let result=corpse_action(peer,character,request,&mut srv.corpse_loot,&mut srv.store,&mut srv.scene,&srv.inventory_metadata,srv.now_ms);
            // Restore stance on success and rejection, without restarting an
            // attack against the selected corpse.
            if restore {
                peer.combat=true;
                peer.queue_ordered_batch(vec![script::private_call(script::COMBAT_STATE,&script::integer(1)).map_err(other)?])?;
                outbox.deliveries.extend(training_combat_visual(peer,character,&srv.store,&mut srv.scene,true,false,false)?);
            }
            match result{
                Ok(updates)=>outbox.deliveries.extend(updates),
                Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Loot rejected: {error}"),1).map_err(other)?])?,
            }
            return Ok(replies);
        }
        script::Request::Allegiance(query) => {
            let target = query.target.unwrap_or(character);
            if !srv.store
                .characters
                .get(&target)
                .is_some_and(|saved| !saved.deleted)
                || (target != character
                    && !srv.scene.can_observe(character, target))
            {
                return Err(invalid(
                    "That player is not available to examine.",
                ));
            }
            // No server operation currently creates allegiance membership.
            // Report actual unaffiliated state, never a captured hierarchy.
            peer.queue_ordered_batch(vec![
                crate::allegiance::unaffiliated().map_err(other)?,
            ])?;
            return Ok(replies);
        }
        script::Request::Trade(request) => {
            // `trade`: answered after this packet, with both parties.
            if outbox.trade_requests.len() >= 4 {
                return Err(invalid("Too many trade requests in one packet."));
            }
            outbox.trade_requests.push((character, request));
            return Ok(replies);
        }
        script::Request::Group(request) => {
            if !srv.config.initialize_ui
                || !srv.scene.can_observe(character, character)
            {
                return Err(invalid(
                    "Groups require a ready player with the HUD enabled.",
                ));
            }
            if outbox.group_requests.len() >= 4 {
                return Err(invalid(
                    "Too many group requests in one packet.",
                ));
            }
            outbox.group_requests.push((character, request));
            return Ok(replies);
        }
        script::Request::MerchantSell(request) => {
            return on_merchant_sell(srv, peer, outbox, character, request, replies);
        }
        script::Request::MoneyWithdraw(request) => {
            return on_money_withdraw(srv, peer, outbox, character, request, replies);
        }
        script::Request::MerchantBuy(request) => {
            return on_merchant_buy(srv, peer, outbox, character, request, replies);
        }
        script::Request::MerchantRefresh(merchant) => {
            return on_merchant_refresh(srv, peer, character, merchant, replies);
        }
        script::Request::MerchantClose(merchant) => {
            return on_merchant_close(peer, merchant, replies);
        }
        script::Request::MerchantConsignment(merchant) => {
            return on_merchant_consignment(srv, peer, character, merchant, replies);
        }
        script::Request::ExamineClear => return Ok(replies),
        script::Request::CraftRaise(skill) => {
            return on_craft_raise(srv, peer, character, skill, replies);
        }
        script::Request::CraftValidate(request) => {
            return on_craft_validate(srv, peer, character, request, replies);
        }
        script::Request::CraftExecute(request) => {
            return on_craft_execute(srv, peer, outbox, character, request, replies);
        }
        script::Request::Emote(token) => {
            return on_emote(srv, peer, outbox, character, token, replies);
        }
        script::Request::UseSkill {
            request_id,
            skill,
            target,
        } => {
            return on_use_skill(srv, peer, outbox, character, request_id, skill, target, replies);
        }
        script::Request::Examine(target) => {
            return on_examine(srv, peer, outbox, character, target, replies);
        }
        script::Request::UseObject(request) => {
            return on_use_object(srv, peer, outbox, character, request, replies);
        }
        script::Request::Equip(request) => {
            return on_equip(srv, peer, outbox, character, request, replies);
        }
        script::Request::UnEquip(request) => {
            return on_unequip(srv, peer, outbox, character, request, replies);
        }
        script::Request::InventoryDrop(request) => {
            return on_inventory_drop(srv, peer, outbox, character, request, replies);
        }
        script::Request::InventoryMove(request) => {
            return on_inventory_move(srv, peer, character, request, replies);
        }
        script::Request::StoryPlayed => {
            // Nothing a retail server did with the list is known (`crate::story`); it is accepted.
            eprintln!("story: played-scene list accepted for character {character} ({} bytes)", arguments.len());
        }
        script::Request::Resurrect => {
            match resurrect_character(
                peer,
                character,
                &mut srv.store,
                &mut srv.scene,
                &mut srv.encounters,
                srv.config.lifestones,
                &mut srv.bootstrap,
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
        }
        script::Request::Skill { action, skill } => {
            return on_skill(srv, peer, outbox, character, action, skill, replies);
        }
        script::Request::GameplayOptions(options) => {
            srv.store.update_gameplay_options(&peer.session.account, character, options)?;
        }
        // Kept, and sent back in the login's options profile
        // (`ui::player_login_with_chat`).
        script::Request::ChatOptions(options) => {
            srv.store.update_chat_options(&peer.session.account, character, options)?;
        }
        script::Request::UiSettings(settings) => {
            srv.store.update_ui(
                &peer.session.account,
                character,
                settings,
            )?;
        }
        script::Request::Hotbar(update) => {
            srv.store.update_hotbar(
                &peer.session.account,
                character,
                update,
            )?;
        }
        script::Request::Target(target) => {
            return on_target(srv, peer, character, target, replies);
        }
        request @ (script::Request::Combat(_)
        | script::Request::Attack(_)
        | script::Request::StopAttack(_)) => {
            return on_combat(srv, peer, outbox, character, request, replies);
        }
        request @ (script::Request::Say(_)
        | script::Request::Tell { .. }) => {
            return on_chat(srv, peer, outbox, character, request, replies);
        }
    }
    Ok(replies)
}
