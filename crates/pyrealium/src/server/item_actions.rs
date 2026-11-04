//! Merchants, gathering, crafting, corpses and inventory stacks.
use super::*;

pub(super) fn merchant_ready(character: u64, scene: &crate::scene::Scene, target: Option<u64>) -> io::Result<u64> {
    scene.nearby_vendor(character, target)
        .ok_or_else(|| invalid("Move within four metres of the vendor whose shop you opened."))
}

pub(super) fn merchant_action(
    peer: &mut Peer,
    character: u64,
    action: crate::merchants::Action,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
    catalog: &crate::merchants::Catalog,
    next_offer: &mut u64,
    now: u64,
    npc_target: Option<u64>,
    trade: Option<&crate::tradeskills::Assets>,
    names: &crate::examination::Names,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let preferred = npc_target.or_else(|| {
        (action != crate::merchants::Action::List)
            .then(|| peer.native_shop.as_ref().map(|shop| shop.merchant())).flatten()
    });
    let merchant = merchant_ready(character, scene, preferred)?;
    let saved = &store.characters[&character];
    if !saved.progression.alive() || peer.combat {
        return Err(invalid("Trading requires a living player out of combat."));
    }
    if peer
        .last_inventory_ms
        .is_some_and(|last| now.saturating_sub(last) < 250)
    {
        return Err(invalid("Wait briefly before another inventory action."));
    }
    if action == crate::merchants::Action::List {
        // A trader whose entity names a store record of the DAT (the raw-material traders) sells
        // that record's lines; every other vendor is the development quartermaster.
        let trader = trade.and_then(|assets| {
            let entity = scene.examination_content(character, merchant)?.entity;
            assets.trader_store(entity).map(|(store, lines)| (store, lines.to_vec()))
        });
        let count = trader.as_ref().map_or_else(
            || if catalog.entries().count() > 4 { crate::merchants::STOCK.len() as u64 } else { 4 },
            |(_, lines)| lines.len() as u64,
        );
        let last_offer = next_offer
            .checked_add(count)
            .filter(|id| id >> 48 == 0x8160)
            .ok_or_else(|| invalid("Shop offer identities exhausted; restart required."))?;
        let shop = if let Some((store, lines)) = trader {
            let offers = (0..count).map(|index| *next_offer + 1 + index).collect();
            let sales = lines
                .into_iter()
                .map(|line| Ok((line, names.get(line.definition, "Item").wire().map_err(other)?)))
                .collect::<io::Result<Vec<_>>>()?;
            crate::merchants::Shop::dat(merchant, offers, crate::merchants::DatShop { store, sales }).map_err(other)?
        } else {
            let offers: Vec<u64> = (0..count).map(|index| *next_offer + 1 + index).collect();
            crate::merchants::Shop::new(merchant, offers).map_err(other)?
        };
        let mut events = if npc_target.is_some() {
            npc_interaction_events(
                character,
                merchant,
                scene,
                "Merchant",
                &if shop.is_trader() { "Browse my wares.".to_owned() } else { crate::merchants::listing() },
            )?
        } else {
            vec![script::chat(&crate::merchants::listing(), 1).map_err(other)?]
        };
        events.push(shop.open(catalog).map_err(other)?);
        peer.queue_ordered_batch(events)?;
        *next_offer = last_offer;
        peer.native_shop = Some(shop);
        peer.merchant_open_ms = Some(now);
        peer.last_inventory_ms = Some(now);
        return Ok(Vec::new());
    }
    let plan = match action {
        crate::merchants::Action::Buy(stock) => crate::merchants::plan_buy(
            stock,
            &saved.inventory,
            &saved.progression,
            store.next_item_id()?,
        ),
        crate::merchants::Action::SellSlot(slot) => {
            crate::merchants::plan_sell(slot, &saved.inventory, &saved.progression)
        }
        crate::merchants::Action::List => unreachable!(),
    }
    .map_err(io::Error::other)?;
    let mut completion = plan.events;
    completion.push(script::chat(&plan.message, 1).map_err(other)?);
    let updates = item_service_transaction(
        peer,
        character,
        &plan.items,
        &plan.changes,
        completion,
        store,
        scene,
        metadata,
    )?;
    peer.merchant_open_ms = Some(now);
    peer.last_inventory_ms = Some(now);
    Ok(updates)
}

pub(super) fn use_gathering(peer:&mut Peer,character:u64,item:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,loot:&crate::loot::Loot,geometry:Option<&mut crate::geometry::Geometry>,now_ms:u64)->io::Result<Vec<crate::scene::Delivery>>{
    use crate::{harvesting as h,professions as p,inventory::{Command,Location}};
    if peer.last_inventory_ms.is_some_and(|last|now_ms.saturating_sub(last)<1500){return Err(invalid("Wait for your gathering action to finish."))}
    let saved=&store.characters[&character];let time=crate::clock::now();
    if !scene.can_observe(character,character)||!saved.progression.alive()||peer.combat{return Err(invalid("Gathering requires a ready living player in peace mode."))}
    let owned=saved.inventory.items().find(|i|i.id==item&&matches!(i.location,Location::Bag(_)));
    if let Some(map_item)=owned.filter(|i|h::map(i.definition).is_some()){
        let j=saved.professions.1.jobs.get(&item).ok_or_else(||invalid("This map has no active survey. Create a new map from the Surveying profession."))?;
        let mut point=scene.player_placement(character).ok_or_else(||invalid("Enter the world before surveying."))?;
        if !j.distance(point).is_some_and(|d|d<=40.){return Err(invalid(&j.guidance()))}
        let geometry=geometry.ok_or_else(||invalid("Surveying requires native world geometry."))?;
        if !geometry.contact(point)||!geometry.dry_ground(point){return Err(invalid("Stand on dry, solid ground at the survey site."))}
        // Place beside the finder on reachable ground, never through the player.
        point=[[0.8,0.,0.],[-0.8,0.,0.],[0.,0.8,0.],[0.,-0.8,0.]].into_iter().find_map(|step|{
            let mut p=point;for _ in 0..3{p=geometry.walk_step(p,p.translated(step)?,0.3)?;}
            (geometry.dry_ground(p)&&j.distance(p).is_some_and(|d|d<=40.)).then_some(p)
        }).ok_or_else(||invalid("Move into a clear area near the survey site before using this map."))?;
        let m=h::mine(h::map(map_item.definition).unwrap().mine).ok_or_else(||invalid("Mine profile missing"))?;
        let object=crate::content::Object::resource(h::node_id(item),m.id,m.visual,point)?;
        scene.prepare_cell(point.cell)?;scene.check_content_live(&[],std::slice::from_ref(&object))?;
        let change=p::Change{skill:h::SURVEY,xp:0,time,raise:false,activity:Some(h::Activity::Locate{item,point:[point.cell,point.coordinates[0].to_bits(),point.coordinates[1].to_bits(),point.coordinates[2].to_bits()]})};
        let events=vec![script::chat("Deposit found. Select it and use the appropriate gathering tool. It can be harvested three times.",1).map_err(other)?];
        let mut updates=item_service_transaction_profile(peer,character,&[Command::ConsumeDense{item}],&[],events,store,scene,metadata,false,false,None,Some(&change),None)?;
        updates.extend(scene.insert_content_live(object,true)?);peer.last_inventory_ms=Some(now_ms);return Ok(updates);
    }
    let node=if item>>48==0x8190{item}else{target};
    let mut entropy=[0;4];getrandom::getrandom(&mut entropy).map_err(other)?;
    let (tid,activity,material,base,variance)=if node>>48==0x8190{
        let mid=h::map_id(node);let j=saved.professions.1.jobs.get(&mid).filter(|j|j.expires>time&&j.remaining>0).ok_or_else(||invalid("That deposit has expired or belongs to another player."))?;
        let mine=h::mine(h::map(j.definition).unwrap().mine).unwrap();
        let skill=match mine.material{0x470010f2=>0x6f0025a8,0x470010f3=>0x6f002579,0x470010fe=>0x6f0025a6,0x470010f4=>0x6f0025a7,0x470010f5=>0x6f0025a9,_=>return Err(invalid("Unsupported mineral"))};
        let t=if item==node{saved.inventory.items().filter(|i|matches!(i.location,Location::Bag(_))&&h::tool(i.definition).is_some_and(|t|t.skill==skill)&&i.roll.is_none_or(|r|r.level<=1||saved.professions.get(skill).level>=u32::from(r.level)*3)).max_by_key(|i|i.roll.map_or(1,|r|r.level))}else{owned};
        let t=t.filter(|i|h::tool(i.definition).is_some_and(|t|t.skill==skill)).ok_or_else(||invalid("Carry the correct tool in your backpack: pickaxe for iron, mattock for silver, chisel for crystal, rock hammer for stone, or hatchet for wood."))?;
        (t.id,h::Activity::Mine{item:mid,tool:t.id,definition:t.definition,wear:None},mine.material,mine.quantity,mine.variance)
    }else{
        let t=owned.filter(|i|h::tool(i.definition).is_some_and(|t|t.skill==h::BUTCHERY)).ok_or_else(||invalid("Select your defeated creature and use a hunting knife from your backpack."))?;
        if loot.get(target,character,now_ms).is_none(){return Err(invalid("Butchery requires a recent corpse from your kill."))}
        let corpse=scene.examination_content(character,target).filter(|o|o.stat(0x102)==Some(0)&&o.stat(0x107).is_some()).ok_or_else(||invalid("Select a defeated creature."))?;
        let profile=p::catalog().butcher.iter().find(|b|b.id==corpse.entity).ok_or_else(||invalid("This creature cannot be butchered."))?;
        (t.id,h::Activity::Butcher{corpse:target,instance:corpse.instance_stamp(),tool:t.id,definition:t.definition,wear:None},profile.material,5,0.15)
    };
    if !scene.combat_distance(character,node).is_some_and(|d|d<=4.){return Err(invalid("Move within four meters to gather."))}
    let plan=h::gather(&saved.professions,&saved.inventory,tid,activity,material,base,variance,store.next_item_id()?,u32::from_le_bytes(entropy),time)?;
    let next=saved.professions.apply(&plan.change)?;
    let tool_def=saved.inventory.items().find(|i|i.id==tid).unwrap().definition;
    let remaining=h::tool(tool_def).unwrap().durability-next.1.tool_uses.get(&tid).copied().unwrap_or(h::tool(tool_def).unwrap().durability);
    let events=vec![crate::crafting::skill_update(plan.change.skill,&next).map_err(other)?,script::chat(&format!("Gathered {}. Tool durability remaining: {remaining}.",plan.name),1).map_err(other)?];
    let depleted=node>>48==0x8190&&!next.1.jobs.contains_key(&h::map_id(node));
    let mut updates=item_service_transaction_profile(peer,character,&plan.items,&[],events,store,scene,metadata,false,false,None,Some(&plan.change),None)?;
    if depleted{updates.extend(scene.remove_content_live(node)?);}peer.last_inventory_ms=Some(now_ms);Ok(updates)
}

pub(super) fn craft_item(
    peer: &mut Peer,
    character: u64,
    request: &crate::crafting::ExecuteRequest,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
    now: u64,
    geometry: Option<&mut crate::geometry::Geometry>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    if peer
        .last_inventory_ms
        .is_some_and(|last| now.saturating_sub(last) < crate::crafting::COOLDOWN_MS)
    {
        return Err(invalid("Wait one second before crafting again."));
    }
    let saved = &store.characters[&character];
    if !scene.can_observe(character,character)||!saved.progression.alive()||peer.combat{return Err(invalid("Crafting requires a ready living player in peace mode."))}
    if request.recipe==crate::arwic_mines::RECIPE {
        if request.spinner!=0 || request.ingredients.len()!=1 || request.ingredients.get(&1)!=Some(&request.primary) {return Err(invalid("The Arwic Mines recipe needs one Iron ingredient slot and no quality override."));}
        let events=vec![crate::crafting::completed(request.recipe,true).map_err(other)?];
        let updates=arwic_exchange(peer,character,crate::arwic_mines::Action::Craft(request.primary),events,store,scene,metadata)?;
        peer.last_inventory_ms=Some(now);return Ok(updates);
    }
    if crate::crafting::recipe(request.recipe).is_none() {
        if request.ingredients.is_empty()&&request.primary!=character{return Err(invalid("Survey helper must be your character."))}
        crate::professions::require_workshop(scene,character,request.recipe)?;
        let mut entropy=[0u8;4];getrandom::getrandom(&mut entropy).map_err(|e|invalid(&format!("Craft randomness unavailable: {e}")))?;
        let mut plan=crate::professions::plan(&saved.professions,&saved.inventory,request,store.next_item_id()?,u32::from_le_bytes(entropy),crate::clock::now(),metadata)?;
        if matches!(plan.change.activity,Some(crate::harvesting::Activity::Survey{..})) {
            crate::harvesting::ground_survey(&saved.professions,&mut plan.change,geometry.ok_or_else(||invalid("Surveying requires native world geometry."))?)?;
        }
        let next=saved.professions.apply(&plan.change)?;
        let gained=next.get(plan.change.skill).available.saturating_sub(saved.professions.get(plan.change.skill).available);
        let mut events=vec![crate::crafting::completed(request.recipe,plan.success).map_err(other)?,crate::crafting::skill_update(plan.change.skill,&next).map_err(other)?,script::chat(&format!("{} {}. Gained {gained} crafting XP.",if plan.success{"Crafted"}else{"Failed to craft"},plan.name),1).map_err(other)?];
        if let Some(crate::harvesting::Activity::Survey{item,..})=&plan.change.activity {events.push(script::chat(&next.1.jobs[item].guidance(),1).map_err(other)?);}
        let costs=if plan.gold==0{vec![]}else{vec![crate::progression::Command::SpendGold(plan.gold)]};
        let updates=item_service_transaction_profile(peer,character,&plan.items,&costs,events,store,scene,metadata,false,false,None,Some(&plan.change),None)?;
        peer.last_inventory_ms=Some(now);return Ok(updates);
    }
    let ingredients = request.require_inputs(&saved.inventory, true)
        .map_err(|_| invalid("Crafting needs one Mucor Mushroom and a full 100-unit Tallow bundle in your bag. Use @shop near the training quartermaster."))?;
    let mut items: Vec<_> = ingredients
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            if index == 1 {
                crate::inventory::Command::ConsumeStackDense {
                    item,
                    quantity: crate::crafting::TALLOW_QUANTITY,
                }
            } else {
                crate::inventory::Command::ConsumeDense { item }
            }
        })
        .collect();
    let mut bag = saved.inventory.clone();
    for item in &items {
        bag = bag.apply(item).map_err(io::Error::other)?;
    }
    let slot = bag
        .free_slot()
        .ok_or_else(|| invalid("There is no bag space for the crafted item."))?;
    let output = crate::crafting::output(request.recipe, store.next_item_id()?, slot)
        .ok_or_else(|| invalid("That recipe is not enabled."))?;
    let name = if request.recipe == crate::crafting::HEALTH_RECIPE {
        "health"
    } else {
        "vigor"
    };
    items.push(crate::inventory::Command::Grant(output));
    let events = vec![
        crate::crafting::completed(request.recipe, true).map_err(other)?,
        script::chat(&format!("Crafted one {name} potion."), 1).map_err(other)?,
    ];
    let practice = saved.quests.potion_phase() == 1;
    let updates = item_service_transaction_quest(peer, character, &items, &[], events, store, scene, metadata, false, practice)?;
    peer.last_inventory_ms = Some(now);
    Ok(updates)
}

pub(super) fn corpse_action(peer:&mut Peer,character:u64,request:crate::loot::Request,loot:&mut crate::loot::Loot,
    store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    use crate::loot::Request;
    let container=match request {Request::Open(id)|Request::Close(id)=>id,Request::Take{container,..}|Request::Transmute{container,..}=>container};
    if matches!(request,Request::Close(_)){
        peer.queue_ordered_batch(vec![crate::loot::closed(container).map_err(other)?])?;return scene.close_chest(character,Some(container));
    }
    if !store.characters[&character].progression.alive() || !scene.combat_distance(character,container).is_some_and(|d|d<=4.) {
        return Err(invalid("Move within four meters of the defeated creature to loot it."));
    }
    if let Some(crate::services::Service::Chest{locked})=scene.examination_content(character,container).and_then(|o|o.service) {
        if locked{return Err(invalid("This chest requires a key or quest condition."));}
        if matches!(request,Request::Open(_)) {
            let events=vec![crate::loot::opened(container,None).map_err(other)?,
                script::chat("This chest has no configured contents.",1).map_err(other)?];
            peer.validate_scene_batch(&events)?;
            let updates=scene.open_chest(character,container)?;
            peer.queue_ordered_batch(events)?;return Ok(updates);
        }
        return Err(invalid("No loot is available in this chest."));
    }
    let reward=loot.get(container,character,now).ok_or_else(||invalid("No loot is available to your character."))?;
    let pending=reward.items.clone();
    if matches!(request,Request::Open(_)){
        loot.opened(container);
        let mut events=pending.iter().map(|item|world::create_inventory_item(item,container,1).map_err(other)).collect::<io::Result<Vec<_>>>()?;
        events.push(crate::loot::opened_items(container,&pending.iter().map(|i|i.id).collect::<Vec<_>>()).map_err(other)?);
        peer.queue_ordered_batch(events)?;
        return scene.close_chest(character,None);
    }
    if let Request::Transmute{player,..}=request {
        if player!=character{return Err(invalid("Loot request does not match this container."));}
        let eligible=pending.iter().filter_map(|i|crate::merchants::sale_value(i).map(|(_,price)|(i.id,price*u32::from(i.quantity)))).collect::<Vec<_>>();
        if eligible.is_empty(){return Err(invalid("No items in this container can be transmuted."));}
        let total=eligible.iter().map(|(_,price)|price).sum();
        let remaining=pending.iter().filter(|i|!eligible.iter().any(|(id,_)|*id==i.id)).map(|i|i.id).collect::<Vec<_>>();
        let mut completion=eligible.iter().map(|(id,_)|world::destroy_inventory_item(*id,1)).collect::<Vec<_>>();
        completion.push(if remaining.is_empty(){crate::loot::closed(container)}else{crate::loot::opened_items(container,&remaining)}.map_err(other)?);
        completion.push(script::chat(&format!("Loot transmuted for {total} gold."),1).map_err(other)?);
        // Journal the award before removing any corpse items. Quest and unknown
        // item profiles have no sale value and remain in the container.
        let updates=item_service_transaction(peer,character,&[],&[crate::progression::Command::AwardGold(total)],completion,store,scene,metadata)?;
        for (id,_) in eligible{loot.claimed(container,id);}
        loot.looted(container,character);
        return Ok(updates);
    }
    let Request::Take{player,item,..}=request else {unreachable!()};
    if player!=character || item.is_some_and(|id|!pending.iter().any(|i|i.id==id)){return Err(invalid("Loot request does not match this container."));}
    if pending.is_empty(){return Err(invalid("This container is empty."));}
    let mut updates=Vec::new();let mut collected=0;
    for pending in pending.iter().filter(|i|item.is_none_or(|id|id==i.id)) {
        let Some(slot)=store.characters[&character].inventory.free_slot() else {
            peer.queue_ordered_batch(vec![script::chat("Your backpack is full. Remaining loot stays in the container.",1).map_err(other)?])?;break;
        };
        let mut owned=pending.clone();owned.id=store.next_item_id()?;owned.location=crate::inventory::Location::Bag(slot);
        let remaining=loot.get(container,character,now).unwrap().items.iter().filter(|i|i.id!=pending.id).map(|i|i.id).collect::<Vec<_>>();
        let state=if remaining.is_empty(){crate::loot::closed(container)}else{crate::loot::opened_items(container,&remaining)}.map_err(other)?;
        let completion=vec![world::destroy_inventory_item(pending.id,1),state,script::chat("Loot collected.",1).map_err(other)?];
        // Each item is journaled before it leaves the corpse. A full bag or a
        // failed transaction retains every item that was not granted.
        if pending.definition==crate::arwic_mines::HAMMER {
            updates.extend(arwic_exchange(peer,character,crate::arwic_mines::Action::Hammer,completion,store,scene,metadata)?);
        } else {updates.extend(item_service_transaction(peer,character,&[crate::inventory::Command::Grant(owned)],&[],completion,store,scene,metadata)?);}
        loot.claimed(container,pending.id);loot.looted(container,character);collected+=1;
    }
    if collected==0 && store.characters[&character].inventory.free_slot().is_some(){return Err(invalid("No loot was collected."));}
    Ok(updates)
}
/// The stack a drop on an occupied slot merges into: the bag item at the request's destination
/// when it is another stack of the same kind as the moved one and has room for the amount
/// (`ContainerDisplay::HandleListBoxDrop` names the slot under the pointer, an item or not).
pub(super) fn merge_target(inventory:&crate::inventory::Inventory,request:&crate::inventory_request::MoveRequest)->Option<u64>{
    use crate::inventory::Location;
    let moved=inventory.items().find(|i|i.id==request.item && i.location==Location::Bag(request.source))?;
    let target=inventory.items().find(|i|i.location==Location::Bag(request.destination) && i.id!=request.item)?;
    (moved.stack_limit>1
        && (moved.definition,moved.visual,moved.stack_limit,moved.equipment_mask)==(target.definition,target.visual,target.stack_limit,target.equipment_mask)
        && request.quantity>0 && request.quantity<=moved.quantity
        && u32::from(target.quantity)+u32::from(request.quantity)<=u32::from(target.stack_limit))
        .then_some(target.id)
}
/// Merge `request.quantity` of a stack into the stack at the destination slot (`Command::Merge`).
/// The target is re-created with the new count, as every other quantity change is; a whole stack
/// leaves the pack (`item_services::removed`, the gap closes) and a part is re-created reduced.
/// A server-side development behaviour; no capture of a retail merge exists.
pub(super) fn merge_stacks(peer:&mut Peer, character:u64, request:&crate::inventory_request::MoveRequest,
    store:&mut Store, scene:&mut crate::scene::Scene,
    _metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,now_ms:u64) -> io::Result<()> {
    use crate::inventory::Command;
    if request.owner!=character {return Err(invalid("Item move does not match this character."));}
    let saved=&store.characters[&character];
    if !saved.progression.alive() {return Err(invalid("You cannot move items while dead."));}
    let into=merge_target(&saved.inventory,request).ok_or_else(||invalid("Those stacks cannot be merged."))?;
    let whole=saved.inventory.items().any(|i|i.id==request.item && i.quantity==request.quantity);
    let command=Command::Merge{item:request.item,into,quantity:request.quantity};
    let candidate=saved.inventory.apply(&command).map_err(other)?;
    let merged=candidate.items().find(|i|i.id==into).ok_or_else(||invalid("Target stack is absent."))?;
    let old_target=scene.inventory_instance(character,into).map_err(other)?;
    let new_target=scene.next_item_instance(character,into).map_err(other)?;
    let old_source=scene.inventory_instance(character,request.item).map_err(other)?;
    let mut events=vec![
        world::destroy_inventory_item(into,old_target),
        world::create_inventory_item(merged,character,new_target).map_err(other)?,
    ];
    let mut reduced_instance=None;
    if whole {
        events.push(crate::item_services::removed(request.item).map_err(other)?);
        events.push(world::destroy_inventory_item(request.item,old_source));
    } else {
        let reduced=candidate.items().find(|i|i.id==request.item).ok_or_else(||invalid("Source stack is absent."))?;
        let instance=scene.next_item_instance(character,request.item).map_err(other)?;
        events.push(world::destroy_inventory_item(request.item,old_source));
        events.push(world::create_inventory_item(reduced,character,instance).map_err(other)?);
        reduced_instance=Some(instance);
    }
    peer.validate_scene_batch(&events)?;
    store.inventory_command(&peer.session.account,character,command)?;
    scene.apparel_changed(character,into,new_target).map_err(other)?;
    match reduced_instance {
        Some(instance)=>scene.apparel_changed(character,request.item,instance).map_err(other)?,
        None=>scene.inventory_removed(character,request.item).map_err(other)?,
    }
    peer.last_inventory_ms=Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(())
}
/// Split part of a stack off into a new stack at the slot the drop named, or at the end of the
/// dense bag list when that is past it (`Command::Split`).
/// The original is re-created with what is left, as every other quantity change is, and the new
/// stack is created and announced with the answer to the request (`ReorganizeContents_Done`,
/// `MoveRequest::split_acknowledgment`): the new stack is its `splitItemID`, which the client's
/// script lists at the slot the answer names (`CInventorySystem::ReorganizeContents_Done`). A
/// server-side development behaviour; no capture of a retail split exists.
pub(super) fn split_stack(peer:&mut Peer, character:u64, request:&crate::inventory_request::MoveRequest,
    store:&mut Store, scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,now_ms:u64) -> io::Result<()> {
    use crate::inventory::{Command,Location};
    if request.owner!=character {return Err(invalid("Item move does not match this character."));}
    let saved=&store.characters[&character];
    if !saved.progression.alive() {return Err(invalid("You cannot move items while dead."));}
    let original=saved.inventory.items().find(|i|i.id==request.item && i.location==Location::Bag(request.source))
        .ok_or_else(||invalid("Item is not at that position."))?;
    if original.stack_limit<=1 || request.quantity==0 || request.quantity>=original.quantity {
        return Err(invalid("That amount cannot be split off."));
    }
    let end=saved.inventory.free_slot().ok_or_else(||invalid("Your backpack is full."))?;
    if !saved.inventory.dense_bags() {return Err(invalid("Bag sorting is unavailable for a sparse backpack."));}
    let destination=request.destination.min(end);
    let new_id=store.next_item_id()?;
    let command=Command::Split{item:request.item,new_id,quantity:request.quantity,destination};
    let candidate=saved.inventory.apply(&command).map_err(other)?;
    let reduced=candidate.items().find(|i|i.id==request.item).ok_or_else(||invalid("Item is not owned."))?;
    let split=candidate.items().find(|i|i.id==new_id).ok_or_else(||invalid("Split stack is absent."))?;
    let info=metadata.get(&split.definition).and_then(|m|*m).ok_or_else(||invalid("Bag sorting is unavailable for this item definition."))?;
    let old_instance=scene.inventory_instance(character,request.item).map_err(other)?;
    let reduced_instance=scene.next_item_instance(character,request.item).map_err(other)?;
    let split_instance=scene.admit_inventory_grant(character,new_id).map_err(other)?;
    let announced=crate::inventory_request::MoveRequest{owner:character,item:request.item,source:request.source,destination,quantity:split.quantity};
    let events=vec![
        world::destroy_inventory_item(request.item,old_instance),
        world::create_inventory_item(reduced,character,reduced_instance).map_err(other)?,
        world::create_inventory_item(split,character,split_instance).map_err(other)?,
        announced.split_acknowledgment(split,info).map_err(other)?,
    ];
    peer.validate_scene_batch(&events)?;
    store.inventory_command(&peer.session.account,character,command)?;
    scene.apparel_changed(character,request.item,reduced_instance).map_err(other)?;
    scene.inventory_granted(character,new_id,split_instance).map_err(other)?;
    peer.last_inventory_ms=Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(())
}
pub(super) fn drop_ground_item(peer:&mut Peer, character:u64, request:&crate::inventory_request::DropRequest,
    store:&mut Store, scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,now_ms:u64) -> io::Result<Vec<crate::scene::Delivery>> {
    if peer.last_inventory_ms.is_some_and(|last|now_ms.saturating_sub(last)<100) {return Err(invalid("Please wait before moving another item."));}
    let position=scene.player_placement(character).ok_or_else(||invalid("Wait until you arrive in the world."))?;
    if request.landblock != position.cell & 0xffff0000 {return Err(invalid("Your destination has changed. Try dropping the item again."));}
    let saved=&store.characters[&character];
    request.moved.checked_command(character,&saved.inventory).map_err(other)?;
    if saved.professions.1.jobs.get(&request.moved.item).is_some_and(|j|j.point.is_none()){
        return Err(invalid("Active survey maps belong to their surveyor and cannot be dropped or traded."));
    }
    let dropped=store.drop_candidate(&peer.session.account,character,request.moved.item,position)?;
    scene.validate_drop(&dropped)?;
    let info=metadata.get(&dropped.item.definition).and_then(|m|*m).ok_or_else(||invalid("This item's ground profile is unavailable."))?;
    // Validate the return-to-inventory profile before removing any ownership.
    crate::item_services::acquired(character,&dropped.item,info).map_err(other)?;
    let instance=scene.inventory_instance(character,dropped.item.id).map_err(other)?;
    let events=vec![crate::item_services::removed(dropped.item.id).map_err(other)?,
        world::destroy_inventory_item(dropped.item.id,instance),script::chat("Item dropped. Double-click it in the world to pick it up.",1).map_err(other)?];
    peer.validate_scene_batch(&events)?;
    store.drop_item(&peer.session.account,character,dropped.item.id,position)?;
    scene.inventory_removed(character,dropped.item.id).map_err(other)?;
    let deliveries=scene.add_drop(&dropped)?;
    peer.last_inventory_ms=Some(now_ms);peer.queue_ordered_batch(events)?;Ok(deliveries)
}
pub(super) fn pickup_ground_item(peer:&mut Peer,character:u64,id:u64,store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,now_ms:u64) -> io::Result<Vec<crate::scene::Delivery>> {
    if peer.last_inventory_ms.is_some_and(|last|now_ms.saturating_sub(last)<100) {return Err(invalid("Please wait before moving another item."));}
    if !scene.combat_distance(character,id).is_some_and(|d|d<=4.) {return Err(invalid("Move within four meters to pick up the item."));}
    let item=store.pickup_candidate(&peer.session.account,character,id)?;
    let info=metadata.get(&item.definition).and_then(|m|*m).ok_or_else(||invalid("Item metadata is unavailable."))?;
    let instance=scene.admit_inventory_grant(character,item.id).map_err(other)?;
    let events=vec![world::create_inventory_item(&item,character,instance).map_err(other)?,
        crate::item_services::acquired(character,&item,info).map_err(other)?,script::chat("Item picked up.",1).map_err(other)?];
    peer.validate_scene_batch(&events)?;
    store.pickup_item(&peer.session.account,character,id)?;
    let deliveries=scene.remove_drop(id)?;
    scene.inventory_granted(character,item.id,instance).map_err(other)?;
    peer.last_inventory_ms=Some(now_ms);peer.queue_ordered_batch(events)?;Ok(deliveries)
}
