//! The tradeskill actions of one connection (a child module of `server`): crafting a
//! Surveying or other DAT recipe, using a surveyor's map, working a deposit.
//! Sources and policy: `tradeskills.rs`.
use super::{invalid, item_service_transaction_quest, other, Peer};
use crate::{
    content,
    crafting::{self, ExecuteRequest},
    inventory::{Command, Inventory, Item, Location},
    inventory_request::Metadata,
    scene::{Delivery, Scene},
    script,
    store::Store,
    tradeskills::{self, Assets},
};
use std::{
    collections::BTreeMap,
    io,
    sync::atomic::{AtomicU64, Ordering},
};

/// A prospected deposit and who may work it (retail: "Only the surveyor who discovered this
/// outcropping may extract its ore").
#[derive(Clone, Copy, Debug)]
pub(super) struct Deposit {
    pub owner: u64,
    pub entity: u32,
    pub charges: u32,
    pub point: crate::movement::Placement,
}

static SEQUENCE: AtomicU64 = AtomicU64::new(1);
fn roll(now: u64) -> f32 {
    // Policy: a splitmix64 of the clock and a counter; retail's dice are not recoverable.
    let mut z = now ^ SEQUENCE.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) % 1000) as f32
}

/// The craft registry packets for this connection's skills and known recipes.
pub(super) fn registry(peer: &Peer, assets: &Assets) -> io::Result<Vec<Vec<u8>>> {
    crafting::initialization_with(&peer.trade.skills(), &peer.trade.known_recipes(&assets.catalog))
        .map_err(other)
}

fn free_slot_after(inventory: &Inventory, commands: &[Command]) -> io::Result<u16> {
    let mut bag = inventory.clone();
    for command in commands {
        bag = bag.apply(command).map_err(io::Error::other)?;
    }
    bag.free_slot().ok_or_else(|| invalid("There is no bag space."))
}

/// Execute a catalog recipe (anything but the two potion recipes): check the ingredients, take
/// them, roll the outcome, create the item, and add the skill's crafting experience.
pub(super) fn craft(
    peer: &mut Peer,
    character: u64,
    request: &ExecuteRequest,
    assets: &Assets,
    store: &mut Store,
    scene: &mut Scene,
    metadata: &BTreeMap<u32, Option<Metadata>>,
    now: u64,
) -> io::Result<Vec<Delivery>> {
    let recipe = assets
        .catalog
        .get(request.recipe)
        .filter(|r| peer.trade.known_recipes(&assets.catalog).contains(&r.id))
        .ok_or_else(|| invalid("You do not know that recipe."))?;
    let saved = &store.characters[&character];
    let mut commands = Vec::new();
    let mut taken: Vec<u64> = Vec::new();
    for slot in &recipe.ingredients {
        // The stack the panel's slot holds; an ingredient the panel showed no slot for (the
        // client lists a slot only at the spinner value that reaches its count: a pickaxe's coal,
        // steel, tallow and pyreal stand in no slot at the spinner it opens on) is the first
        // stack of the pack that fills it. Policy: retail's server is unknown.
        let id = match request.ingredients.get(&slot.ordinal) {
            Some(id) => *id,
            None => saved
                .inventory
                .items()
                .find(|i| {
                    matches!(i.location, Location::Bag(_))
                        && !taken.contains(&i.id)
                        && slot.allowed.contains(&i.definition)
                        && u32::from(i.quantity) >= slot.quantity
                })
                .map(|i| i.id)
                .ok_or_else(|| invalid("An ingredient is missing."))?,
        };
        let owned = saved
            .inventory
            .items()
            .find(|i| i.id == id && matches!(i.location, Location::Bag(_)))
            .ok_or_else(|| invalid("An ingredient is not in your bag."))?;
        // What the client's `CheckIngredient` takes for a slot: one stack of the allowed kind
        // holding at least the slot's quantity (probed live: a stack of 10 is refused for 100).
        if taken.contains(&id) || !slot.allowed.contains(&owned.definition) || u32::from(owned.quantity) < slot.quantity {
            return Err(invalid("An ingredient does not fit the recipe."));
        }
        taken.push(id);
        commands.push(if u32::from(owned.quantity) == slot.quantity {
            Command::ConsumeStackDense { item: id, quantity: owned.quantity }
        } else {
            Command::ConsumePart { item: id, quantity: slot.quantity }
        });
    }
    // What the recipe makes: its item outcome, else (classic refining) the skill's refined
    // material in the rank's yield (`tradeskills::refined_product`, `refine_yield`). Policy: one
    // stack, at most `STACK` units (the item transaction grants one item).
    // A study recipe makes nothing: the DAT's description of `rcp_spellbinding_study` (string
    // table 25007d5d) says "study Spellbinding by crafting and discarding sample totems. You will
    // earn no profits, but this is the fastest way to improve your skills", so the ingredients
    // are used up and the skill gains its experience.
    let made = match tradeskills::pick_outcome(&recipe.outcomes, roll(now)) {
        Some(outcome) => Some((outcome.output, 1)),
        None if tradeskills::classic_refining(recipe) => {
            let product = tradeskills::refined_product(recipe.skill).expect("classic refining names its product");
            let units = tradeskills::refine_yield(recipe.ingredients[0].quantity, peer.trade.level(recipe.skill));
            Some((product, units.min(tradeskills::STACK)))
        }
        None if tradeskills::is_study(recipe.id) => None,
        None => return Err(invalid("That recipe makes nothing.")),
    };
    let name = made.map_or(0, |(definition, _)| definition);
    if let Some((definition, quantity)) = made {
        let mut bag = saved.inventory.clone();
        for command in &commands {
            bag = bag.apply(command).map_err(io::Error::other)?;
        }
        let slot = bag.free_slot().ok_or_else(|| invalid("There is no bag space."))?;
        let output = assets
            .make(definition, store.next_item_id()?, slot, quantity)
            .ok_or_else(|| invalid("That recipe's item is not supported."))?;
        commands.push(Command::GrantFront(output));
    }
    let backup = peer.trade.clone();
    let gained = peer.trade.gain(recipe.skill);
    let mut events = vec![
        crafting::completed(recipe.id, true).map_err(other)?,
        script::chat(&if made.is_some() { format!("You craft item {name:#010x}. You gain {gained} crafting experience.") } else { format!("You study and experiment with different techniques. You gain {gained} crafting experience.") }, 1).map_err(other)?,
    ];
    events.extend(registry(peer, assets)?);
    let result = item_service_transaction_quest(peer, character, &commands, &[], events, store, scene, metadata, false, false);
    finish(peer, character, store, backup, result)
}

fn finish(
    peer: &mut Peer,
    character: u64,
    store: &mut Store,
    backup: tradeskills::Progress,
    result: io::Result<Vec<Delivery>>,
) -> io::Result<Vec<Delivery>> {
    if result.is_err() {
        peer.trade = backup;
        return result;
    }
    save(peer, character, store)?;
    result
}

/// Use a surveyor's map (a bag item): it is spent and a deposit of its material appears
/// beside the player, to be worked by this player only. Policy: the map's coordinates ("near
/// 14.9S, 40.0W", "use the map in nearby locations") are not checked; the deposit appears where
/// the map is used.
pub(super) fn use_map(
    peer: &mut Peer,
    character: u64,
    map: u64,
    assets: &Assets,
    store: &mut Store,
    scene: &mut Scene,
    metadata: &BTreeMap<u32, Option<Metadata>>,
    names: &crate::examination::Names,
) -> io::Result<Vec<Delivery>> {
    let saved = &store.characters[&character];
    let owned = saved
        .inventory
        .items()
        .find(|i| i.id == map && matches!(i.location, Location::Bag(_)))
        .ok_or_else(|| invalid("That map is not in your bag."))?;
    let entity = tradeskills::map_deposit(owned.definition).ok_or_else(|| invalid("That is not a surveyor's map."))?;
    let info = *assets.deposit(entity).ok_or_else(|| invalid("That deposit is not supported."))?;
    let material = tradeskills::MATERIALS.iter().find(|m| m.skill == info.skill).ok_or_else(|| invalid("Unknown deposit."))?;
    let here = scene
        .player_placement(character)
        .ok_or_else(|| invalid("You are not in the world."))?;
    // The map names a place; it is used there. Policy: within `NEAR_CELLS` of it, and a map whose
    // text names none works anywhere.
    let description = names.description_lines(owned.definition).join(" ");
    if let Some(place) = tradeskills::map_coordinates(&description) {
        let [x, y] = tradeskills::land_of(here);
        if ((x - place[0]).powi(2) + (y - place[1]).powi(2)).sqrt() > tradeskills::NEAR_CELLS {
            return Err(invalid(&format!("You are not near the place the map marks. {description}")));
        }
    }
    let mut point = here;
    point.coordinates[0] += 2.5;
    let id = content::NAMESPACE | 0x0007_0000_0000 | SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let object = content::Object::world_object(id, entity, info.visual, point, None, Some(crate::services::Service::Deposit))?;
    let events = vec![script::chat(
        &format!("You use the map and discover a {} deposit nearby. Wield your {} and double-click it to work it.", material.name, tool_name(material)),
        1,
    )
    .map_err(other)?];
    let mut updates = item_service_transaction_quest(
        peer,
        character,
        &[Command::ConsumeDense { item: map }],
        &[],
        events,
        store,
        scene,
        metadata,
        false,
        false,
    )?;
    updates.extend(scene.insert_content_live(object, true)?);
    // Development knob (never set by a player's server): PYREALIUM_DEV_DEPOSIT_CHARGES strikes a deposit
    // holds instead of the entity's own (`410002c8`, 3), so a live test can gather a recipe's
    // 100 units from one deposit.
    let charges = std::env::var("PYREALIUM_DEV_DEPOSIT_CHARGES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(info.charges)
        .max(1);
    peer.deposits.insert(id, Deposit { owner: character, entity, charges, point });
    save(peer, character, store)?;
    Ok(updates)
}

fn tool_name(material: &tradeskills::Material) -> &'static str {
    match material.name {
        "crystal" => "chisel",
        "iron" => "pickaxe",
        "silver" => "mattock",
        "stone" => "rock hammer",
        _ => "hatchet",
    }
}

/// Work a deposit (double-click) with the right tool wielded: a stack of its motes comes into
/// the pack and the mining skill gains experience; the deposit is spent after its charges.
pub(super) fn mine(
    peer: &mut Peer,
    character: u64,
    deposit: u64,
    assets: &Assets,
    store: &mut Store,
    scene: &mut Scene,
    metadata: &BTreeMap<u32, Option<Metadata>>,
) -> io::Result<Vec<Delivery>> {
    let state = *peer.deposits.get(&deposit).ok_or_else(|| {
        invalid("Only the surveyor who discovered this deposit may work it.")
    })?;
    if state.owner != character {
        return Err(invalid("Only the surveyor who discovered this deposit may work it."));
    }
    if !scene.combat_distance(character, deposit).is_some_and(|d| d <= 6.0) {
        return Err(invalid("Move closer to the deposit."));
    }
    let info = assets.deposit(state.entity).ok_or_else(|| invalid("That deposit is not supported."))?;
    let material = tradeskills::MATERIALS.iter().find(|m| m.skill == info.skill).ok_or_else(|| invalid("Unknown deposit."))?;
    let saved = &store.characters[&character];
    let wielded = saved.inventory.items().any(|i| {
        matches!(i.location, Location::Equipped(_)) && tradeskills::material_for_tool(i.definition) == Some(material)
    });
    if !wielded {
        return Err(invalid(&format!("To extract the {}, double-click on the deposit while wielding a {}.", material.name, tool_name(material))));
    }
    // The deposit's weenie description names 10.0 units a strike; AC2Hero (`survey.html`) adds
    // up to 40 for a high Surveying rank (`tradeskills::survey_bonus`).
    let units: u32 = 10 + tradeskills::survey_bonus(peer.trade.level(tradeskills::SURVEYING));
    let slot = free_slot_after(&saved.inventory, &[])?;
    let mote = assets
        .make(info.mote, store.next_item_id()?, slot, units)
        .ok_or_else(|| invalid("That deposit's resource is not supported."))?;
    let backup = peer.trade.clone();
    let gained = peer.trade.gain(info.skill);
    let mut events = vec![script::chat(
        // Retail's string (craft string table 25007ace): "You extract  units of iron ore from ...".
        &format!("You extract {units} units of {} from the {}. You gain {gained} crafting experience.", ore_name(material, info.mote), material.name),
        1,
    )
    .map_err(other)?];
    events.extend(registry(peer, assets)?);
    let updates = item_service_transaction_quest(
        peer,
        character,
        &[Command::GrantFront(mote)],
        &[],
        events,
        store,
        scene,
        metadata,
        false,
        false,
    );
    let mut updates = finish(peer, character, store, backup, updates)?;
    let left = state.charges.saturating_sub(1);
    if left == 0 {
        peer.deposits.remove(&deposit);
        updates.extend(scene.remove_content_live(deposit)?);
    } else if let Some(d) = peer.deposits.get_mut(&deposit) {
        d.charges = left;
    }
    save(peer, character, store)?;
    Ok(updates)
}

/// Corpses already butchered ("has already been butchered": a corpse butchers once).
static BUTCHERED: std::sync::Mutex<std::collections::BTreeSet<u64>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// The creature `corpse` came back: its new body can be butchered.
pub(super) fn respawned(corpse: u64) {
    if let Ok(mut butchered) = BUTCHERED.lock() {
        butchered.remove(&corpse);
    }
}

/// Whether `knife` is a hunting knife in the character's pack (the tool of `WM_Usage` 0x22 whose
/// target is a corpse).
pub(super) fn is_knife(inventory: &Inventory, knife: u64) -> bool {
    inventory
        .items()
        .any(|i| i.id == knife && matches!(i.location, Location::Bag(_)) && crate::butchery::is_knife(i.definition))
}

/// Use a hunting knife on a corpse (`butchery.rs` for the data and the policy): ten units of the
/// trait its butchery profile's family names come into the pack and the Butchering skill gains
/// experience; the corpse butchers once.
pub(super) fn butcher(
    peer: &mut Peer,
    character: u64,
    corpse: u64,
    assets: &Assets,
    store: &mut Store,
    scene: &mut Scene,
    metadata: &BTreeMap<u32, Option<Metadata>>,
) -> io::Result<Vec<Delivery>> {
    let object = scene
        .examination_content(character, corpse)
        .filter(|o| o.stat(0x102) == Some(0) && o.service.is_none())
        .ok_or_else(|| invalid("That cannot be butchered."))?;
    let profile = assets.butchery_profile(object.entity).ok_or_else(|| invalid("That cannot be butchered."))?;
    if !scene.combat_distance(character, corpse).is_some_and(|d| d <= 6.0) {
        return Err(invalid("Move closer to the corpse."));
    }
    if BUTCHERED.lock().map_err(|_| invalid("butchery lock"))?.contains(&corpse) {
        return Err(invalid("The corpse has already been butchered."));
    }
    // AC2Hero (`butcher.html`): "You will not be able to butcher anything less than (or exactly)
    // eight levels below your character level."
    let level = store.characters[&character].progression.level().number;
    if object.stat(0x12e).is_some_and(|creature| crate::butchery::too_far_below(u32::from(level), creature)) {
        return Err(invalid("That creature is too far below your level to butcher."));
    }
    // AC2Hero (`traits.html`): the traits the creature's family gives, ten units each (policy).
    let traits = crate::butchery::traits_of(profile);
    if traits.is_empty() {
        return Err(invalid("The corpse contains nothing of value."));
    }
    let units: u32 = 10;
    let inventory = store.characters[&character].inventory.clone();
    let mut grants = Vec::new();
    let first = store.next_item_id()?;
    for (index, butchered) in traits.iter().enumerate() {
        let slot = free_slot_after(&inventory, &grants)?;
        // The transaction wants the new identities consecutive from the next free one.
        let item = assets
            .make(butchered.definition(), first + index as u64, slot, units)
            .ok_or_else(|| invalid("That corpse's trait is not supported."))?;
        grants.push(Command::GrantFront(item));
    }
    let backup = peer.trade.clone();
    let gained = peer.trade.gain(tradeskills::BUTCHERY);
    let mut events = Vec::new();
    for (index, butchered) in traits.iter().enumerate() {
        // Retail's string (table 25000014): "You butcher  units of  from the ...".
        let mut line = format!("You butcher {units} units of {} from the corpse.", butchered.name());
        if index + 1 == traits.len() {
            line.push_str(&format!(" You gain {gained} crafting experience."));
        }
        events.push(script::chat(&line, 1).map_err(other)?);
    }
    events.extend(registry(peer, assets)?);
    let updates = item_service_transaction_quest(peer, character, &grants, &[], events, store, scene, metadata, false, false);
    let updates = finish(peer, character, store, backup, updates)?;
    BUTCHERED.lock().map_err(|_| invalid("butchery lock"))?.insert(corpse);
    save(peer, character, store)?;
    Ok(updates)
}

/// Whether `item` is an item that Use acts on as a surveyor's map in this bag.
pub(super) fn is_map(inventory: &Inventory, item: u64) -> bool {
    inventory
        .items()
        .any(|i: &Item| i.id == item && tradeskills::map_deposit(i.definition).is_some())
}

/// Whether the player's item may fill slot `ordinal` of a catalog recipe (the validation the
/// craft panel asks before it takes a drop).
pub(super) fn validates(assets: &Assets, inventory: &Inventory, request: &crafting::ValidateRequest) -> bool {
    let Some(recipe) = assets.catalog.get(request.recipe) else {
        return false;
    };
    let Some(slot) = recipe.ingredients.iter().find(|i| i.ordinal == request.ordinal) else {
        return false;
    };
    inventory.items().any(|i| {
        i.id == request.item
            && matches!(i.location, Location::Bag(_))
            && slot.allowed.contains(&i.definition)
            && u32::from(i.quantity) >= slot.quantity
    })
}

/// Load a character's saved tradeskill state at logon: the experience, and the deposits the
/// character prospected and has not worked out (retail: "Only the surveyor who discovered this
/// outcropping may extract its ore", so a deposit belongs to its surveyor and outlasts the logon).
/// A deposit the scene no longer holds (the server restarted) is placed again.
/// Development knob (never set by a player's server): `PYREALIUM_DEV_CRAFT_XP` gives a character with
/// no saved state that much crafting experience in every tradeskill.
pub(super) fn load(
    peer: &mut Peer,
    character: u64,
    store: &mut Store,
    scene: &mut Scene,
    assets: &Assets,
) -> io::Result<Vec<Delivery>> {
    let mut deliveries = Vec::new();
    peer.deposits.clear();
    let saved = store.trade_get(character).map(str::to_owned);
    let (progress, deposits) = saved
        .as_deref()
        .map(|line| line.split_once('|').unwrap_or((line, "")))
        .unzip();
    peer.trade = match progress.and_then(tradeskills::Progress::decode) {
        Some(saved) => saved,
        None => {
            let mut fresh = tradeskills::Progress::default();
            if let Some(xp) = std::env::var("PYREALIUM_DEV_CRAFT_XP").ok().and_then(|v| v.parse::<u64>().ok()) {
                fresh.grant_all(xp);
            }
            fresh
        }
    };
    for entry in deposits.unwrap_or("").split(',').filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.split(':').collect();
        let parsed = (|| -> Option<(u64, Deposit)> {
            let [id, entity, charges, cell, x, y, z, heading] = f[..] else { return None };
            Some((
                id.parse().ok()?,
                Deposit {
                    owner: character,
                    entity: u32::from_str_radix(entity, 16).ok()?,
                    charges: charges.parse().ok()?,
                    point: crate::movement::Placement {
                        cell: u32::from_str_radix(cell, 16).ok()?,
                        coordinates: [x.parse().ok()?, y.parse().ok()?, z.parse().ok()?],
                        heading: heading.parse().ok()?,
                    },
                },
            ))
        })();
        let Some((id, deposit)) = parsed else { continue };
        let Some(info) = assets.deposit(deposit.entity) else { continue };
        SEQUENCE.fetch_max((id & 0xffff_ffff) + 1, Ordering::Relaxed);
        if scene.content_placement(id).is_none() {
            let object = content::Object::world_object(id, deposit.entity, info.visual, deposit.point, None, Some(crate::services::Service::Deposit))?;
            deliveries.extend(scene.insert_content_live(object, true)?);
        }
        peer.deposits.insert(id, deposit);
    }
    save(peer, character, store)?;
    if !peer.deposits.is_empty() {
        deliveries.push(Delivery {
            recipient: character,
            data: script::chat(&format!("You have {} prospected deposit(s) still to work.", peer.deposits.len()), 1)
                .map_err(other)?
                .into(),
        });
    }
    Ok(deliveries)
}

/// Save the state after a change; on a failed transaction the caller restores its backup.
fn save(peer: &Peer, character: u64, store: &mut Store) -> io::Result<()> {
    let deposits: Vec<String> = peer
        .deposits
        .iter()
        .filter(|(_, d)| d.owner == character)
        .map(|(id, d)| {
            let p = d.point;
            format!("{id}:{:x}:{}:{:x}:{}:{}:{}:{}", d.entity, d.charges, p.cell, p.coordinates[0], p.coordinates[1], p.coordinates[2], p.heading)
        })
        .collect();
    store.trade_put(character, format!("{}|{}", peer.trade.encode(), deposits.join(",")))
}

/// `RequestRaiseCraftSkill`: served for the client's request; ranks follow experience as it
/// arrives (`Progress`), so the answer is the skill's rank.
pub(super) fn raise(peer: &mut Peer, skill: u32) -> io::Result<()> {
    let text = match peer.trade.raise(skill) {
        Ok(level) => format!("Your craft skill rises to rank {level}."),
        Err(reason) => reason.to_owned(),
    };
    peer.queue_ordered_batch(vec![script::chat(&text, 1).map_err(other)?])
}

/// The resource as retail's extraction lines name it ("iron ore", "silver ore", "crystal", ...).
fn ore_name(material: &tradeskills::Material, mote: u32) -> &'static str {
    match mote {
        0x4700_3646 => return "granite",
        0x4700_3648 => return "gypsum",
        0x4700_364a => return "kaolin",
        0x4700_364c => return "oak",
        0x4700_364e => return "slate",
        _ => {}
    }
    match material.name {
        "iron" => "iron ore",
        "silver" => "silver ore",
        "wood" => "wood",
        "stone" => "stone",
        _ => "crystal",
    }
}
