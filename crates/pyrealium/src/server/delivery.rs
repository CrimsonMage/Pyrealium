//! Scene deliveries to connected peers, inventory spawns and selection lists.
use super::*;

pub(super) fn inventory_spawn(
    item: &crate::inventory::Item,
    owner: u64,
    instance: u16,
    parent_instance: u16,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
) -> io::Result<Vec<u8>> {
    if matches!(item.location, crate::inventory::Location::Equipped(_)) {
        if let Some(hand) = metadata
            .get(&item.definition)
            .and_then(|entry| *entry)
            .and_then(|entry| entry.equipment)
        {
            if hand.hand_slot(item).is_some() {
                return world::create_hand_item(
                    item,
                    owner,
                    instance,
                    parent_instance,
                    hand.primary,
                )
                .map_err(other);
            }
        }
    }
    world::create_inventory_item(item, owner, instance).map_err(other)
}

pub(super) fn selection_list(
    store: &Store,
    account: &str,
    profile: Option<&crate::bootstrap::Bootstrap>,
    visual: Option<u32>,
) -> io::Result<Vec<u8>> {
    selection_list_excluding(store, account, profile, visual, None)
}
pub(super) fn selection_list_excluding(
    store: &Store,
    account: &str,
    profile: Option<&crate::bootstrap::Bootstrap>,
    visual: Option<u32>,
    excluded: Option<u64>,
) -> io::Result<Vec<u8>> {
    if profile.is_none() && visual.is_none() {
        return world::empty_character_set(account.as_bytes(), 5).map_err(other);
    }
    let selection: Vec<_> = store
        .characters
        .values()
        .filter(|c| c.account == account && !c.deleted && Some(c.id) != excluded)
        .filter_map(|c| {
            if let Some(profile) = profile {
                c.creation
                    .as_ref()
                    .filter(|r| profile.supports(r))
                    .map(|r| {let mut selection=profile.selection(c.id,r);selection.appearance=crate::starter_gear::appearance(c).ok();selection})
            } else {
                Some(world::SelectionCharacter {
                    id: c.id,
                    name: c.name.clone(),
                    seconds_greyed_out: 0,
                    parent_visual_did: visual.unwrap(),
                    appearance: None,
                })
            }
        })
        .collect();
    world::character_set(account.as_bytes(), 5, &selection).map_err(other)
}

pub(super) fn deliver_scene(
    deliveries: Vec<crate::scene::Delivery>,
    active: &BTreeMap<u64, SocketAddr>,
    peers: &mut BTreeMap<SocketAddr, Peer>,
    _socket: &UdpSocket,
    _now: Instant,
    failed: &mut BTreeSet<SocketAddr>,
) -> io::Result<()> {
    for delivery in deliveries {
        let Some(addr) = active.get(&delivery.recipient) else {
            continue;
        };
        let Some(peer) = peers.get_mut(addr) else {
            continue;
        };
        if failed.contains(addr) {
            continue;
        }
        if peer.queue_scene(delivery.data).is_err() {
            failed.insert(*addr);
        }
    }
    Ok(())
}
