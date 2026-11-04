//! Merchant and money-bag script calls: selling, buying, withdrawing, refreshing, closing and consignment.
use super::*;

pub(super) fn on_merchant_sell(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::merchants::SellRequest,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    // `DragToMoneyBag` melts an item into the bag anywhere,
    // as retail did: the 2005 capture
    // `selling.items.in.inventory.elog` melts with no store
    // open (0x82050011 line, money 0xC1C, 0x820E0058,
    // 0x820F0009, 0x100C2), and Turbine's 2002 developer
    // chat (AC2 Vault interviews/devchat 030702): "we allow
    // players to melt down their loot in to gold". The
    // prices stay Pyrealium's (`merchants::sale_value`).
    let result =
        (|| -> io::Result<Vec<crate::scene::Delivery>> {
            if srv.merchant_catalog.is_none() {
                return Err(invalid(
                    "Melting items is not enabled on this server.",
                ));
            }
            if peer.last_inventory_ms.is_some_and(|last| {
                srv.now_ms.saturating_sub(last) < 250
            }) {
                return Err(invalid(
                    "Wait briefly before another inventory action.",
                ));
            }
            let saved = &srv.store.characters[&character];
            if peer.combat {return Err(invalid("Leave combat before trading."));}
            let plan = crate::merchants::plan_native_sell(
                character,
                &request,
                &saved.inventory,
                &saved.progression,
            )
            .map_err(io::Error::other)?;
            let mut events = plan.events;
            events.push(
                script::chat(&plan.message, 1)
                    .map_err(other)?,
            );
            let updates = item_service_transaction(
                peer,
                character,
                &plan.items,
                &plan.changes,
                events,
                &mut srv.store,
                &mut srv.scene,
                &srv.inventory_metadata,
            )?;
            peer.last_inventory_ms = Some(srv.now_ms);
            Ok(updates)
        })(
        );
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
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

pub(super) fn on_money_withdraw(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::money::WithdrawRequest,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let result =
        (|| -> io::Result<Vec<crate::scene::Delivery>> {
            let coin = srv.money_coin.as_ref().ok_or_else(|| {
                invalid("The money bag is not enabled on this server.")
            })?;
            if peer.last_inventory_ms.is_some_and(|last| {
                srv.now_ms.saturating_sub(last) < 250
            }) {
                return Err(invalid(
                    "Wait briefly before another inventory action.",
                ));
            }
            if peer.combat {return Err(invalid("Leave combat before taking money out."));}
            let saved = &srv.store.characters[&character];
            let plan = crate::money::plan_withdraw(
                character,
                &request,
                coin,
                &saved.inventory,
                &saved.progression,
                srv.store.next_item_id()?,
            )
            .map_err(io::Error::other)?;
            let updates = item_service_transaction(
                peer,
                character,
                &plan.items,
                &plan.changes,
                plan.events,
                &mut srv.store,
                &mut srv.scene,
                &srv.inventory_metadata,
            )?;
            peer.last_inventory_ms = Some(srv.now_ms);
            Ok(updates)
        })(
        );
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
        Err(error) => {
            // The bag stays ghosted until a WM_Money answer.
            let mut events = vec![script::chat(&error.to_string(), 1).map_err(other)?];
            events.push(crate::money::bag_done(character).map_err(other)?);
            for event in events {
                replies.extend(peer.send_event(&event, srv.now)?);
            }
        }
    }
    Ok(replies)
}

pub(super) fn on_merchant_buy(
    srv: &mut Server,
    peer: &mut Peer,
    outbox: &mut Outbox,
    character: u64,
    request: crate::merchants::BuyRequest,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let result =
        (|| -> io::Result<Vec<crate::scene::Delivery>> {
            if srv.merchant_catalog.is_none()
                || !peer.merchant_open_ms.is_some_and(
                    |last| {
                        srv.now_ms.saturating_sub(last)
                            <= 30_000
                    },
                )
            {
                return Err(invalid(
                    "Open the nearby quartermaster before buying.",
                ));
            }
            let merchant =
                merchant_ready(character, &srv.scene, peer.native_shop.as_ref().map(|shop| shop.merchant()))?;
            let shop = peer.native_shop.as_ref().filter(|shop| shop.merchant() == merchant)
            .ok_or_else(|| invalid("The shop has changed; speak to the quartermaster again."))?;
            if peer.last_inventory_ms.is_some_and(|last| {
                srv.now_ms.saturating_sub(last) < 250
            }) {
                return Err(invalid(
                    "Wait briefly before buying again.",
                ));
            }
            let saved = &srv.store.characters[&character];
            if peer.combat {return Err(invalid("Leave combat before trading."));}
            let plan = if shop.is_trader() {
                shop.plan_buy_dat(
                    character,
                    &request,
                    &saved.inventory,
                    &saved.progression,
                    srv.store.next_item_id()?,
                    |definition, id, slot, quantity| {
                        srv.trade_assets.as_ref()?.make(definition, id, slot, quantity)
                    },
                )
            } else {
                shop.plan_buy(
                    character,
                    &request,
                    &saved.inventory,
                    &saved.progression,
                    srv.store.next_item_id()?,
                )
            }
            .map_err(io::Error::other)?;
            let mut events = plan.events;
            events.push(shop.refresh(srv.merchant_catalog.as_ref().ok_or_else(|| invalid("Merchant catalog is not enabled."))?).map_err(other)?);
            events.push(
                script::chat(&plan.message, 1)
                    .map_err(other)?,
            );
            let updates = item_service_transaction(
                peer,
                character,
                &plan.items,
                &plan.changes,
                events,
                &mut srv.store,
                &mut srv.scene,
                &srv.inventory_metadata,
            )?;
            peer.last_inventory_ms = Some(srv.now_ms);
            peer.merchant_open_ms = Some(srv.now_ms);
            Ok(updates)
        })(
        );
    match result {
        Ok(updates) => outbox.deliveries.extend(updates),
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

pub(super) fn on_merchant_refresh(
    srv: &mut Server,
    peer: &mut Peer,
    character: u64,
    merchant: u64,
    mut replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let result = (|| -> io::Result<()> {
        if peer.last_shop_refresh_ms.is_some_and(|last| srv.now_ms.saturating_sub(last) < 100) {
            return Err(invalid("Wait briefly before refreshing the shop."));
        }
        if peer.combat
            || !srv.store.characters[&character].progression.alive()
            || !peer.merchant_open_ms.is_some_and(|last| srv.now_ms.saturating_sub(last) <= 30_000)
            || merchant_ready(character, &srv.scene, peer.native_shop.as_ref().map(|shop| shop.merchant())).ok() != Some(merchant)
        {
            return Err(invalid("Refresh requires a nearby active shop while alive and out of combat."));
        }
        let shop = peer.native_shop.as_ref()
            .filter(|shop| shop.merchant() == merchant)
            .ok_or_else(|| invalid("That shop is no longer open."))?;
        let catalog = srv.merchant_catalog.as_ref()
            .ok_or_else(|| invalid("Merchant catalog is not enabled."))?;
        peer.queue_ordered_batch(vec![shop.refresh(catalog).map_err(other)?])?;
        peer.merchant_open_ms = Some(srv.now_ms);
        peer.last_shop_refresh_ms = Some(srv.now_ms);
        Ok(())
    })();
    if let Err(error) = result {
        replies.extend(peer.send_event(&script::chat(&error.to_string(), 1).map_err(other)?, srv.now)?);
    }
    Ok(replies)
}

pub(super) fn on_merchant_close(
    peer: &mut Peer,
    merchant: u64,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    if peer
        .native_shop
        .as_ref()
        .is_some_and(|shop| shop.merchant() == merchant)
    {
        peer.queue_ordered_batch(vec![
            crate::merchants::close(merchant)
                .map_err(other)?,
        ])?;
        peer.native_shop = None;
        peer.merchant_open_ms = None;
    }
    Ok(replies)
}

pub(super) fn on_merchant_consignment(
    srv: &mut Server,
    peer: &mut Peer,
    character: u64,
    merchant: u64,
    replies: Vec<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let text = if peer
        .native_shop
        .as_ref()
        .is_some_and(|shop| shop.merchant() == merchant)
        && merchant_ready(character, &srv.scene, peer.native_shop.as_ref().map(|shop| shop.merchant())).ok()
            == Some(merchant)
    {
        "Consignment listings are not available at this training shop. Use the inventory Sell action for fixed buyback. No items or fees were taken."
    } else {
        "Speak to a nearby quartermaster to open a shop."
    };
    peer.queue_ordered_batch(vec![
        script::chat(text, 1).map_err(other)?
    ])?;
    Ok(replies)
}
