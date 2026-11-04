//! Equipping, unequipping and replacing hand items; selection status.
use super::*;

pub(super) enum HandChange {
    Toggle,
    Equip { slot: u16, source: u16 },
    Unequip { slot: u16, destination: u16 },
}
pub(super) fn change_hand_item(
    peer: &mut Peer,
    character: u64,
    request: &crate::equipment::UseRequest,
    action: HandChange,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
    now: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    if request.owner != character {
        return Err(invalid("Item owner does not match this character."));
    }
    if peer
        .last_inventory_ms
        .is_some_and(|last| now.saturating_sub(last) < 100)
    {
        return Err(invalid("Wait briefly before changing equipment again."));
    }
    let saved = &store.characters[&character];
    if !saved.progression.alive() {
        return Err(invalid("You cannot change equipment while dead. Recall to safety first."));
    }
    if peer.combat {
        return Err(invalid("Return to peace with Z or the Stop Combat button, then change equipment."));
    }
    if !saved.inventory.dense_bags() {
        return Err(invalid("This equipment profile requires a dense bag list."));
    }
    let repair=saved.inventory.items().find(|i|i.id==request.item && i.equipment_mask==0 && i.quantity==1 && i.stack_limit==1)
        .and_then(|i|crate::apparel::recovered_slot(i.definition,i.visual).map(|mask|crate::inventory::Command::RepairEquipment{item:i.id,definition:i.definition,mask}));
    if let Some(command)=repair {store.inventory_command(&peer.session.account,character,command)?;}
    let saved=&store.characters[&character];
    let item = saved
        .inventory
        .items()
        .find(|item| item.id == request.item)
        .ok_or_else(|| invalid("Item is not owned."))?;
    if let Some(race)=crate::apparel::required_race(item.definition) {
        if !saved.creation.as_ref().is_some_and(|r|r.race==race) {
            return Err(invalid(match race {1=>"This equipment requires a Human.",2=>"This equipment requires a Lugian.",4=>"This equipment requires a Tumerok.",_=>"This equipment requires another race."}));
        }
    }
    let apparel = saved.creation.as_ref().and_then(|r| crate::starter_gear::apparel_slot(item,r.race).or_else(||crate::apparel::slot(item,r.race)));
    let hand = metadata
        .get(&item.definition)
        .and_then(|entry| *entry)
        .and_then(|entry| entry.equipment.or_else(|| apparel.map(|slot|crate::inventory_request::Equipment {
            valid:u32::from(slot), preferred:u32::from(slot), primary:0, secondary:0 })))
        .ok_or_else(|| invalid("This item-use profile is not available."))?;
    let slot = apparel.or_else(||hand.hand_slot(item))
        .ok_or_else(|| invalid("This item's equipment appearance is not supported."))?;
    if hand.primary == 3 && !saved.creation.as_ref().is_some_and(|request| request.race == 4) {
        return Err(invalid("This instrument requires a Tumerok holding location."));
    }
    let equipped = !matches!(action, HandChange::Unequip { .. })
        && matches!(item.location, crate::inventory::Location::Bag(_));
    if let HandChange::Equip {
        slot: requested,
        source,
    } = action
    {
        if requested != slot || item.location != crate::inventory::Location::Bag(source) {
            return Err(invalid(
                "The equip source or supported hand slot has changed.",
            ));
        }
    }
    if equipped {
        crate::treasure_weapons::check_level(item,saved.progression.level().number)?;
        if crate::professions::creation_tool(item.definition).is_some()&&!crate::professions::creation_tool_usable(&saved.professions,item) {
            return Err(invalid("This tool is exhausted or requires crafting rank at least ten times its level in a matching profession."));
        }
        let previous=saved.inventory.items().find(|other|other.id!=item.id && other.location==crate::inventory::Location::Equipped(slot)).map(|i|i.id);
        if let Some(previous)=previous {
            return replace_equipment(peer,character,item.id,previous,slot,store,scene,metadata,now);
        }
    }
    let command = if let HandChange::Unequip {
        slot: expected_slot,
        destination: requested,
    } = action
    {
        if item.location != crate::inventory::Location::Equipped(expected_slot)
            || slot != expected_slot
        {
            return Err(invalid("The item is no longer equipped in that slot."));
        }
        let count = saved
            .inventory
            .items()
            .filter(|item| matches!(item.location, crate::inventory::Location::Bag(_)))
            .count() as u16;
        if requested >= crate::inventory::BAG_SLOTS {
            return Err(invalid("That backpack position is unavailable."));
        }
        crate::inventory::Command::UnequipAt {
            item: item.id,
            slot,
            destination: requested.min(count),
        }
    } else if equipped {
        crate::inventory::Command::Wear {
            item: item.id,
            slot,
        }
    } else {
        crate::inventory::Command::Unequip { item: item.id }
    };
    let candidate = saved.inventory.apply(&command).map_err(other)?;
    let changed = candidate
        .items()
        .find(|item| item.id == request.item)
        .unwrap();
    let bag_slot = match changed.location {
        crate::inventory::Location::Bag(index) => index,
        _ => 0,
    };
    let other_hands = saved
        .inventory
        .items()
        .filter(|other| other.id != item.id)
        .fold(0, |mask, other| match other.location {
            crate::inventory::Location::Equipped(slot) => mask | (slot & 0xc0),
            _ => mask,
        });
    let acknowledgment = crate::ui::hand_equipment_at(
        character,
        item.id,
        slot,
        other_hands,
        hand,
        equipped,
        bag_slot,
    )
    .map_err(other)?;
    let instance = scene
        .next_item_instance(character, item.id)
        .map_err(other)?;
    let parent_instance = peer
        .motion
        .as_ref()
        .ok_or_else(|| invalid("Equipment has no avatar lifetime."))?
        .instance;
    let spawn = inventory_spawn(changed, character, instance, parent_instance, metadata)?;
    if apparel.is_some() {
        let creation=saved.creation.as_ref().unwrap();
        let visual=crate::appearance::body(creation.race,creation.sex).unwrap()
            .visual_desc_equipped(&creation.physical_values,crate::starter_gear::worn_mask(&candidate,creation.race),&crate::apparel::aspects(&candidate,creation.race,creation.sex)).map_err(other)?;
        let (next,stamp)=peer.emotes.next_visual(saved.instance).map_err(other)?;
        let appearance=crate::combat_visual::appearance(character,saved.instance,stamp,&visual).map_err(other)?;
        let old_instance=scene.inventory_instance(character,item.id).map_err(other)?;
        let events=vec![world::destroy_inventory_item(item.id,old_instance),spawn,acknowledgment,appearance.clone()];
        peer.validate_scene_batch(&events)?;
        let updates=scene.broadcast(character,appearance).map_err(other)?;
        store.inventory_command(&peer.session.account,character,command)?;
        scene.apparel_changed(character,request.item,instance).map_err(other)?;
        scene.replace_appearance(character,&visual,stamp).map_err(other)?;
        peer.emotes=next;peer.last_inventory_ms=Some(now);peer.queue_ordered_batch(events)?;
        return Ok(updates.into_iter().filter(|d|d.recipient!=character).collect());
    }
    scene
        .validate_hand_change(character, item.id, &spawn, equipped)
        .map_err(other)?;
    let old_instance = scene
        .inventory_instance(character, item.id)
        .map_err(other)?;
    let events = vec![
        world::destroy_inventory_item(item.id, old_instance),
        spawn.clone(),
        acknowledgment,
    ];
    peer.validate_scene_batch(&events)?;
    store.inventory_command(&peer.session.account, character, command)?;
    let updates = scene
        .hand_change(character, request.item, spawn, equipped)
        .map_err(other)?;
    peer.last_inventory_ms = Some(now);
    peer.queue_ordered_batch(events)?;
    Ok(updates
        .into_iter()
        .filter(|delivery| delivery.recipient != character)
        .collect())
}

/// Swap an occupied slot in one durable inventory command, even with a full
/// backpack. Admit both native item lifetimes before committing either change.
pub(super) fn replace_equipment(
    peer:&mut Peer, character:u64, incoming:u64, outgoing:u64, slot:u16,
    store:&mut Store, scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>, now:u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    use crate::inventory::{Command,Location};
    let saved=&store.characters[&character];
    let command=Command::Equip{item:incoming,slot};
    let candidate=saved.inventory.apply(&command).map_err(other)?;
    let creation=saved.creation.as_ref().ok_or_else(||invalid("Character appearance is unavailable."))?;
    let parent_instance=peer.motion.as_ref().ok_or_else(||invalid("Equipment has no avatar lifetime."))?.instance;
    let other_hands=candidate.items().filter(|i|i.id!=incoming).fold(0,|mask,i|match i.location{Location::Equipped(s)=>mask|(s&0xc0),_=>mask});
    let mut events=Vec::new();let mut lifetimes=Vec::new();let mut apparel_changed=false;
    // Equipping removes the source from the dense bag. Returning the displaced
    // item to that same ordinal inserts it again, preserving all other slots.
    for id in [incoming,outgoing] {
        let item=candidate.items().find(|i|i.id==id).ok_or_else(||invalid("Equipment changed."))?;
        let apparel=crate::starter_gear::apparel_slot(item,creation.race).or_else(||crate::apparel::slot(item,creation.race));
        let hand=metadata.get(&item.definition).and_then(|m|*m).and_then(|m|m.equipment)
            .or_else(||apparel.map(|s|crate::inventory_request::Equipment{valid:u32::from(s),preferred:u32::from(s),primary:0,secondary:0}))
            .ok_or_else(||invalid("Replacement equipment profile is unavailable."))?;
        let instance=scene.next_item_instance(character,id).map_err(other)?;
        let spawn=inventory_spawn(item,character,instance,parent_instance,metadata)?;
        let equipped=id==incoming;
        if apparel.is_none(){scene.validate_hand_change(character,id,&spawn,equipped).map_err(other)?;}
        let old=scene.inventory_instance(character,id).map_err(other)?;
        events.push(world::destroy_inventory_item(id,old));events.push(spawn.clone());
        let destination=match item.location{Location::Bag(index)=>index,_=>0};
        events.push(crate::ui::hand_equipment_at(character,id,slot,other_hands,hand,equipped,destination).map_err(other)?);
        apparel_changed|=apparel.is_some();lifetimes.push((id,instance,spawn,equipped,apparel.is_some()));
    }
    let appearance=if apparel_changed {
        let visual=crate::appearance::body(creation.race,creation.sex).ok_or_else(||invalid("Unsupported appearance."))?
            .visual_desc_equipped(&creation.physical_values,crate::starter_gear::worn_mask(&candidate,creation.race),&crate::apparel::aspects(&candidate,creation.race,creation.sex)).map_err(other)?;
        let (next,stamp)=peer.emotes.next_visual(saved.instance).map_err(other)?;
        let event=crate::combat_visual::appearance(character,saved.instance,stamp,&visual).map_err(other)?;
        events.push(event.clone());Some((visual,next,stamp,event))
    } else {None};
    peer.validate_scene_batch(&events)?;
    store.inventory_command(&peer.session.account,character,command)?;
    let mut updates=Vec::new();
    for(id,instance,spawn,equipped,apparel)in lifetimes {
        if apparel {scene.apparel_changed(character,id,instance).map_err(other)?;}
        else {updates.extend(scene.hand_change(character,id,spawn,equipped).map_err(other)?);}
    }
    if let Some((visual,next,stamp,event))=appearance {
        updates.extend(scene.broadcast(character,event).map_err(other)?);
        scene.replace_appearance(character,&visual,stamp).map_err(other)?;peer.emotes=next;
    }
    peer.last_inventory_ms=Some(now);peer.queue_ordered_batch(events)?;
    Ok(updates.into_iter().filter(|d|d.recipient!=character).collect())
}

pub(super) fn selection_status(
    observer: u64,
    target: u64,
    scene: &crate::scene::Scene,
    store: &Store,
) -> io::Result<Vec<u8>> {
    let vitals = store
        .characters
        .get(&target)
        .filter(|character| !character.deleted)
        .map(|character| {
            let state = &character.progression;
            [
                state.health,
                state.level().health,
                state.vigor,
                state.level().vigor,
            ]
        })
        .or_else(|| {
            let object = scene.examination_content(observer, target)?;
            Some([object.stat(0x102)?, object.stat(0x107)?, object.stat(0x103).unwrap_or(0), object.stat(0x108).unwrap_or(0)])
        });
    crate::ui::selection(target, vitals).map_err(other)
}
