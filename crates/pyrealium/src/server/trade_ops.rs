//! Player-to-player trade steps and swaps.
use super::*;

/// What `trade` asks of the world: names of players in it, who is in view, what packs hold.
pub(super) struct TradeWorld<'a> {
    pub(super) store: &'a Store,
    pub(super) scene: &'a crate::scene::Scene,
    pub(super) active: &'a BTreeMap<u64, SocketAddr>,
}
impl crate::trade::World for TradeWorld<'_> {
    fn name(&self, player: u64) -> Option<String> {
        self.active.contains_key(&player).then_some(())?;
        self.store.characters.get(&player).filter(|c| !c.deleted).map(|c| c.name.clone())
    }
    fn reachable(&self, player: u64, other: u64) -> bool {
        self.active.contains_key(&other) && self.scene.can_observe(player, other)
    }
    fn holds(&self, player: u64, item: u64, amount: u32) -> bool {
        self.store.characters.get(&player).is_some_and(|c| {
            c.inventory.items().any(|owned| {
                owned.id == item
                    && matches!(owned.location, crate::inventory::Location::Bag(_))
                    && owned.quantity >= amount
            })
        })
    }
}

/// One trade request: the notices to both parties and, when both accepted, the swap (both
/// packs change together, `Store::swap_items`). A swap that cannot be made (a full pack, an
/// item gone) clears both acceptances (`0x51`) and says why; the offers stay.
pub(super) fn trade_step(
    character: u64,
    request: &crate::trade::Request,
    trades: &mut crate::trade::Runtime,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    active: &BTreeMap<u64, SocketAddr>,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
) -> Vec<crate::scene::Delivery> {
    let delivery = |recipient: u64, data: Vec<u8>| crate::scene::Delivery { recipient, data: data.into() };
    let line = |recipient: u64, text: &str| script::chat(text, 1).ok().map(|data| delivery(recipient, data));
    let mut next = trades.clone();
    let world = TradeWorld { store, scene, active };
    let plan = match next.apply(character, request, &world) {
        Ok(plan) => plan,
        Err(refused) => return line(character, refused.message()).into_iter().collect(),
    };
    let mut out: Vec<_> = plan.before.into_iter().map(|(r, d)| delivery(r, d)).collect();
    if let Some(swap) = &plan.swap {
        match trade_swap(swap, store, scene, metadata) {
            Ok(events) => out.extend(events),
            Err(error) => {
                let mut out: Vec<_> = trades.clear_acceptances(character).into_iter().map(|(r, d)| delivery(r, d)).collect();
                for party in [swap.initiator, swap.responder] {
                    out.extend(line(party, &format!("The trade could not be completed: {error}")));
                }
                return out;
            }
        }
    }
    out.extend(plan.after.into_iter().map(|(r, d)| delivery(r, d)));
    *trades = next;
    out
}

/// Each side's offers leave its pack (a whole stack, or part of one) and enter the other's
/// first, as new items (Pyrealium's acquisition shape, `item_services::acquired`; retail's own
/// item messages for a trade are not in the captures' trade windows).
pub(super) fn trade_swap(
    swap: &crate::trade::Swap,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    use crate::inventory::{Command, Location};
    let first = store.next_item_id()?;
    let mut commands: BTreeMap<u64, (Vec<Command>, Vec<Command>)> = BTreeMap::new();
    for (giver, taker, gives) in [
        (swap.initiator, swap.responder, &swap.initiator_gives),
        (swap.responder, swap.initiator, &swap.responder_gives),
    ] {
        let saved = store.characters.get(&giver).ok_or_else(|| invalid("trade party missing"))?;
        for &(item, amount) in gives {
            let owned = saved
                .inventory
                .items()
                .find(|owned| owned.id == item && matches!(owned.location, Location::Bag(_)))
                .ok_or_else(|| invalid("an offered item is no longer in its pack"))?;
            let take = if amount == owned.quantity {
                Command::ConsumeStackDense { item, quantity: amount }
            } else {
                Command::ConsumePart { item, quantity: amount }
            };
            commands.entry(giver).or_default().0.push(take);
            if metadata.get(&owned.definition).and_then(|info| *info).is_none() {
                return Err(invalid("an offered item cannot be given"));
            }
            let mut given = owned.clone();
            given.quantity = amount;
            given.location = Location::Bag(0);
            commands.entry(taker).or_default().1.push(Command::GrantFront(given));
        }
    }
    // The initiator's grants take the first ids (`Store::swap_items`).
    let list = |id: u64| {
        let (takes, grants) = commands.get(&id).cloned().unwrap_or_default();
        takes.into_iter().chain(grants).collect::<Vec<_>>()
    };
    let (a_items, b_items) = (list(swap.initiator), list(swap.responder));
    // New ids: the initiator's grants first, then the responder's (`Store::swap_items`).
    let mut id = first;
    let renumber = |items: Vec<Command>, id: &mut u64| -> Vec<Command> {
        items
            .into_iter()
            .map(|command| match command {
                Command::GrantFront(mut item) => {
                    item.id = *id;
                    *id += 1;
                    Command::GrantFront(item)
                }
                other => other,
            })
            .collect()
    };
    let a_items = renumber(a_items, &mut id);
    let b_items = renumber(b_items, &mut id);
    store.swap_items(swap.initiator, &a_items, swap.responder, &b_items, first)?;
    let mut out = Vec::new();
    for (party, items) in [(swap.initiator, &a_items), (swap.responder, &b_items)] {
        let bag = store.characters[&party].inventory.clone();
        let mut events = Vec::new();
        for command in items {
            match command {
                Command::ConsumeStackDense { item, .. } => {
                    let instance = scene.inventory_instance(party, *item).map_err(other)?;
                    events.push(crate::item_services::removed(*item).map_err(other)?);
                    events.push(world::destroy_inventory_item(*item, instance));
                    scene.inventory_removed(party, *item).map_err(other)?;
                }
                Command::ConsumePart { item, .. } => {
                    let reduced = bag.items().find(|owned| owned.id == *item).ok_or_else(|| invalid("reduced stack missing"))?;
                    let old = scene.inventory_instance(party, *item).map_err(other)?;
                    let new = scene.next_item_instance(party, *item).map_err(other)?;
                    events.push(world::destroy_inventory_item(*item, old));
                    events.push(world::create_inventory_item(reduced, party, new).map_err(other)?);
                    scene.apparel_changed(party, *item, new).map_err(other)?;
                }
                Command::GrantFront(granted) => {
                    let item = bag.items().find(|owned| owned.id == granted.id).ok_or_else(|| invalid("granted item missing"))?;
                    let instance = scene.admit_inventory_grant(party, item.id).map_err(other)?;
                    events.push(world::create_inventory_item(item, party, instance).map_err(other)?);
                    let info = metadata.get(&item.definition).and_then(|info| *info).ok_or_else(|| invalid("item metadata missing"))?;
                    events.push(crate::item_services::acquired(party, item, info).map_err(other)?);
                    scene.inventory_granted(party, item.id, instance).map_err(other)?;
                }
                _ => {}
            }
        }
        out.extend(events.into_iter().map(|data| crate::scene::Delivery { recipient: party, data: data.into() }));
    }
    Ok(out)
}
