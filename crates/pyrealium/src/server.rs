//! Single-owner UDP runtime: every session mutation is serialized through one loop.
use crate::{
    movement::{Motion, Placement},
    script,
    state::{Phase, Session},
    store::Store,
    transport::Transport,
    wire::{self, Header},
    world,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Config {
    pub bind: SocketAddr,
    pub account: String,
    /// SQLite database holding accounts, characters and prepared content.
    pub database: crate::db::Database,
    /// Time between periodic character saves (`Store::save_due`).
    pub save_interval: Duration,
    /// Run directory: the `stop.request` signal file and the admin token.
    pub data: PathBuf,
    pub run_seconds: Option<u64>,
    pub visual_did: Option<u32>,
    pub game_data: Option<PathBuf>,
    pub initialize_ui: bool,
    pub training_kit: bool,
    pub lifestones: bool,
    pub portals: bool,
    pub map_travel: bool,
    pub content: Option<PathBuf>,
    pub encounters: Option<PathBuf>,
    /// Operator world-builder placements added on top of `content`.
    pub content_overlay: Option<PathBuf>,
    /// Operator creature stats, behaviour, loot, patrol and spawn-group rules.
    pub world_rules: Option<PathBuf>,
    /// Loopback address for live world editing (needs `content_overlay`).
    pub admin_bind: Option<SocketAddr>,
    /// Prepared content read from the database (`content_prep`); takes the place of `content`.
    pub prepared: Option<crate::content_prep::World>,
}
#[path = "tradeskill_actions.rs"]
mod trade_actions;

struct Peer {
    client_version: String,
    session: Session,
    transport: Transport,
    connect_packet: Vec<u8>,
    login_proof: String,
    id: u16,
    iteration: u16,
    blob: u32,
    selection_pending: Option<Vec<u8>>,
    ordered_stamp: u16,
    motion: Option<Motion>,
    target: Option<u64>,
    combat: bool,
    pending_skill: Option<u32>,
    emotes: crate::emotes::State,
    last_chat_ms: Option<u64>,
    last_inventory_ms: Option<u64>,
    last_examine_ms: Option<u64>,
    last_world_use_ms: Option<u64>,
    /// Tradeskills (`tradeskills.rs`): this connection's craft experience and prospected deposits.
    trade: crate::tradeskills::Progress,
    deposits: BTreeMap<u64, trade_actions::Deposit>,
    last_npc_dialogue_ms: Option<u64>,
    guide_page: usize,
    npc_dialogue: crate::npc_dialogue::Cursor,
    merchant_open_ms: Option<u64>,
    last_shop_refresh_ms: Option<u64>,
    native_shop: Option<crate::merchants::Shop>,
    unhandled_scripts: BTreeSet<u32>,
    scene_outbox: VecDeque<(Arc<[u8]>, bool)>,
    scene_bytes: usize,
    last_time_sync_ms: u64,
}
#[derive(Default)]
struct Counters {
    received: u64,
    rejected: u64,
    logins: u64,
    connected: u64,
    events: u64,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}
fn other(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

impl Peer {
    fn admit_npc_dialogue(&mut self, now: u64) -> io::Result<()> {
        if self.last_npc_dialogue_ms.is_some_and(|last| now.saturating_sub(last) < 1000) {
            return Err(invalid("Wait briefly before speaking again."));
        }
        self.last_npc_dialogue_ms = Some(now);
        Ok(())
    }

    fn clear_scene_outbox(&mut self) {
        self.scene_outbox.clear();
        self.scene_bytes = 0;
    }
    fn queue_scene(&mut self, data: Arc<[u8]>) -> io::Result<()> {
        let ordered = Self::scene_ordered(&data);
        self.queue_scene_as(data, ordered)
    }
    fn scene_ordered(data: &[u8]) -> bool {
        data.get(..4).is_some_and(|opcode| {
            matches!(
                u32::from_le_bytes(opcode.try_into().unwrap()),
                0x10058 | 0x100c2 | 0x10069
                    ..=0x10076 | 0x100d4 | 0x100d5 | 0x10066 | 0x100a8 | 0x10082 | crate::emotes::EVENT
            )
        })
    }
    fn queue_scene_as(&mut self, data: Arc<[u8]>, ordered: bool) -> io::Result<()> {
        if data.len() < 4
            || data.len() > 65536
            || self.scene_outbox.len() >= 1024
            || self.scene_bytes.saturating_add(data.len()) > 1024 * 1024
        {
            return Err(invalid("scene outbox limit"));
        }
        self.scene_bytes += data.len();
        self.scene_outbox.push_back((data, ordered));
        Ok(())
    }
    fn flush_scene(
        &mut self,
        socket: &UdpSocket,
        addr: &SocketAddr,
        now: Instant,
    ) -> io::Result<()> {
        if !matches!(
            self.session.phase,
            Phase::Entering { .. } | Phase::InWorld { .. }
        ) {
            self.clear_scene_outbox();
            return Ok(());
        }
        for _ in 0..32 {
            let Some((data, ordered)) = self.scene_outbox.front().cloned() else {
                break;
            };
            if !self.transport.send_available(data.len()).map_err(other)? {
                break;
            }
            let packets = self.send_world_event(&data, ordered, now)?;
            self.scene_outbox.pop_front();
            self.scene_bytes -= data.len();
            for packet in packets {
                socket.send_to(&packet, addr)?;
            }
        }
        Ok(())
    }
    /// Admit a dependent group completely before changing the outgoing queue.
    /// Fragment-window pressure is handled by flush_scene, without cloning
    /// transport/ISAAC history or requiring every event to fit in one send window.
    fn validate_scene_batch(&self, events: &[Vec<u8>]) -> io::Result<usize> {
        let mut total = self.scene_bytes;
        if self.scene_outbox.len().saturating_add(events.len()) > 1024 {
            return Err(invalid("scene batch event limit"));
        }
        for event in events {
            if !(4..=65536).contains(&event.len()) {
                return Err(invalid("scene batch message limit"));
            }
            total = total
                .checked_add(event.len())
                .filter(|size| *size <= 1024 * 1024)
                .ok_or_else(|| invalid("scene batch byte limit"))?;
        }
        Ok(total)
    }
    fn queue_event_batch(&mut self, events: Vec<Vec<u8>>) -> io::Result<()> {
        let total = self.validate_scene_batch(&events)?;
        self.scene_outbox.extend(events.into_iter().map(|data| {
            let ordered = Self::scene_ordered(&data);
            (Arc::from(data), ordered)
        }));
        self.scene_bytes = total;
        Ok(())
    }
    fn queue_ordered_batch(&mut self, events: Vec<Vec<u8>>) -> io::Result<()> {
        let count = events.len();
        self.queue_event_batch(events)?;
        for (_, ordered) in self.scene_outbox.iter_mut().rev().take(count) {
            *ordered = true;
        }
        Ok(())
    }
    fn send_world_event(
        &mut self,
        bytes: &[u8],
        ordered: bool,
        now: Instant,
    ) -> io::Result<Vec<Vec<u8>>> {
        let blob = self
            .blob
            .checked_add(1)
            .ok_or_else(|| invalid("blob sequence exhausted"))?;
        let ordered_stamp = if ordered {
            self.ordered_stamp.wrapping_add(1)
        } else {
            self.ordered_stamp
        };
        let (namespace, stamp) = if ordered {
            (0x0100_0000u32, u32::from(ordered_stamp))
        } else {
            (0x0300_0000u32, blob & 0xffff)
        };
        let id = (u64::from(namespace | stamp) << 32) | u64::from(blob);
        let packets = self.transport.send(id, 1, bytes, now).map_err(other)?;
        self.blob = blob;
        self.ordered_stamp = ordered_stamp;
        Ok(packets)
    }
    fn send_event(&mut self, bytes: &[u8], now: Instant) -> io::Result<Vec<Vec<u8>>> {
        if matches!(
            self.session.phase,
            Phase::Entering { .. } | Phase::InWorld { .. }
        ) {
            self.send_world_event(bytes, true, now)
        } else {
            self.send_to_queue(1, bytes, now)
        }
    }
    fn send_to_queue(
        &mut self,
        queue: u16,
        bytes: &[u8],
        now: Instant,
    ) -> io::Result<Vec<Vec<u8>>> {
        let blob = self
            .blob
            .checked_add(1)
            .ok_or_else(|| invalid("blob sequence exhausted"))?;
        // 00494890 drops ordinary world events while NOT_IN_WORLD. Pre-world
        // messages need bit61 and unordered type3, with no player CBookKeep.
        let preworld = !matches!(
            self.session.phase,
            Phase::InWorld { .. } | Phase::Entering { .. }
        );
        let namespace = if preworld { 0x2300_0000 } else { 0x0100_0000 };
        // Native client's CEventQueue is receive queue1; queue4 is its logon
        // chat queue and discards CharacterSet (0071fb10).
        // The upper ordering stamp is only 16 bits; the blob identifier is 32.
        // Long world sessions must still be able to return to selection.
        let id = (u64::from(namespace | (blob & 0xffff)) << 32) | u64::from(blob);
        let packets = self.transport.send(id, queue, bytes, now).map_err(other)?;
        self.blob = blob;
        Ok(packets)
    }
    fn control(&self, flag: u32, body: &[u8]) -> io::Result<Vec<u8>> {
        wire::encode_control_packet(
            Header {
                flags: flag,
                recipient: self.id,
                iteration: self.iteration,
                ..Default::default()
            },
            body,
            None,
        )
        .map_err(other)
    }
}

struct ForcedMove {
    motion: Motion,
    update: crate::movement::ClientPosition,
    wire: Vec<u8>,
}
fn prepare_forced_move(
    peer: &Peer,
    character: u64,
    destination: Placement,
    scene: &mut crate::scene::Scene,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
) -> io::Result<ForcedMove> {
    if scene.player_placement(character).is_none()
        || !bootstrap
            .as_mut()
            .is_some_and(|profile| profile.contains_cell(destination.cell))
    {
        return Err(invalid("Saved destination is not available in this world."));
    }
    scene.prepare_cell(destination.cell)?;
    let (motion, update) = peer
        .motion
        .as_ref()
        .ok_or_else(|| invalid("Missing player motion state."))?
        .forced(destination)
        .map_err(other)?;
    let wire = crate::movement::server_position(
        character,
        motion.instance,
        motion.position_stamp,
        &update,
    )
    .map_err(other)?;
    Ok(ForcedMove {
        motion,
        update,
        wire,
    })
}
fn use_portal(
    peer: &mut Peer,
    character: u64,
    target: u64,
    route: crate::portals::Route,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    encounters: &mut Option<crate::encounters::Encounters>,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
    now_ms: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if peer.combat || !saved.progression.alive() {
        return Err(invalid("Use a portal while alive and out of combat."));
    }
    if saved.progression.level().number < route.level {
        return Err(io::Error::other(format!(
            "This portal requires level {}.",
            route.level
        )));
    }
    if peer
        .last_world_use_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < 5000)
    {
        return Err(invalid("Wait a few seconds before using a portal."));
    }
    let player = scene
        .player_placement(character)
        .ok_or_else(|| invalid("Player is not ready."))?;
    let portal = scene
        .examination_content(character, target)
        .ok_or_else(|| invalid("Portal is not visible."))?;
    let distance: f64 = player
        .coordinates
        .into_iter()
        .zip(portal.placement.coordinates)
        .map(|(a, b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    if player.cell >> 16 != portal.placement.cell >> 16 || distance > 16.0 {
        return Err(invalid("Move within four meters of the portal."));
    }
    let prepared = prepare_forced_move(peer, character, route.destination, scene, bootstrap)?;
    let mut events = vec![
        crate::ui::portal_used(character, target, route.destination, distance.sqrt() as f32)
            .map_err(other)?,
        prepared.wire,
        script::private_call(script::COMBAT_STATE, &script::integer(0)).map_err(other)?,
    ];
    if saved.quests.journey_phase() == 2 && crate::quests::is_training_portal(target) {
        events.extend(saved.quests.apply(&crate::quests::Command::Travelled).map_err(other)?.updates_since(&saved.quests).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    store.relocate_training_portal(&peer.session.account, character, route.destination, target)?;
    if let Some(runtime) = encounters {
        runtime.stop(character);
    }
    let updates = scene
        .update(character, prepared.motion.position_stamp, &prepared.update)
        .map_err(other)?;
    peer.motion = Some(prepared.motion);
    peer.target = None;
    peer.last_world_use_ms = Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
fn map_travel_character(
    peer: &mut Peer, character: u64, destination: Placement, store: &mut Store,
    scene: &mut crate::scene::Scene, encounters: &mut Option<crate::encounters::Encounters>,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>, now_ms: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid("Leave combat before map travel; the character must be alive."));
    }
    if peer.last_world_use_ms.is_some_and(|last| now_ms.saturating_sub(last) < 5000) {
        return Err(invalid("Wait a few seconds before traveling again."));
    }
    let prepared = prepare_forced_move(peer, character, destination, scene, bootstrap)?;
    let events = vec![prepared.wire, script::chat("Map travel accepted.", 1).map_err(other)?];
    peer.validate_scene_batch(&events)?;
    store.relocate(&peer.session.account, character, destination)?;
    if let Some(runtime) = encounters { runtime.stop(character); }
    let updates = scene.update(character, prepared.motion.position_stamp, &prepared.update).map_err(other)?;
    peer.motion = Some(prepared.motion);
    peer.target = None;
    peer.last_world_use_ms = Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
fn recall_character(
    peer: &mut Peer,
    character: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    encounters: &mut Option<crate::encounters::Encounters>,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
    now_ms: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if peer.combat || !saved.progression.alive() {
        return Err(invalid("Recall while alive and out of combat."));
    }
    if peer
        .last_world_use_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < 5000)
    {
        return Err(invalid("Wait a few seconds before recalling."));
    }
    let destination = saved
        .binding
        .ok_or_else(|| invalid("Bind to a lifestone before recalling."))?;
    let prepared = prepare_forced_move(peer, character, destination, scene, bootstrap)?;
    let mut events = vec![prepared.wire];
    if saved.skills.get(crate::skills::LIFESTONE_RECALL).is_some() {
        events
            .push(crate::ui::skill_update(crate::skills::LIFESTONE_RECALL, &saved.skills, saved.skill_used.get(&crate::skills::LIFESTONE_RECALL).copied()).map_err(other)?);
    }
    events.push(script::private_call(script::COMBAT_STATE, &script::integer(0)).map_err(other)?);
    peer.validate_scene_batch(&events)?;
    store.return_to_binding(&peer.session.account, character, false)?;
    if let Some(runtime) = encounters {
        runtime.stop(character);
    }
    let updates = scene
        .update(character, prepared.motion.position_stamp, &prepared.update)
        .map_err(other)?;
    peer.motion = Some(prepared.motion);
    peer.target = None;
    peer.last_world_use_ms = Some(now_ms);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}
fn resurrect_character(
    peer: &mut Peer,
    character: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    encounters: &mut Option<crate::encounters::Encounters>,
    lifestones: bool,
    bootstrap: &mut Option<crate::bootstrap::Bootstrap>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = store
        .characters
        .get(&character)
        .filter(|c| !c.deleted && c.account == peer.session.account)
        .ok_or_else(|| invalid("Character unavailable."))?;
    if saved.progression.alive() {
        return Err(invalid("You are already alive."));
    }
    let next = saved
        .progression
        .apply(&crate::progression::Command::Resurrect)
        .map_err(other)?;
    let forced = if lifestones {
        saved
            .binding
            .map(|destination| prepare_forced_move(peer, character, destination, scene, bootstrap))
            .transpose()?
    } else {
        None
    };
    let mut owner_events = next.private_updates();
    if let Some(prepared) = &forced {
        owner_events.push(prepared.wire.clone());
    }
    owner_events.push(crate::ui::lock_player(false).map_err(other)?);
    owner_events
        .push(script::private_call(script::COMBAT_STATE, &script::integer(0)).map_err(other)?);
    if let Some(request_id) = peer.pending_skill {
        owner_events.push(script::skill_finished(request_id).map_err(other)?);
    }
    peer.validate_scene_batch(&owner_events)?;
    if forced.is_some() {
        store.return_to_binding(&peer.session.account, character, true)?;
    } else {
        store.progression_command(
            &peer.session.account,
            character,
            crate::progression::Command::Resurrect,
        )?;
    }
    if let Some(runtime) = encounters {
        runtime.stop(character);
    }
    peer.combat = false;
    peer.target = None;
    peer.pending_skill = None;
    let state = &store.characters[&character].progression;
    let mut updates = scene.player_stats(character, state);
    updates.extend(player_body_alive(peer, scene, character, store.characters[&character].instance, true));
    if let Some(prepared) = forced {
        updates.extend(
            scene
                .update(character, prepared.motion.position_stamp, &prepared.update)
                .map_err(other)?,
        );
        peer.motion = Some(prepared.motion);
    }
    peer.queue_ordered_batch(owner_events)?;
    Ok(updates)
}

fn inventory_spawn(
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

fn examine_target(
    character: u64,
    target: u64,
    store: &Store,
    scene: &crate::scene::Scene,
    names: &crate::examination::Names,
    loot: &crate::loot::Loot,
    now: u64,
    effect_lines: &[String],
) -> io::Result<Vec<u8>> {
    use crate::{
        examination::{self, Name},
        inventory::Location,
    };
    if let Some(player) = store
        .characters
        .get(&target)
        .filter(|player| !player.deleted && scene.can_observe(character, player.id))
    {
        let state = &player.progression;
        let mut lines = vec![
            format!("Level {}", state.level().number),
            format!("Health: {} / {}", state.health, state.level().health),
        ];
        if let Some(creation)=&player.creation {
            lines.insert(0,match creation.race {1=>"Human",2=>"Lugian",4=>"Tumerok",8=>"Drudge",16=>"Empyrean",_=>"Player"}.into());
        }
        if !state.alive() {
            lines.push("Dead".to_owned());
        }
        if character == target {
            lines.extend([
                format!("Vigor: {} / {}", state.vigor, state.level().vigor),
                format!("Experience: {}", state.xp),
                format!("Available experience: {}", state.available_xp),
                format!("Gold: {}", state.gold),
            ]);
            if player.binding.is_some() {
                lines.push("Bound to a lifestone.".to_owned());
            }
            lines.push(format!("Available skill credits: {}",player.skills.available_credits(state)));
        }
        return examination::response(target, Name::Literal(player.name.clone()), &lines)
            .map_err(other);
    }
    if let Some(item) = store.characters[&character]
        .inventory
        .items()
        .find(|item| item.id == target)
    {
        let location = match item.location {
            Location::Equipped(0x40) => "Equipped in main hand.",
            Location::Equipped(0x80) => "Equipped in off hand.",
            Location::Equipped(_) => "Equipped.",
            Location::Bag(_) => "In your inventory.",
        };
        let mut lines=names.item_lines(item,location);
        let profession=&store.characters[&character].professions;
        if let Some(job)=profession.1.jobs.get(&item.id){lines.push(job.guidance());}
        if let Some(tool)=crate::harvesting::tool(item.definition){lines.push(format!("Tool durability: {} / {}. Select a resource or corpse and double-click the tool.",tool.durability.saturating_sub(crate::harvesting::wear(profession,item)),tool.durability));}
        if let Some(skills)=crate::professions::creation_tool(item.definition){
            let level=item.roll.map_or(1,|r|r.level);let used=item.roll.map_or(0,|r|r.wear);
            lines.push(format!("Creation tool: +{level} effective rank while equipped. Requires matching crafting rank {}. Durability: {} / 20.",u32::from(level)*10,20u16.saturating_sub(used)));
            lines.push(skills.iter().filter_map(|s|crate::professions::skill(*s).map(|s|s.name.as_str())).collect::<Vec<_>>().join(", "));
        }
        return examination::response(
            target,
            names.get(item.definition, "Item"),
            &lines,
        )
        .map_err(other);
    }
    if let Some(item)=loot.inspect_item(target,character,now,|container|scene.can_observe(character,container)&&scene.combat_distance(character,container).is_some_and(|d|d<=4.)) {
        return examination::response(target,names.get(item.definition,"Item"),&names.item_lines(item,"In the loot container.")).map_err(other);
    }
    if let Some(drop)=store.drops.get(&target).filter(|_|scene.can_observe(character,target)) {
        return examination::response(target,names.get(drop.item.definition,"Item"),&names.item_lines(&drop.item,"On the ground. Double-click nearby to pick up.")).map_err(other);
    }
    if let Some(definition) = scene.visible_hand_definition(character, target) {
        return examination::response(target, names.get(definition, "Item"), &[]).map_err(other);
    }
    if let Some(object) = scene.examination_content(character, target) {
        let mut lines = Vec::new();
        let dead=object.stat(0x102)==Some(0) && object.stat(0x107).is_some();
        if dead {lines.push("Defeated creature. Double-click to examine its remaining loot.".into());}
        else if let Some(level)=object.stat(0x12e) {lines.push(format!("Level {level}"));}
        if let Some(service)=object.service {
            use crate::services::Service;
            lines.push(match service {
                Service::Vendor=>"Vendor. Double-click nearby to trade.",
                Service::Door{locked:true}=>"Locked door.",Service::Door{locked:false}=>"Door. Double-click nearby to open or close.",
                Service::Chest{locked:true}=>"Locked chest.",Service::Chest{locked:false}=>"Chest. Double-click nearby to open.",
                Service::Sign=>"Sign. Double-click to read.",
                Service::Deposit=>"Resource deposit. Wield the matching tool and double-click it.",
            }.into());
        }
        if let Some((health, maximum)) = object.stat(0x102).zip(object.stat(0x107)) {
            lines.push(format!("Health: {health} / {maximum}"));
        }
        if let Some((vigor, maximum)) = object.stat(0x103).zip(object.stat(0x108)) {
            lines.push(format!("Vigor: {vigor} / {maximum}"));
        }
        // Timed effects on the creature (a burn, a lowered armor), with the time left. Pyrealium's own
        // text: the retail server's profile for another creature's effects is not known.
        lines.extend(effect_lines.iter().take(3).cloned());
        lines.extend(names.description_lines(object.entity));
        if object.entity == TRAINING_MENTOR_ENTITY {
            lines.push("Use to accept training lessons, check progress or deliver supplies.".into());
            lines.push("Nearby conversation topics: @mentor help, gear, combat, healing, supplies, crafting, travel.".into());
        } else if object.entity == crate::merchants::MERCHANT_ENTITY {
            lines.push("Use to trade. Mucor Mushrooms and Tallow bundles each cost 2 gold.".into());
        }
        if let Some(name) = crate::training_dialogue::portal_name(target) {
            lines.push("Use while alive and out of combat, within four metres.".into());
            return examination::response(target, Name::Literal(name.into()), &lines).map_err(other);
        }
        let mut name=object.name.clone().map(Name::Literal).unwrap_or_else(||names.get(object.entity, "Object"));
        if dead {name=Name::Corpse(Box::new(name));}
        return examination::response(target, name, &lines)
            .map_err(other);
    }
    Err(invalid("That object is not available to examine."))
}

fn selection_list(
    store: &Store,
    account: &str,
    profile: Option<&crate::bootstrap::Bootstrap>,
    visual: Option<u32>,
) -> io::Result<Vec<u8>> {
    selection_list_excluding(store, account, profile, visual, None)
}
fn selection_list_excluding(
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

fn deliver_scene(
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

fn training_combat_visual(
    peer: &mut Peer,
    character: u64,
    store: &Store,
    scene: &mut crate::scene::Scene,
    fighting: bool,
    swing: bool,
    skill: bool,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    let mode = if fighting { crate::starter_gear::combat_mode(&saved.inventory) }
        else { crate::combat_visual::PEACE };
    let (mut next, stamp) = peer.emotes.next_visual(saved.instance).map_err(other)?;
    let mut last_stamp = stamp;
    let mut events = vec![
        crate::combat_visual::mode_id(character, saved.instance, stamp, mode).map_err(other)?,
    ];
    if swing {
        let (after, stamp) = next.next_visual(saved.instance).map_err(other)?;
        next = after;
        last_stamp = stamp;
        let attack=if skill && mode==crate::combat_visual::MAGIC {
            crate::combat_visual::attack_behavior(character,saved.instance,stamp,if saved.creation.as_ref().is_some_and(|r|r.race==4){0x410000bc}else{0x410000b9})
        } else {crate::combat_visual::weapon_attack(character,saved.instance,stamp,mode)};
        events.push(attack.map_err(other)?);
        if mode==crate::combat_visual::MAGIC {
            if let Some((fx,_))=saved.creation.as_ref().and_then(|r|crate::combat_visual::magic_effects(r.race)) {
                let (after,stamp)=next.next_visual(saved.instance).map_err(other)?;next=after;last_stamp=stamp;
                events.push(crate::combat_visual::effect(character,saved.instance,stamp,fx).map_err(other)?);
            }
        }
    }
    peer.validate_scene_batch(&events)?;
    let mut deliveries = Vec::new();
    for event in events {
        deliveries.extend(scene.broadcast(character, event).map_err(other)?);
    }
    scene
        .save_combat_mode(character, mode, last_stamp)
        .map_err(other)?;
    peer.emotes = next;
    Ok(deliveries)
}

/// A player's body lies down dead (`death` behaviour, actions locked) or stands up in the peace
/// mode again, for the player and the observers; the mode is kept for late observers.
fn player_body_alive(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    alive: bool,
) -> Vec<crate::scene::Delivery> {
    let Ok((mut next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let mut last = stamp;
    let mut wires = Vec::new();
    if alive {
        let Ok((after, second)) = next.next_visual(instance) else {
            return Vec::new();
        };
        next = after;
        last = second;
        let (Ok(a), Ok(b)) = (
            crate::combat_visual::player_alive(owner, instance, stamp),
            crate::combat_visual::stop_death(owner, instance, second),
        ) else {
            return Vec::new();
        };
        wires.extend([a, b]);
    } else {
        let Ok(a) = crate::combat_visual::player_death(owner, instance, stamp) else {
            return Vec::new();
        };
        wires.push(a);
    }
    let mut deliveries = Vec::new();
    for wire in wires {
        let Ok(more) = scene.broadcast(owner, wire) else {
            return Vec::new();
        };
        deliveries.extend(more);
    }
    let mode = if alive { crate::combat_visual::PEACE } else { crate::combat_visual::DEAD_MODE };
    if scene.save_combat_mode(owner, mode, last).is_err() {
        return Vec::new();
    }
    peer.emotes = next;
    deliveries
}

fn publish_health_story(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    attacker: u64,
    health_change: i32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) =
        crate::combat_visual::damage_story(owner, instance, stamp, attacker, health_change)
    else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) => {
            if scene.save_visual_stamp(owner, stamp).is_err() {
                return Vec::new();
            }
            peer.emotes = next;
            deliveries
        }
        Err(_) => Vec::new(),
    }
}

fn use_recovered_elixir(peer:&mut Peer,character:u64,item:u64,store:&mut Store,scene:&mut crate::scene::Scene)->io::Result<()> {
    if !scene.can_observe(character,character)||peer.combat{return Err(invalid("Leave combat before drinking a quest elixir."));}
    let c=&store.characters[&character];let account=c.account.clone();
    let owned=c.inventory.items().find(|i|i.id==item).ok_or_else(||invalid("Elixir not owned"))?;
    let q=crate::recovered_quests::use_profile(owned.definition).ok_or_else(||invalid("Unsupported quest elixir"))?;
    let instance=scene.inventory_instance(character,item).map_err(other)?;
    let mut quests=c.quests.clone();
    quests.recovered=quests.recovered.accept(q.id,c.progression.level().number,crate::recovered_quests::now())?;
    let mut events=vec![crate::item_services::removed(item).map_err(other)?,world::destroy_inventory_item(item,instance)];
    events.extend(quests.updates_since(&c.quests).map_err(other)?);
    events.push(script::chat(&format!("{} accepted. Defeat {} targets before the time limit.",q.title,q.goal),1).map_err(other)?);
    peer.validate_scene_batch(&events)?;
    store.recovered_elixir(&account,character,item)?;
    scene.inventory_removed(character,item).map_err(other)?;peer.queue_ordered_batch(events)?;Ok(())
}

fn arwic_exchange(peer:&mut Peer,character:u64,action:crate::arwic_mines::Action,events:Vec<Vec<u8>>,store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>)->io::Result<Vec<crate::scene::Delivery>> {
    let saved=&store.characters[&character];let time=crate::recovered_quests::now();
    let plan=crate::arwic_mines::plan(&saved.quests,&saved.inventory,action,store.next_item_id()?,time)?;
    item_service_transaction_profile(peer,character,&plan.items,&plan.rewards,events,store,scene,metadata,false,false,None,None,Some((action,time)))
}

fn use_exploration_npc(peer:&mut Peer,character:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid("Speak to the quest NPC while alive and out of combat."));
    }
    let completion=npc_use_completion(character,target,scene)?;
    let entity=scene.examination_content(character,target).ok_or_else(||invalid("Quest object is not visible."))?.entity;
    peer.admit_npc_dialogue(now)?;
    let saved=&store.characters[&character];let account=saved.account.clone();
    let time=crate::recovered_quests::now();
    let conversation=saved.quests.exploration.converse(entity,saved.progression.level().number,time)?;
    let mut quests=saved.quests.clone();quests.exploration=conversation.book;
    let state=if conversation.xp>0 {saved.progression.apply(&crate::progression::Command::AwardXp(conversation.xp)).map_err(other)?}
        else {saved.progression.clone()};
    let speaker=crate::exploration_quests::speaker(entity);
    let mut events=vec![completion,crate::interaction::speech(target,speaker,&conversation.text).map_err(other)?,
        crate::interaction::dialogue(speaker,&conversation.text).map_err(other)?];
    events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state)!=saved.skills.available_credits(&saved.progression) {
        events.push(crate::ui::skills_state(&saved.skills,&state,&saved.skill_used).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    store.exploration_conversation(&account,character,entity,time)?;
    let updates=scene.player_stats(character,&state);peer.queue_ordered_batch(events)?;Ok(updates)
}

fn use_hunting_npc(peer:&mut Peer,character:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid("Speak to the quest NPC while alive and out of combat."));
    }
    let completion=npc_use_completion(character,target,scene)?;
    let entity=scene.examination_content(character,target).ok_or_else(||invalid("Quest object is not visible."))?.entity;
    peer.admit_npc_dialogue(now)?;
    let saved=&store.characters[&character];let account=saved.account.clone();
    let time=crate::recovered_quests::now();
    let conversation=saved.quests.hunting.converse(entity,&saved.progression,time)?;
    let mut quests=saved.quests.clone();quests.hunting=conversation.book;
    let state=if conversation.xp>0 {saved.progression.apply(&crate::progression::Command::AwardXp(conversation.xp)).map_err(other)?}
        else {saved.progression.clone()};
    let speaker=crate::hunting_quests::speaker(entity);
    let mut events=vec![completion,crate::interaction::speech(target,speaker,&conversation.text).map_err(other)?,
        crate::interaction::dialogue(speaker,&conversation.text).map_err(other)?];
    events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state)!=saved.skills.available_credits(&saved.progression) {
        events.push(crate::ui::skills_state(&saved.skills,&state,&saved.skill_used).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    store.hunting_conversation(&account,character,entity,time)?;
    let updates=scene.player_stats(character,&state);peer.queue_ordered_batch(events)?;Ok(updates)
}

fn use_recovered_delivery(peer:&mut Peer,character:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>)->io::Result<Vec<crate::scene::Delivery>> {
    if peer.combat||!store.characters[&character].progression.alive(){return Err(invalid("Speak to the quest NPC while alive and out of combat."));}
    let npc=scene.examination_content(character,target).ok_or_else(||invalid("Quest NPC is not visible."))?.entity;
    let completion=npc_use_completion(character,target,scene)?;
    let saved=&store.characters[&character];let account=saved.account.clone();let time=crate::recovered_quests::now();
    let first=store.next_item_id()?;
    let (items,changes,book)=saved.quests.recovered.delivery(&saved.inventory,npc,saved.progression.level().number,time,first)?;
    let (bag,state)=if items.is_empty()&&changes.is_empty(){(saved.inventory.clone(),saved.progression.clone())}else{
        crate::player_transaction::candidate(&saved.inventory,&saved.progression,&items,&changes,Some(first))?};
    let mut quests=saved.quests.clone();quests.recovered=book;
    let mut events=vec![completion];let mut granted=Vec::new();let mut removed=Vec::new();
    for command in &items {match command {
        crate::inventory::Command::Grant(item)=>{
            let info=metadata.get(&item.definition).and_then(|i|*i).ok_or_else(||invalid("Quest bag metadata unavailable"))?;
            let instance=scene.admit_inventory_grant(character,item.id).map_err(other)?;
            events.push(world::create_inventory_item(item,character,instance).map_err(other)?);
            events.push(crate::item_services::acquired(character,item,info).map_err(other)?);granted.push((item.id,instance));
        },
        crate::inventory::Command::ConsumeDense{item}=>{
            let instance=scene.inventory_instance(character,*item).map_err(other)?;
            events.push(crate::item_services::removed(*item).map_err(other)?);events.push(world::destroy_inventory_item(*item,instance));removed.push(*item);
        },_=>return Err(invalid("Unsupported delivery inventory transition"))
    }}
    events.extend(state.private_changes(&saved.progression));
    events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    events.push(script::chat(if npc==crate::recovered_quests::KANTEN {"Gambling Debt: deliver Kanten's Bag to Orven in Arwic (44.9N, 7.1W) within two hours."}
        else {"Gambling Debt complete. You receive 1,000 experience."},1).map_err(other)?);
    if saved.skills.available_credits(&state)!=saved.skills.available_credits(&saved.progression){events.push(crate::ui::skills_state(&saved.skills,&state,&saved.skill_used).map_err(other)?);}
    peer.validate_scene_batch(&events)?;
    store.recovered_delivery(&account,character,npc,time)?;
    debug_assert_eq!(store.characters[&character].inventory,bag);
    for item in removed {scene.inventory_removed(character,item).map_err(other)?;}
    for (item,instance) in granted {scene.inventory_granted(character,item,instance).map_err(other)?;}
    let updates=scene.player_stats(character,&state);peer.queue_ordered_batch(events)?;Ok(updates)
}

fn skill_visual(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    behavior: u32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) = crate::combat_visual::attack_behavior(owner, instance, stamp, behavior) else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) if scene.save_visual_stamp(owner, stamp).is_ok() => {
            peer.emotes = next;
            deliveries
        }
        _ => Vec::new(),
    }
}


/// [`skill_visual`] for a skill that fires a missile: the cast story names the skill and its
/// fire-missile hook, so every client in view makes and flies the missile itself.
#[allow(clippy::too_many_arguments)]
fn skill_missile_visual(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    behavior: u32,
    skill: u32,
    target: u64,
    hook: u32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) = crate::combat_visual::skill_missile_story(owner, instance, stamp, behavior, skill, target, hook) else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) if scene.save_visual_stamp(owner, stamp).is_ok() => {
            peer.emotes = next;
            deliveries
        }
        _ => Vec::new(),
    }
}

fn player_fx(
    peer: &mut Peer,
    scene: &mut crate::scene::Scene,
    owner: u64,
    instance: u16,
    fx: u32,
) -> Vec<crate::scene::Delivery> {
    let Ok((next, stamp)) = peer.emotes.next_visual(instance) else {
        return Vec::new();
    };
    let Ok(wire) = crate::combat_visual::effect(owner, instance, stamp, fx) else {
        return Vec::new();
    };
    match scene.broadcast(owner, wire) {
        Ok(deliveries) if scene.save_visual_stamp(owner, stamp).is_ok() => {
            peer.emotes = next;
            deliveries
        }
        _ => Vec::new(),
    }
}


fn rating_sentence(cast: &crate::abilities::PreparedCast<'_>) -> String {
    let mut lines = Vec::new();
    if let Some((ratings, duration)) = cast.caster_ratings() {
        lines.push(crate::combat::describe(&ratings, duration));
    }
    if let Some((who, ratings, duration)) = cast.other_ratings() {
        if who != cast.player() {
            lines.push(format!("On your target, {}", crate::combat::describe(&ratings, duration)));
        }
    }
    lines.join(" ")
}
fn store_ratings(
    runtime: &mut Option<crate::encounters::Encounters>,
    cast: &crate::abilities::PreparedCast<'_>,
    now: u64,
) -> io::Result<()> {
    if cast.caster_ratings().is_none() && cast.other_ratings().is_none() {
        return Ok(());
    }
    let runtime = runtime.as_mut().ok_or_else(|| invalid("Lasting skill effects require combat rules on this server."))?;
    if let Some((ratings, duration)) = cast.caster_ratings() {
        runtime.add_ratings(cast.player(), cast.skill(), ratings, duration, now)?;
    }
    if let Some((who, ratings, duration)) = cast.other_ratings().filter(|_|!matches!(cast.outcome(),crate::abilities::Outcome::Strike{..})) {
        runtime.add_ratings(who, cast.skill(), ratings, duration, now)?;
    }
    Ok(())
}

/// The timed effects on `target` as examination lines, with the time left.
fn effect_lines(
    encounters: &Option<crate::encounters::Encounters>,
    abilities: &Option<crate::abilities::Runtime>,
    target: u64,
    now: u64,
) -> Vec<String> {
    let (Some(encounters), Some(abilities)) = (encounters, abilities) else {
        return Vec::new();
    };
    encounters
        .effects_on(target, now)
        .into_iter()
        .map(|(skill, did, left)| {
            format!("{}: {} s left", abilities.effect_label(skill, did), left.div_ceil(1000))
        })
        .collect()
}

/// The timed effects a cast put on players join the effect lists (`WM_Effect` 0x17 follows with
/// the next tick, `Event::EffectAdded`).
fn store_effects(
    runtime: &mut Option<crate::encounters::Encounters>,
    cast: &crate::abilities::PreparedCast<'_>,
    now: u64,
) {
    if let Some(runtime) = runtime.as_mut() {
        for effect in cast.applied() {
            runtime.note_effect(effect.entity, cast.skill(), effect.did, effect.duration_ms, now, effect.power, effect.pulsed);
        }
    }
}

fn use_training_potion(
    peer: &mut Peer,
    character: u64,
    item: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    now: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if !scene.can_observe(character, character) || !saved.progression.alive() {
        return Err(invalid("Potion use requires a ready living character."));
    }
    if peer
        .last_world_use_ms
        .is_some_and(|last| now.saturating_sub(last) < crate::item_services::POTION_COOLDOWN_MS)
    {
        return Err(invalid("Wait five seconds before using another potion."));
    }
    let owned = saved
        .inventory
        .items()
        .find(|owned| owned.id == item)
        .filter(|owned| {
            (owned.definition == crate::item_services::POTION_DEFINITION
                && owned.visual == crate::item_services::POTION_VISUAL
                || owned.definition == crate::crafting::VIGOR_OUTPUT && owned.visual == 0x1f000e40)
                && owned.quantity == 1
                && owned.stack_limit == 1
                && owned.equipment_mask == 0
        })
        .ok_or_else(|| {
            invalid("This training potion is not owned or has unsupported properties.")
        })?;
    let vigor = owned.definition == crate::crafting::VIGOR_OUTPUT;
    if !matches!(owned.location, crate::inventory::Location::Bag(_))
        || if vigor {
            saved.progression.vigor == saved.progression.level().vigor
        } else {
            saved.progression.health == saved.progression.level().health
        }
    {
        return Err(invalid(
            "The potion must be in your bag and its vital must be below the maximum.",
        ));
    }
    let instance = scene.inventory_instance(character, item).map_err(other)?;
    let command = crate::inventory::Command::ConsumeDense { item };
    let changes = [if vigor {
        crate::progression::Command::RecoverVigor(25)
    } else {
        crate::progression::Command::Heal(crate::item_services::POTION_HEALTH)
    }];
    let next = saved
        .progression
        .apply(&changes[0])
        .map_err(io::Error::other)?;
    let physics = saved.instance;
    let recovered_health = if vigor {
        0
    } else {
        next.health.saturating_sub(saved.progression.health)
    };
    let mut events = vec![
        crate::item_services::removed(item).map_err(other)?,
        world::destroy_inventory_item(item, instance),
    ];
    events.extend(next.private_changes(&saved.progression));
    events.push(
        script::chat(
            &format!(
                "You recover {} {}.",
                if vigor {
                    next.vigor - saved.progression.vigor
                } else {
                    next.health - saved.progression.health
                },
                if vigor { "vigor" } else { "health" }
            ),
            1,
        )
        .map_err(other)?,
    );
    peer.validate_scene_batch(&events)?;
    store.player_transaction(&peer.session.account, character, &[command], &changes)?;
    scene.inventory_removed(character, item).map_err(other)?;
    peer.last_world_use_ms = Some(now);
    let mut updates = scene.player_stats(character, &next);
    if let Ok(recovered) = i32::try_from(recovered_health) {
        if recovered > 0 {
            updates.extend(publish_health_story(
                peer, scene, character, physics, character, recovered,
            ));
        }
    }
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}

fn quest_action(
    peer: &mut Peer,
    character: u64,
    action: &str,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    speaker: Option<u64>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    if !scene.can_observe(character, character) {
        return Err(invalid("Player is not ready."));
    }
    let saved = &store.characters[&character];
    if action.trim().eq_ignore_ascii_case("shelter") {
        let quest=crate::quest_emotes::installed()?.quest("shelter").ok_or_else(||invalid("no shelter quest"))?.journal_id;
        let text=crate::quest_emotes::installed()?.stage(quest,saved.quests.stage(quest)).map_or("",|(_,t)|t);
        peer.queue_ordered_batch(vec![script::chat(text,1).map_err(other)?])?;
        return Ok(Vec::new());
    }
    let command = match action.trim().to_ascii_lowercase().as_str() {
        "" | "status" => None,
        "start" => Some(crate::quests::Command::Accept),
        "supplies" => Some(crate::quests::Command::AcceptSupplies),
        "potion" => Some(crate::quests::Command::AcceptPotion),
        "potion claim" => Some(crate::quests::Command::ClaimPotion),
        "journey" => Some(crate::quests::Command::AcceptJourney),
        "journey claim" => Some(crate::quests::Command::ClaimJourney),
        "claim" => Some(crate::quests::Command::Claim),
        _ => return Err(invalid("Use @quest for status; start, claim, supplies, potion, potion claim, journey or journey claim for lessons.")),
    };
    if command.is_some() && (!saved.progression.alive() || peer.combat) {
        return Err(invalid(
            "Quest acceptance and rewards require a living player out of combat.",
        ));
    }
    let quests = match command.as_ref() {
        Some(command) => saved.quests.apply(command).map_err(io::Error::other)?,
        None => saved.quests.clone(),
    };
    let mut state = saved.progression.clone();
    if let Some(command) = &command {
        for reward in crate::quests::rewards(command) {
            state = state.apply(&reward).map_err(io::Error::other)?;
        }
    }
    let text = quests.guidance();
    let mut events = match speaker {
        Some(speaker) => {
            let race = saved.creation.as_ref().map_or(1, |request| request.race);
            mentor_interaction_events(character, speaker, scene, &crate::training_dialogue::greeting(race, &quests))?
        }
        None => vec![script::chat(&text, 1).map_err(other)?],
    };
    if quests.get().is_some() {
        events.extend(quests.updates_since(&saved.quests).map_err(other)?);
    }
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state) != saved.skills.available_credits(&saved.progression)
    {
        events
            .push(crate::ui::skills_state(&saved.skills, &state, &saved.skill_used).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    if let Some(command) = command {
        store.quest_command(&peer.session.account, character, command)?;
    }
    let updates = scene.player_stats(character, &state);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}

enum HandChange {
    Toggle,
    Equip { slot: u16, source: u16 },
    Unequip { slot: u16, destination: u16 },
}
fn change_hand_item(
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
fn replace_equipment(
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

fn selection_status(
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

/// Admit the complete native item/vital update before committing a service's
/// ownership and currency changes. Only the server's bounded grant/removal
/// plans are supported here; no client object description enters this path.
fn item_service_transaction(
    peer: &mut Peer,
    character: u64,
    items: &[crate::inventory::Command],
    changes: &[crate::progression::Command],
    completion: Vec<Vec<u8>>,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    item_service_transaction_quest(peer, character, items, changes, completion, store, scene, metadata, false, false)
}
fn item_service_transaction_quest(peer:&mut Peer,character:u64,items:&[crate::inventory::Command],changes:&[crate::progression::Command],completion:Vec<Vec<u8>>,store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,supplies:bool,practice:bool)->io::Result<Vec<crate::scene::Delivery>> {
    item_service_transaction_profile(peer,character,items,changes,completion,store,scene,metadata,supplies,practice,None,None,None)
}
fn item_service_transaction_profile(
    peer: &mut Peer, character: u64,
    items: &[crate::inventory::Command], changes: &[crate::progression::Command],
    mut completion: Vec<Vec<u8>>, store: &mut Store, scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>, supplies: bool, practice: bool, emote:Option<&crate::quest_emotes::Outcome>, profession:Option<&crate::professions::Change>, arwic:Option<(crate::arwic_mines::Action,u64)>,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let saved = &store.characters[&character];
    if !scene.can_observe(character, character) || !saved.progression.alive() || peer.combat {
        return Err(invalid(
            "Item services require a ready living player out of combat.",
        ));
    }
    if items
        .iter()
        .filter(|item| matches!(item, crate::inventory::Command::Grant(_) | crate::inventory::Command::GrantFront(_)))
        .count()
        // A butchered corpse gives up to two traits.
        > if profession.is_some() || emote.is_some() {17} else if arwic.is_some() {3} else {2}
    {
        return Err(invalid("Item service output limit exceeded."));
    }
    let first = items
        .iter()
        .any(|item| item.allocated_id().is_some())
        .then(|| store.next_item_id())
        .transpose()?;
    let (bag, state) = if items.is_empty() && changes.is_empty() {(saved.inventory.clone(),saved.progression.clone())} else {crate::player_transaction::candidate(
        &saved.inventory,
        &saved.progression,
        items,
        changes,
        first,
    )?};
    let mut events = Vec::new();
    let mut removed = Vec::new();
    let mut granted = Vec::new();
    let mut reduced_stacks = Vec::new();
    for command in items {
        match command {
            crate::inventory::Command::ToolWear {..} if profession.is_some() => {},
            crate::inventory::Command::ConsumeDense { item }
            | crate::inventory::Command::ConsumeStackDense { item, .. } => {
                let owned = saved
                    .inventory
                    .items()
                    .find(|owned| owned.id == *item)
                    .ok_or_else(|| invalid("Service item is not owned."))?;
                if !matches!(owned.location, crate::inventory::Location::Bag(_)) {
                    return Err(invalid("Service items must be in your bag."));
                }
                let instance = scene.inventory_instance(character, *item).map_err(other)?;
                events.push(crate::item_services::removed(*item).map_err(other)?);
                events.push(world::destroy_inventory_item(*item, instance));
                removed.push(*item);
            }
            crate::inventory::Command::ConsumePart { item, .. } => {
                // The stack stays with the rest: re-created with its new count, as a split or
                // a merge does (`merge_stacks`).
                let reduced = bag
                    .items()
                    .find(|owned| owned.id == *item)
                    .ok_or_else(|| invalid("Service item is not owned."))?;
                let old = scene.inventory_instance(character, *item).map_err(other)?;
                let new = scene.next_item_instance(character, *item).map_err(other)?;
                events.push(world::destroy_inventory_item(*item, old));
                events.push(world::create_inventory_item(reduced, character, new).map_err(other)?);
                reduced_stacks.push((*item, new));
            }
            crate::inventory::Command::Grant(item) | crate::inventory::Command::GrantFront(item) => {
                let item = bag
                    .items()
                    .find(|owned| owned.id == item.id)
                    .ok_or_else(|| invalid("Service grant absent from final inventory."))?;
                let info = metadata
                    .get(&item.definition)
                    .and_then(|info| *info)
                    .ok_or_else(|| invalid("Service item metadata unavailable."))?;
                let instance = scene
                    .admit_inventory_grant(character, item.id)
                    .map_err(other)?;
                events
                    .push(world::create_inventory_item(item, character, instance).map_err(other)?);
                events.push(crate::item_services::acquired(character, item, info).map_err(other)?);
                granted.push((item.id, instance));
            }
            _ => return Err(invalid("Unsupported item service transition.")),
        }
    }
    events.extend(state.private_changes(&saved.progression));
    if saved.skills.available_credits(&state) != saved.skills.available_credits(&saved.progression) {
        events.push(crate::ui::skills_state(&saved.skills, &state, &saved.skill_used).map_err(other)?);
    }
    events.append(&mut completion);
    if supplies {
        let next = crate::quests::validate_supplies_exchange(&saved.quests, &saved.inventory, items, changes, first.ok_or_else(|| invalid("Missing supply reward identity"))?)?;
        events.extend(next.updates_since(&saved.quests).map_err(other)?);
    }
    if practice {
        let next = crate::quests::validate_practice_craft(&saved.quests, &saved.inventory, items, changes, first.ok_or_else(|| invalid("Missing practice craft output identity"))?)?;
        events.extend(next.updates_since(&saved.quests).map_err(other)?);
        events.push(script::chat(&next.potion_guidance(), 1).map_err(other)?);
    }
    if let Some(outcome)=emote {
        if crate::player_transaction::encode_items(items)!=crate::player_transaction::encode_items(&outcome.items) || crate::player_transaction::encode_changes(changes)!=crate::player_transaction::encode_changes(&outcome.changes) {return Err(invalid("Quest exchange differs from its emote outcome"));}
        let next=outcome.quests.iter().try_fold(saved.quests.clone(),|book,c|book.apply(c)).map_err(other)?;
        events.extend(next.updates_since(&saved.quests).map_err(other)?);
        for line in &outcome.lines {events.push(script::chat(line,1).map_err(other)?);}
    }
    if let Some((action,time))=arwic {
        let plan=crate::arwic_mines::plan(&saved.quests,&saved.inventory,action,store.next_item_id()?,time)?;
        if crate::player_transaction::encode_items(items)!=crate::player_transaction::encode_items(&plan.items) || crate::player_transaction::encode_changes(changes)!=crate::player_transaction::encode_changes(&plan.rewards) {return Err(invalid("Lesson exchange differs from canonical plan"));}
        events.extend(plan.quests.updates_since(&saved.quests).map_err(other)?);
        if plan.quests.arwic.learned && !saved.quests.arwic.learned {events.push(crate::crafting::learned_recipe(crate::arwic_mines::RECIPE).map_err(other)?);}
        let guidance=plan.quests.arwic.guidance().split_whitespace().collect::<Vec<_>>().join(" ");
        events.push(crate::interaction::dialogue("Arwic Mines and Crafting Quest",&guidance).map_err(other)?);
    }
    peer.validate_scene_batch(&events)?;
    if let Some((action,time))=arwic {store.arwic_transaction(&peer.session.account,character,action,time)?;}
    else if let Some(outcome)=emote {store.emote_transaction(&peer.session.account,character,outcome)?;}
    else if let Some(change)=profession {store.profession_transaction(&peer.session.account,character,items,changes,change)?;}
    else {store.player_transaction_quest(&peer.session.account, character, items, changes, supplies, practice)?;}
    for item in removed {
        scene.inventory_removed(character, item).map_err(other)?;
    }
    for (item, instance) in reduced_stacks {
        scene.apparel_changed(character, item, instance).map_err(other)?;
    }
    for (item, instance) in granted {
        scene
            .inventory_granted(character, item, instance)
            .map_err(other)?;
    }
    let updates = scene.player_stats(character, &state);
    peer.queue_ordered_batch(events)?;
    Ok(updates)
}

fn group_online(
    active: &BTreeMap<u64, SocketAddr>,
    store: &Store,
    scene: &crate::scene::Scene,
) -> BTreeMap<u64, crate::groups::Member> {
    active
        .keys()
        .filter_map(|id| {
            let saved = store.characters.get(id).filter(|saved| !saved.deleted)?;
            let point = scene.player_placement(*id)?;
            let level = saved.progression.level();
            Some((
                *id,
                crate::groups::Member {
                    id: *id,
                    name: saved.name.clone(),
                    level: u32::from(level.number),
                    health: saved.progression.health,
                    max_health: level.health,
                    vigor: saved.progression.vigor,
                    max_vigor: level.vigor,
                    cell: point.cell,
                },
            ))
        })
        .collect()
}

/// A kill's experience split among the earner's fellowship (`fellowship_xp`) and paid, each
/// recipient's progression published as `encounters::Event::Owner` publishes the earner's.
/// Fellows count by the level rule whether alive or not (the panel's parts); only living ones
/// in the world receive.
fn share_kill_xp(
    player: u64,
    xp: u64,
    groups: &crate::groups::Runtime,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    active: &BTreeMap<u64, SocketAddr>,
    initialize_ui: bool,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let fellows = groups.sharing(player).map(|sharing| {
        let fellows: Vec<crate::fellowship_xp::Fellow> = sharing
            .members
            .iter()
            .filter(|id| active.contains_key(id))
            .filter_map(|id| {
                let saved = store.characters.get(id).filter(|c| !c.deleted)?;
                Some(crate::fellowship_xp::Fellow {
                    id: *id,
                    level: u32::from(saved.progression.level().number),
                    placement: scene.player_placement(*id),
                })
            })
            .collect();
        (sharing.leader, sharing.shared, fellows)
    });
    let paid = match fellows {
        Some((leader, shared, fellows)) => crate::fellowship_xp::split(player, xp, leader, shared, &fellows),
        None => vec![(player, xp)],
    };
    let mut updates = Vec::new();
    for (id, amount) in paid {
        let Some(saved) = store.characters.get(&id).filter(|c| !c.deleted && c.progression.alive()) else {
            continue;
        };
        let (account, previous) = (saved.account.clone(), saved.progression.clone());
        if store
            .progression_command(&account, id, crate::progression::Command::AwardXp(amount))
            .is_err()
        {
            continue;
        }
        let saved = &store.characters[&id];
        let state = saved.progression.clone();
        updates.extend(scene.player_stats(id, &state));
        if initialize_ui
            && saved.skills.available_credits(&previous) != saved.skills.available_credits(&state)
        {
            updates.push(crate::scene::Delivery {
                recipient: id,
                data: crate::ui::skills_state(&saved.skills, &state, &saved.skill_used)
                    .map_err(other)?
                    .into(),
            });
        }
        for wire in state.private_changes(&previous) {
            updates.push(crate::scene::Delivery { recipient: id, data: wire.into() });
        }
    }
    Ok(updates)
}

fn group_admit(
    deliveries: &[crate::groups::Delivery],
    active: &BTreeMap<u64, SocketAddr>,
    peers: &BTreeMap<SocketAddr, Peer>,
) -> io::Result<BTreeMap<SocketAddr, Vec<Vec<u8>>>> {
    let mut packets: BTreeMap<SocketAddr, Vec<Vec<u8>>> = BTreeMap::new();
    for delivery in deliveries {
        // A disconnect plan also clears the departed player's UI. There is no
        // transport to send that clear to, and reconnect starts with empty UI.
        if let Some(addr) = active.get(&delivery.recipient) {
            packets
                .entry(*addr)
                .or_default()
                .push(delivery.data.clone());
        }
    }
    for (addr, events) in &packets {
        peers
            .get(addr)
            .ok_or_else(|| invalid("Group recipient disconnected."))?
            .validate_scene_batch(events)?;
    }
    Ok(packets)
}

fn group_publish(
    packets: BTreeMap<SocketAddr, Vec<Vec<u8>>>,
    peers: &mut BTreeMap<SocketAddr, Peer>,
) -> io::Result<()> {
    for (addr, events) in packets {
        peers
            .get_mut(&addr)
            .ok_or_else(|| invalid("Group recipient disconnected."))?
            .queue_ordered_batch(events)?;
    }
    Ok(())
}

// Explicit development services assigned to these DAT-backed NPC types.
// Placements are discovered from the live scene, not a fixed list of IIDs.
const TRAINING_MENTOR_ENTITY: u32 = crate::training_dialogue::MENTOR_ENTITY;

/// One trigger of the quest emote tables (`quest_emotes`) on a world object for a player: the
/// matching set's lines, quest stages, items, experience and move, committed as one record.
#[allow(clippy::too_many_arguments)]
fn use_emote(peer:&mut Peer,character:u64,target:u64,trigger:crate::quest_emotes::Trigger,item:Option<u64>,store:&mut Store,
    scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,
    encounters:&mut Option<crate::encounters::Encounters>,bootstrap:&mut Option<crate::bootstrap::Bootstrap>,now:u64)->io::Result<Vec<crate::scene::Delivery>> {
    let saved=&store.characters[&character];
    if peer.combat || !saved.progression.alive() {return Err(invalid("Do that while alive and out of combat."));}
    let object=scene.examination_content(character,target).ok_or_else(||invalid("That is not visible."))?;
    let used=item.map(|id|saved.inventory.items().find(|i|i.id==id && matches!(i.location,crate::inventory::Location::Bag(_))).map(|i|i.definition)
        .ok_or_else(||invalid("Use an item from your backpack."))).transpose()?;
    let roll=(now % 1000) as f32/1000.;
    let outcome=crate::quest_emotes::installed()?.run(trigger,object.entity,target,used,&saved.quests,&saved.inventory,&saved.progression,store.next_item_id()?,roll,crate::recovered_quests::now(),&crate::quest_emotes::item)?
        .ok_or_else(||invalid("Nothing happens."))?;
    let player=scene.player_placement(character).ok_or_else(||invalid("Player is not ready."))?;
    let distance=player.distance(object.placement).filter(|d|*d<=outcome.range)
        .ok_or_else(||invalid(&format!("Move within {} metres.",outcome.range)))?;
    let mut events=vec![crate::interaction::success(character,target,player,distance).map_err(other)?];
    if outcome.moveto.is_some() && peer.last_world_use_ms.is_some_and(|last|now.saturating_sub(last)<5000) {return Err(invalid("Wait a few seconds."));}
    for (table,key) in &outcome.tells {events.push(crate::quest_emotes::tell(*table,*key).map_err(other)?);}
    peer.admit_npc_dialogue(now)?;
    let prepared=outcome.moveto.map(|to|prepare_forced_move(peer,character,to,scene,bootstrap)).transpose()?;
    if let Some(prepared)=&prepared {
        events.push(prepared.wire.clone());
        events.push(script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?);
    }
    let mut updates=item_service_transaction_profile(peer,character,&outcome.items,&outcome.changes,events,store,scene,metadata,false,false,Some(&outcome),None,None)?;
    // The quest's effects (ApplyEffect / RemoveEffect) reach the client's strip (`WM_Effect`);
    // an escort starts or ends.
    if let Some(runtime)=encounters.as_mut() {
        let unix=crate::recovered_quests::now();
        runtime.sync_quest_effects(character,&store.characters[&character].quests.quest_effects(unix),unix,now,true);
        if let Some(entity)=outcome.escort {
            if runtime.start_escort(character,entity,scene).is_none() {events_note(peer,"Your escort is not nearby.")?;}
        }
        if outcome.end_escort {runtime.end_escort(character);}
    }
    if let Some(prepared)=prepared {
        if let Some(runtime)=encounters {runtime.stop(character);}
        updates.extend(scene.update(character,prepared.motion.position_stamp,&prepared.update).map_err(other)?);
        peer.motion=Some(prepared.motion);peer.target=None;peer.last_world_use_ms=Some(now);
    }
    Ok(updates)
}

/// A craft's quest emotes (`quest_emotes` Crafted sets): each item the craft made (`made`, entity
/// DIDs) runs the set named for it, committed as one record each.
#[allow(clippy::too_many_arguments)]
fn crafted_emotes(peer:&mut Peer,character:u64,made:&[u32],store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,encounters:&mut Option<crate::encounters::Encounters>,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    trigger_emotes(peer,character,crate::quest_emotes::Trigger::Crafted,made,store,scene,metadata,encounters,now)
}
/// Runs the sets of `trigger` named for each of `entities` (no world object: a craft, an escort's
/// loss) for the player, each committed as one record.
#[allow(clippy::too_many_arguments)]
fn trigger_emotes(peer:&mut Peer,character:u64,trigger:crate::quest_emotes::Trigger,made:&[u32],store:&mut Store,scene:&mut crate::scene::Scene,
    metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,encounters:&mut Option<crate::encounters::Encounters>,now:u64)
    ->io::Result<Vec<crate::scene::Delivery>> {
    let Some(tables)=crate::quest_emotes::tables() else {return Ok(Vec::new())};
    let mut updates=Vec::new();
    let unix=crate::recovered_quests::now();
    for definition in made {
        let saved=&store.characters[&character];
        if !tables.answers(trigger,*definition,0,None,&saved.quests,unix) {continue;}
        let Some(outcome)=tables.run(trigger,*definition,0,None,&saved.quests,&saved.inventory,&saved.progression,
            store.next_item_id()?,(now % 1000) as f32/1000.,unix,&crate::quest_emotes::item)? else {continue};
        let mut events=Vec::new();
        for (table,key) in &outcome.tells {events.push(crate::quest_emotes::tell(*table,*key).map_err(other)?);}
        updates.extend(item_service_transaction_profile(peer,character,&outcome.items,&outcome.changes,events,store,scene,metadata,false,false,Some(&outcome),None,None)?);
        if let Some(runtime)=encounters.as_mut() {
            runtime.sync_quest_effects(character,&store.characters[&character].quests.quest_effects(unix),unix,now,true);
        }
    }
    Ok(updates)
}

fn events_note(peer:&mut Peer,text:&str)->io::Result<()> {
    peer.queue_ordered_batch(vec![script::chat(text,1).map_err(other)?])
}

fn npc_use_completion(
    owner: u64,
    target: u64,
    scene: &crate::scene::Scene,
) -> io::Result<Vec<u8>> {
    let player = scene
        .player_placement(owner)
        .ok_or_else(|| invalid("Player is not ready."))?;
    let npc = scene
        .examination_content(owner, target)
        .ok_or_else(|| invalid("NPC is not visible."))?;
    let distance = player.distance(npc.placement).filter(|d| *d <= 4.0);
    let Some(distance) = distance else {
        return Err(invalid("Move within four metres of the NPC."));
    };
    crate::interaction::success(owner, target, player, distance).map_err(other)
}
fn npc_interaction_events(owner: u64, target: u64, scene: &crate::scene::Scene,
    name: &str, text: &str) -> io::Result<Vec<Vec<u8>>> {
    Ok(vec![npc_use_completion(owner,target,scene)?,
        crate::interaction::speech(target,name,text).map_err(other)?])
}

fn use_npc_dialogue(peer:&mut Peer, owner:u64,target:u64,store:&Store,scene:&crate::scene::Scene,now:u64)
    ->io::Result<()> {
    if peer.combat || !store.characters[&owner].progression.alive() {
        return Err(invalid("Speak to the NPC while alive and out of combat."));
    }
    let object=scene.examination_content(owner,target).ok_or_else(||invalid("NPC is not visible."))?;
    let profile=crate::npc_dialogue::profiles().get(&object.entity).ok_or_else(||invalid("Unknown NPC."))?;
    let completion=npc_use_completion(owner,target,scene)?;
    peer.admit_npc_dialogue(now)?;
    let text=profile.reply(peer.npc_dialogue.page(owner,target));
    peer.queue_ordered_batch(vec![completion,
        crate::interaction::speech(target,&profile.name,&text).map_err(other)?,
        crate::interaction::dialogue(&profile.name,&text).map_err(other)?])?;
    peer.npc_dialogue.advance(owner,target,profile.topics.len());
    Ok(())
}

fn mentor_interaction_events(owner: u64, target: u64, scene: &crate::scene::Scene, text: &str) -> io::Result<Vec<Vec<u8>>> {
    let mut events = npc_interaction_events(owner, target, scene, "Pectar, Training Master", text)?;
    events.push(crate::interaction::dialogue("Pectar, Training Master", text).map_err(other)?);
    Ok(events)
}

fn use_training_mentor(
    peer: &mut Peer,
    character: u64,
    target: u64,
    store: &mut Store,
    scene: &mut crate::scene::Scene,
    metadata: &BTreeMap<u32, Option<crate::inventory_request::Metadata>>,
    now: u64,
) -> io::Result<Vec<crate::scene::Delivery>> {
    let player = scene
        .player_placement(character)
        .ok_or_else(|| invalid("Player is not ready."))?;
    let mentor = scene
        .examination_content(character, target)
        .filter(|object| object.entity == TRAINING_MENTOR_ENTITY)
        .ok_or_else(|| invalid("That training mentor is not visible."))?;
    if !player.distance(mentor.placement).is_some_and(|distance| distance <= 4.0) {
        return Err(invalid("Move within four metres of the training mentor."));
    }
    if peer.combat || !store.characters[&character].progression.alive() {
        return Err(invalid(
            "Speak to the training mentor while alive and out of combat.",
        ));
    }
    peer.admit_npc_dialogue(now)?;
    let book = &store.characters[&character].quests;
    if book.journey_phase() != 0 {
        return quest_action(peer, character, if book.journey_phase() == 3 { "journey claim" } else { "status" }, store, scene, Some(target));
    }
    if book.potion_phase() == 3 {
        return quest_action(peer, character, "journey", store, scene, Some(target));
    }
    if book.potion_phase() != 0 {
        return quest_action(peer, character, if book.potion_phase() == 2 { "potion claim" } else { "status" }, store, scene, Some(target));
    }
    if book.supplies() == Some(true) {
        return quest_action(peer, character, "potion", store, scene, Some(target));
    }
    if book.get().is_some_and(|state| state.claimed) {
        if book.supplies().is_none() {
            return quest_action(peer, character, "supplies", store, scene, Some(target));
        }
        if book.supplies() == Some(false) {
            if let Ok(items) = crate::quests::supplies_items(&store.characters[&character].inventory, store.next_item_id()?) {
                let changes = crate::quests::rewards(&crate::quests::Command::ClaimSupplies);
                let next = book.apply(&crate::quests::Command::ClaimSupplies).map_err(other)?;
                let events = mentor_interaction_events(character, target, scene, &next.guidance())?;
                return item_service_transaction_quest(peer, character, &items, &changes, events, store, scene, metadata, true, false);
            }
        }
    }
    let action = if book.get().is_none() {
        "start"
    } else if book.ready() {
        "claim"
    } else {
        "status"
    };
    quest_action(peer, character, action, store, scene, Some(target))
}

fn merchant_ready(character: u64, scene: &crate::scene::Scene, target: Option<u64>) -> io::Result<u64> {
    scene.nearby_vendor(character, target)
        .ok_or_else(|| invalid("Move within four metres of the vendor whose shop you opened."))
}

fn merchant_action(
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

fn use_gathering(peer:&mut Peer,character:u64,item:u64,target:u64,store:&mut Store,scene:&mut crate::scene::Scene,metadata:&BTreeMap<u32,Option<crate::inventory_request::Metadata>>,loot:&crate::loot::Loot,geometry:Option<&mut crate::geometry::Geometry>,now_ms:u64)->io::Result<Vec<crate::scene::Delivery>>{
    use crate::{harvesting as h,professions as p,inventory::{Command,Location}};
    if peer.last_inventory_ms.is_some_and(|last|now_ms.saturating_sub(last)<1500){return Err(invalid("Wait for your gathering action to finish."))}
    let saved=&store.characters[&character];let time=crate::recovered_quests::now();
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

fn craft_item(
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
        let mut plan=crate::professions::plan(&saved.professions,&saved.inventory,request,store.next_item_id()?,u32::from_le_bytes(entropy),crate::recovered_quests::now(),metadata)?;
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

fn corpse_action(peer:&mut Peer,character:u64,request:crate::loot::Request,loot:&mut crate::loot::Loot,
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
        loot.claimed(container,pending.id);collected+=1;
    }
    if collected==0 && store.characters[&character].inventory.free_slot().is_some(){return Err(invalid("No loot was collected."));}
    Ok(updates)
}
/// The stack a drop on an occupied slot merges into: the bag item at the request's destination
/// when it is another stack of the same kind as the moved one and has room for the amount
/// (`ContainerDisplay::HandleListBoxDrop` names the slot under the pointer, an item or not).
fn merge_target(inventory:&crate::inventory::Inventory,request:&crate::inventory_request::MoveRequest)->Option<u64>{
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
fn merge_stacks(peer:&mut Peer, character:u64, request:&crate::inventory_request::MoveRequest,
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
fn split_stack(peer:&mut Peer, character:u64, request:&crate::inventory_request::MoveRequest,
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
fn drop_ground_item(peer:&mut Peer, character:u64, request:&crate::inventory_request::DropRequest,
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
fn pickup_ground_item(peer:&mut Peer,character:u64,id:u64,store:&mut Store,scene:&mut crate::scene::Scene,
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
/// Developer login only (loopback): the client's account name is trusted.
pub fn run(config: Config) -> io::Result<()> {
    run_with_auth(config, false)
}
/// With `tickets`, every login spends a one-use ticket from the account service.
pub fn run_with_auth(config: Config, tickets: bool) -> io::Result<()> {
    run_auth_mode(config, tickets, false)
}
/// Loopback password accounts, plus no-ticket login for the configured developer profile.
pub fn run_local_accounts(config: Config) -> io::Result<()> {
    run_auth_mode(config, true, true)
}
fn run_auth_mode(config: Config, tickets: bool, local_accounts: bool) -> io::Result<()> {
    println!("{}", crate::licensing::notice());
    std::fs::create_dir_all(&config.data)?;
    // The game's own connection for spending login tickets (the account service issues them).
    let mut auth_data = if tickets {
        config.database.ensure_exists()?;
        Some(config.database.connect()?)
    } else {
        None
    };
    if !config.bind.is_ipv4() || ((auth_data.is_none() || local_accounts) && !config.bind.ip().is_loopback()) {
        return Err(invalid(
            "developer authentication requires an IPv4 loopback bind",
        ));
    }
    if auth_data.is_some() {
        if local_accounts {
            println!("Loopback accounts enabled; only the configured existing developer profile may use no-ticket login.");
        } else {
            println!("Public native-ticket authentication enabled; developer/no-ticket login is disabled.");
        }
    }
    if config.bind.port() == 0 || config.bind.port() == u16::MAX {
        return Err(invalid("choose a nonzero base port with a free port+1"));
    }
    if config.account.is_empty()
        || config.account.len() > 32
        || !config
            .account
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(invalid(
            "account must have 1-32 ASCII letters, digits or underscores",
        ));
    }
    if config.map_travel && (!config.bind.ip().is_loopback() || !config.initialize_ui || config.game_data.is_none()) {
        return Err(invalid("--test-map-travel requires a loopback server with DAT world/UI enabled."));
    }
    let mut map_landscape = if config.map_travel {
        println!("Local testing: double-click map travel enabled.");
        Some(crate::map_travel::Landscape::load(config.game_data.as_deref().unwrap())?)
    } else { None };
    let mut bootstrap = config
        .game_data
        .as_deref()
        .map(crate::bootstrap::Bootstrap::load)
        .transpose()?;
    // Replaying trained skills can initialize the process-wide skill catalog.
    // Select the native data profile before opening the character store.
    // The scripted quests' tables (`quest_emotes`) beside the world table, before the journal
    // replays; a bad row refuses the start.
    let quest_tables = if let Some(prepared) = &config.prepared {
        Some((crate::quest_emotes::Tables::parse_files(prepared.quest_files())?, "the database".to_owned()))
    } else if let Some(world) = config.content.as_deref().filter(|p| p.is_file()) {
        let directory = world.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        Some((crate::quest_emotes::Tables::load(directory)?, directory.display().to_string()))
    } else {
        None
    };
    if let Some((tables, directory)) = quest_tables {
        // Each item a set grants needs a profile: the Shelter's pinned five, or one read from the
        // DATs with the inventory metadata (`quest_emotes::install_items`, below), which refuses
        // the start on an item it cannot read.
        if let Some((set, entity)) = tables.gives().find(|(_, e)| e >> 24 != 0x47) {
            return Err(invalid(&format!("emote set {set} gives {entity:08x}, which is not an entity")));
        }
        println!("Quest tables: {} quests from {directory}.", tables.quests().count());
        crate::quest_emotes::install(tables)?;
    }
    let mut store = Store::open(&config.database)?;
    store.set_save_interval(config.save_interval);
    // The hunts the AC2 Vault rows replace (`hunting_quests::retired`): their rows must be loaded,
    // and each character's state of them moves onto those rows' quests.
    if let Some(tables) = crate::quest_emotes::tables() {
        if let Some(id) = crate::hunting_quests::retired_hunts().iter().find(|id| !tables.quests().any(|q| q.journal_id == **id)) {
            return Err(invalid(&format!("retired hunt {id:08x} has no quest in the loaded quest tables (quest-registry-vault.tsv)")));
        }
        let moved = store.migrate_retired_hunts()?;
        if moved > 0 { println!("Retired hunts moved onto their AC2 Vault quests for {moved} characters."); }
    }
    // UI initialization sends the data's own UI state, so its client must read that data
    // release: converted build-7148 data, or the installed 7230 data for 7230 clients.
    if config.initialize_ui && bootstrap.is_none() {
        return Err(invalid(
            "UI initialization requires build-7148 or 7230 game data (--game-data)",
        ));
    }
    let mut inventory_metadata = BTreeMap::new();
    let mut merchant_catalog = None;
    // The money bag's coins (`money`), where the merchants run.
    let mut money_coin: Option<crate::money::Coin> = None;
    // Development profile for the tradeskills live tests (PYREALIUM_DEV_TRADESKILLS=1; never set by a
    // player's server): the DAT's recipes, surveying, mining and the starter tools.
    let mut trade_assets: Option<crate::tradeskills::Assets> = None;
    if (config.training_kit || config.lifestones || config.portals) && !config.initialize_ui {
        return Err(invalid(
            "training kits and world interactions require --initialize-ui true",
        ));
    }
    // The quest content (deliveries, exploration and hunting quests, NPC dialogue, the Arwic
    // mines lesson, recovered loot) runs wherever the world and its UI scripts are served.
    let quest_gameplay = config.initialize_ui;
    let starter_equipment = config.training_kit || quest_gameplay;
    // Rescue the Shelter Hold (`shelter.rs`) runs wherever the client's UI scripts are served.
    let shelter_gameplay = config.initialize_ui;
    let initial_ui = if config.initialize_ui {
        let directory = config.game_data.as_ref().unwrap();
        let mut portal = crate::dat::Dat::open(directory.join("portal.dat"))?;
        let wlib = portal.read(0x56000005)?;
        crate::ui::validate_wlib(&wlib).map_err(|e| other(format!("HUD WLIB: {e}")))?;
        crate::ui::validate_skills_wlib(&wlib).map_err(|e| other(format!("skills WLIB: {e}")))?;
        crate::skills::validate_assets(&mut portal)?;
        crate::abilities::validate_assets(&mut portal)?;
        crate::emotes::validate_assets(&mut portal)?;
        crate::quests::validate_assets(&mut portal)?;
        crate::crafting::validate_wlib(&wlib).map_err(|e| other(format!("crafting WLIB: {e}")))?;
        // These packages are executed by the client's VM.
        crate::ui::validate_layouts(&wlib, &crate::merchants::LAYOUTS).map_err(|e| other(format!("merchants WLIB: {e}")))?;
        crate::ui::validate_layouts(&wlib, &crate::groups::LAYOUTS).map_err(|e| other(format!("groups WLIB: {e}")))?;
        crate::ui::validate_layouts(&wlib, &crate::interaction::LAYOUTS).map_err(|e| other(format!("interaction WLIB: {e}")))?;
        crate::ui::validate_layouts(&wlib, &crate::allegiance::LAYOUTS).map_err(|e| other(format!("allegiance WLIB: {e}")))?;
        if starter_equipment
            || config.lifestones
            || config.portals
            || store
                .characters
                .values()
                .any(|c| !c.deleted && c.inventory.items().next().is_some())
        {
            crate::ui::validate_inventory_wlib(&wlib).map_err(|e| other(format!("inventory WLIB: {e}")))?;
            if config.training_kit || config.lifestones || config.portals {
                // String tables live in the locale archive, not portal.dat.
                let mut locale = crate::dat::Dat::open(directory.join("local_English.dat"))?;
                locale.read(0x25000014)?;
            }
            if shelter_gameplay {
                for (definition,metadata) in crate::shelter::validate(&mut portal)? {inventory_metadata.insert(definition,Some(metadata));}
            }
            // The items the quest tables grant (`quest_emotes::install_items`).
            if let Some(tables) = crate::quest_emotes::tables() {
                for (definition,metadata) in crate::quest_emotes::install_items(tables,&mut portal)? {inventory_metadata.entry(definition).or_insert(Some(metadata));}
            }
            if quest_gameplay {
                for (definition,metadata) in crate::apparel::validate(&mut portal)?.into_iter().chain(crate::arwic_mines::validate(&mut portal)?) {inventory_metadata.insert(definition,Some(metadata));}
                inventory_metadata.insert(crate::recovered_quests::BAG,Some(crate::recovered_quests::delivery_metadata(&mut portal)?));
            }
            if starter_equipment {
                for (definition, metadata) in crate::starter_gear::validate(&mut portal)? {
                    inventory_metadata.insert(definition, Some(metadata));
                }
            }
            if config.training_kit || quest_gameplay {
                let catalog = crate::merchants::Catalog::validate(&mut portal)?;
                for (definition, metadata) in catalog.entries() {
                    inventory_metadata.insert(definition, Some(metadata));
                }
                merchant_catalog = Some(catalog);
                let coin = crate::money::Coin::load(&mut portal)?;
                inventory_metadata.insert(crate::money::COIN, Some(coin.metadata));
                money_coin = Some(coin);
                crate::crafting::validate_assets(&mut portal)?;
                crate::professions::validate_assets(&mut portal,&mut inventory_metadata)?;
                if config.training_kit && std::env::var_os("PYREALIUM_DEV_TRADESKILLS").is_some() {
                    let assets = crate::tradeskills::Assets::load(&mut portal)?;
                    for (definition, info) in assets.metadata() {
                        inventory_metadata.entry(definition).or_insert(Some(info));
                    }
                    println!("Tradeskills: {} recipes, surveying and mining enabled (development profile).", assets.catalog.len());
                    trade_assets = Some(assets);
                }
                let vigor = crate::crafting::validate_vigor(&mut portal)?;
                inventory_metadata.insert(crate::crafting::VIGOR_OUTPUT, Some(vigor));
            }
            for item in store.characters.values().filter(|c|!c.deleted).flat_map(|c|c.inventory.items()).chain(store.drops.values().map(|d|&d.item)) {
                    portal.read(item.definition)?;
                    portal.read(item.visual)?;
                    if !inventory_metadata.contains_key(&item.definition) {
                        if inventory_metadata.len() >= 4096 {
                            return Err(invalid("inventory metadata capacity exceeded"));
                        }
                        let metadata =
                            crate::inventory_request::Metadata::load(&mut portal, item.definition);
                        if let Err(error) = &metadata {
                            eprintln!(
                                "Bag sorting unavailable for definition {:#010x}: {error}",
                                item.definition
                            );
                        }
                        inventory_metadata.insert(item.definition, metadata.ok());
                    }
            }
        }
        println!(
            "Experimental HUD and owned-inventory initialization enabled; native acceptance pending."
        );
        Some(crate::ui::empty_player().map_err(other)?)
    } else {
        None
    };
    let content = match config.content.as_deref() {
        _ if config.prepared.is_some() => {
            let directory = config.game_data.as_deref().ok_or_else(|| invalid("prepared content requires game data"))?;
            let content = crate::content::Content::parse_text(&config.prepared.as_ref().unwrap().world, directory)?;
            println!("Loaded {} prepared content objects from the database.", content.len());
            content
        }
        Some(path) if path == std::path::Path::new("none") => crate::content::Content::default(),
        Some(path) if path == std::path::Path::new("training") => {
            let directory = config
                .game_data
                .as_deref()
                .ok_or_else(|| invalid("--content training requires --game-data"))?;
            let content = crate::content::Content::training(directory)?;
            println!(
                "Loaded {} placed world objects.",
                content.len()
            );
            content
        }
        Some(path) if path == std::path::Path::new("observed") => {
            let directory = config
                .game_data
                .as_deref()
                .ok_or_else(|| invalid("--content observed requires --game-data"))?;
            let content = crate::content::Content::observed(directory)?;
            println!(
                "Loaded {} objects from the observed actor snapshot; stationary presentation only.",
                content.len()
            );
            content
        }
        Some(path) => {
            let directory = config
                .game_data
                .as_deref()
                .ok_or_else(|| invalid("--content requires --game-data"))?;
            let content = crate::content::Content::load(path, directory)?;
            println!(
                "Loaded {} stationary content objects; interactions and AI remain unfinished.",
                content.len()
            );
            content
        }
        None => match config.game_data.as_deref() {
            Some(directory) => {
                let content = crate::content::Content::bundled(directory)?;
                println!(
                    "Loaded {} bundled stationary content objects; interactions and AI remain unfinished.",
                    content.len()
                );
                content
            }
            None => crate::content::Content::default(),
        },
    };
    let mut content = content;
    if config.admin_bind.is_some() && config.content_overlay.is_none() {
        return Err(invalid("--admin-bind needs --content-overlay: live edits are saved there"));
    }
    if let Some(path) = config.content_overlay.as_deref().filter(|path| !path.exists() && config.admin_bind.is_some()) {
        println!("World-builder overlay {} does not exist yet; live edits will create it.", path.display());
    }
    // The prepared world under the overlay: live edits override and restore its objects.
    let base_objects = if config.admin_bind.is_some() { content.object_map() } else { BTreeMap::new() };
    if let Some(path) = config.content_overlay.as_deref().filter(|path| path.exists()) {
        let directory = config.game_data.as_deref().ok_or_else(|| invalid("--content-overlay requires --game-data"))?;
        let added = content.extend(path, directory)?;
        println!("Loaded {added} world-builder overlay objects from {}.", path.display());
    }
    // An overlay's rules sit beside it as NAME.rules.tsv unless named explicitly.
    let sidecar = config.world_rules.clone().or_else(|| config.content_overlay.as_deref().map(|path| path.with_extension("rules.tsv")));
    let rules_path = sidecar.clone().filter(|path| path.is_file());
    let world_rules = match rules_path.as_deref() {
        Some(path) => {
            let rules = crate::world_rules::Rules::load(path)?;
            println!("Loaded world rules from {}: {} stats, {} behaviour, {} loot tables, {} patrols, {} spawn groups.",
                path.display(), rules.stats.len(), rules.behaviour.len(), rules.loot.len(), rules.patrols.len(), rules.groups.len());
            rules
        }
        None => crate::world_rules::Rules::default(),
    };
    let mut projectiles = crate::projectiles::Projectiles::default();
    let mut corpse_loot=crate::loot::Loot::default();
    if let Ok(value)=std::env::var("AC2_TEST_LOOT_SEED") {
        corpse_loot.seed_for_test(value.parse().map_err(|_|invalid("invalid test loot seed"))?)?;
        println!("Test-only reproducible loot seed enabled.");
    }
    if quest_gameplay {
        let mut portal=crate::dat::Dat::open(config.game_data.as_ref().unwrap().join("portal.dat"))?;
        corpse_loot.catalog=crate::loot::Catalog::load_with_rules(&mut portal,&mut inventory_metadata,&world_rules)?;
    }
    // The login's equipment records carry each worn item's implement (field 3 of the 0x1F9
    // record), as `EquipItem_Done` records it during play.
    crate::inventory_request::register_implements(
        inventory_metadata
            .iter()
            .filter_map(|(definition, metadata): (&u32, &Option<crate::inventory_request::Metadata>)| {
                metadata.and_then(|m| m.implement).map(|bits| (*definition, bits))
            }),
    );
    let mut encounters = config
        .encounters
        .as_deref()
        .filter(|path| *path != std::path::Path::new("none"))
        .map(|path| crate::encounters::Encounters::load_with_rules(path, &content, &world_rules))
        .transpose()?;
    if let (Some(runtime),Some(directory))=(&mut encounters,config.game_data.as_deref()) {
        runtime.load_motion(&content,directory)?;
        println!("Creature locomotion loaded from native motion descriptors and placement scales.");
    }
    if config.training_kit || quest_gameplay {
        if let Some(runtime) = &mut encounters {
            // Pyrealium's initial ruleset uses journaled automatic loot without enabling
            // merchant/crafting catalogs from a different DAT revision.
            runtime.enable_training_loot();
        }
    }
    let combat_count=content.objects().filter(|o|encounters.as_ref().is_some_and(|e|e.is_creature(o.id))).count();
    let outdoor_count=content.objects().filter(|o|(1..=64).contains(&(o.placement.cell&65535))
        && encounters.as_ref().is_some_and(|e|e.is_creature(o.id))).count();
    let vendor_count=content.objects().filter(|o|o.service==Some(crate::services::Service::Vendor)).count();
    println!("Population: {combat_count} combat actors ({outdoor_count} outdoors), {vendor_count} native vendors. Captured and separately documented inferred placements; Pyrealium combat/loot rules.");
    let portal_routes = if config.portals {
        let routes = crate::portals::Routes::for_content(
            &content,
            config.game_data.as_deref().unwrap(),
            config.content.as_deref().filter(|_| config.prepared.is_none()),
        )?;
        println!(
            "Loaded {} development portal routes for the selected content pack.",
            routes.len()
        );
        routes
    } else {
        crate::portals::Routes::default()
    };
    // Walking into a routed portal travels as using it does (`portals::Contacts`).
    let mut portal_contacts = if portal_routes.is_empty() {
        None
    } else {
        match crate::portals::Contacts::new(
            &content,
            &portal_routes,
            config.game_data.as_deref().unwrap(),
        ) {
            Ok(contacts) => Some(contacts),
            Err(error) => {
                eprintln!("Portal collision unavailable: {error}");
                None
            }
        }
    };
    let examination_names = if config.initialize_ui {
        crate::examination::Names::load(
            config.game_data.as_deref().unwrap(),
            content.objects().map(|object| object.entity).chain(
                store
                    .characters
                    .values()
                    .filter(|character| !character.deleted)
                    .flat_map(|character| character.inventory.items().map(|item| item.definition))
                    .chain(inventory_metadata.keys().copied()),
            ),
        )?
    } else {
        crate::examination::Names::default()
    };
    let primary = UdpSocket::bind(config.bind)?;
    let mut secondary_addr = config.bind;
    secondary_addr.set_port(config.bind.port() + 1);
    let secondary = UdpSocket::bind(secondary_addr)?;
    primary.set_nonblocking(true)?;
    secondary.set_nonblocking(true)?;
    println!(
        "Listening on {} and {}; {} (7148 / experimental 7230).",
        config.bind,
        secondary_addr,
        if auth_data.is_some() {
            "authenticated native-ticket accounts"
        } else {
            "local developer account configured"
        }
    );
    if bootstrap.is_some() {
        println!(
            "Local human default-body world bootstrap enabled. Custom appearance and gameplay simulation are unfinished."
        );
    } else {
        println!(
            "Character selection enabled. Supply --game-data to test the local human world bootstrap."
        );
    }
    let start = Instant::now();
    let world_clock = crate::clock::WorldClock::open(store.writer())?;
    println!("Native world clock: {:.3} seconds; persistent origin; time sync every 5 seconds.", world_clock.seconds());
    let mut peers: BTreeMap<SocketAddr, Peer> = BTreeMap::new();
    let mut active_characters = BTreeMap::new();
    let mut scene = crate::scene::Scene::with_content(content);
    if let Some(directory) = config.game_data.as_deref() {
        scene = scene.with_visibility(crate::visibility::Visibility::open(directory)?);
    }
    for dropped in store.drops.values() {scene.add_drop(dropped)?;}
    for saved in store.characters.values().filter(|s|!s.deleted){for (item,j) in &saved.professions.1.jobs{
        if j.expires<=crate::recovered_quests::now(){continue}let Some(p)=j.placement()else{continue};
        let m=crate::harvesting::mine(crate::harvesting::map(j.definition).ok_or_else(||invalid("Saved survey definition missing"))?.mine).unwrap();
        scene.prepare_cell(p.cell)?;scene.insert_content_live(crate::content::Object::resource(crate::harvesting::node_id(*item),m.id,m.visual,p)?,true)?;
    }}
    if let Some(runtime) = &mut encounters {
        let mut total=0;let mut outdoor=0;let mut blocks=std::collections::BTreeSet::new();
        for (id, health) in runtime.initial_health() {
            total+=1;
            if let Some(point)=scene.content_placement(id) {
                if (1..=64).contains(&(point.cell&65535)){outdoor+=1;blocks.insert(point.cell>>16);}
            }
            scene.content_stat(id, 0x107, health)?;
            scene.content_stat(id, 0x102, health)?;
        }
        for (id,stat,value) in runtime.inspection_stats() {scene.content_stat(id,stat,value)?;}
        println!(
            "Development encounters enabled with explicit authored rules; original combat parity is unfinished."
        );
        runtime.initialize_spawners(&mut scene,0)?;
        println!("Managed creature point spawners: {total}; outdoors: {outdoor} in {} blocks; {} capacity-limited groups. One lifetime per point; inferred camp capacities and regeneration are authored Pyrealium policy.",blocks.len(),runtime.spawners().group_count());
    }
    let training_quest_enabled = starter_equipment
        && encounters
            .as_ref()
            .is_some_and(|runtime| runtime.includes_entity(crate::quests::TARGET_ENTITY));
    let mut abilities = if starter_equipment {
        let runtime = crate::abilities::Runtime::load().map_err(io::Error::other)?;
        println!("Loaded {} executable DAT skill-effect profiles; specialized effects and recall entitlements remain incomplete.", runtime.supported());
        Some(runtime)
    } else {
        None
    };
    let mut failed_peers = BTreeSet::new();
    let mut next_offer = 0x8160000000000000u64;
    let mut groups = crate::groups::Runtime::default();
    let mut group_cache: BTreeMap<u64, crate::groups::Member> = BTreeMap::new();
    let mut next_group_refresh = 0u64;
    let mut login_limits = crate::auth::RateLimit::new(60);
    let mut generation = 0u64;
    let mut counters = Counters::default();
    let mut buffer = [0u8; 65536];
    let mut next_report = 10_000;
    let mut next_untraining_tick = 0;
    let mut next_stop_check = 0;
    let mut next_checkpoint = 5000;
    let mut next_gun_publish = 0;
    let mut diagnosed_events = BTreeSet::new();
    let mut diagnosed_application_errors = BTreeSet::new();
    let mut diagnosed_packet_errors = BTreeSet::new();
    let mut diagnosed_queues = BTreeSet::new();
    let admin = match (config.admin_bind, config.content_overlay.clone(), sidecar) {
        (Some(bind), Some(world), Some(rules)) => Some(crate::admin::start(bind, &config.data,
            config.game_data.clone().ok_or_else(|| invalid("--admin-bind requires --game-data"))?,
            crate::admin::Paths { world, rules }, base_objects)?),
        _ => None,
    };
    let mut next_resource_cleanup=0;
    loop {
        let now = Instant::now();
        let now_ms = start.elapsed().as_millis() as u64;
        if config
            .run_seconds
            .is_some_and(|seconds| start.elapsed() >= Duration::from_secs(seconds))
        {
            break;
        }
        if now_ms >= next_stop_check {
            let stop = config.data.join("stop.request");
            if stop.exists() {
                std::fs::remove_file(stop)?;
                break;
            }
            next_stop_check = now_ms + 250;
        }
        for (port_index, socket) in [&primary, &secondary].into_iter().enumerate() {
            for _ in 0..64 {
                let (len, addr) = match socket.recv_from(&mut buffer) {
                    Ok(value) => value,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::ConnectionReset | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                counters.received += 1;
                if (auth_data.is_none() && !addr.ip().is_loopback()) || len > 4096 {
                    counters.rejected += 1;
                    continue;
                }
                let bytes = &buffer[..len];
                let h = match Header::decode(bytes) {
                    Ok(h) => h,
                    Err(_) => {
                        counters.rejected += 1;
                        continue;
                    }
                };
                // Endpoint-dependent NAT may map the same client socket to a
                // different source port when it contacts our secondary port.
                // Only a valid current cookie + net ID + iteration on the same
                // IP can map this ACK back to its original login endpoint.
                let addr = if h.flags == wire::CONNECT_ACK
                    && port_index == 1
                    && !peers.contains_key(&addr)
                {
                    let cookie = wire::decode_handshake(bytes)
                        .ok()
                        .and_then(|(_, payload)| wire::decode_connect_ack(payload).ok());
                    cookie
                        .and_then(|cookie| {
                            peers.iter().find_map(|(original, peer)| {
                                if original.ip() != addr.ip()
                                    || peer.id != h.recipient
                                    || peer.iteration != h.iteration
                                {
                                    return None;
                                }
                                let mut candidate = peer.session.clone();
                                candidate
                                    .connect_ack(cookie, now_ms)
                                    .ok()
                                    .map(|()| *original)
                            })
                        })
                        .unwrap_or(addr)
                } else {
                    addr
                };
                if failed_peers.contains(&addr) {
                    continue;
                }
                let mut deliveries = Vec::new();
                let mut group_requests = Vec::new();
                // Keep transport recovery outside fallible application handling.
                // A later world/UI rejection must not discard ACK-covered NAKs.
                let mut recovery_packets = Vec::new();
                let result = (|| -> io::Result<Vec<Vec<u8>>> {
                    if h.flags == wire::LOGON {
                        if port_index != 0 {
                            return Err(invalid("login on wrong port"));
                        }
                        if auth_data.is_some() && !login_limits.allow(addr.ip(), now_ms / 1000) {
                            return Err(invalid("Login rate limit."));
                        }
                        let (_, payload) = wire::decode_handshake(bytes).map_err(other)?;
                        let login = wire::Login::decode(payload).map_err(other)?;
                        // Type 1 is the recovered native admin client's no-ticket path.
                        // This is an explicit local development identity, not password authentication.
                        if auth_data.is_none() && !login.supports_developer_auth(&config.account) {
                            return Err(invalid(&format!(
                                "unsupported login: build={:?} type={:#x} flags={:#x} account_match={} crypto_bytes={} extra_bytes={}",
                                login.version,
                                login.auth_type,
                                login.auth_flags,
                                login.account == config.account,
                                login.crypto_data.len(),
                                login.extra_data.len()
                            )));
                        }
                        let proof = crate::auth::login_proof(&login);
                        if let Some(peer) = peers.get(&addr) {
                            if !peer.session.expired(now_ms) && peer.login_proof == proof {
                                return Ok(vec![peer.connect_packet.clone()]);
                            }
                        }
                        if peers.len() >= 64
                            && !peers.contains_key(&addr)
                            && !(auth_data.is_some()
                                && peers
                                    .values()
                                    .any(|peer| peer.session.account == login.account))
                        {
                            return Err(invalid("session capacity"));
                        }
                        if let Some(root) = auth_data.as_mut() {
                            if !(local_accounts && login.supports_developer_auth(&config.account)) {
                            let token = zeroize::Zeroizing::new(
                                login
                                    .native_ticket()
                                    .map_err(|_| invalid("Public login rejected."))?,
                            );
                            crate::auth::consume(root, &login.account, addr.ip(), &token)?;
                            }
                            // A fresh authenticated login replaces this account's
                            // previous endpoint; it cannot occupy every player slot.
                            let previous: Vec<_> = peers
                                .iter()
                                .filter_map(|(old_addr, peer)| {
                                    (*old_addr != addr && peer.session.account == login.account)
                                        .then_some(*old_addr)
                                })
                                .collect();
                            for old_addr in previous {
                                let old = peers.remove(&old_addr).unwrap();
                                if let Phase::Entering { character }
                                | Phase::InWorld { character } = old.session.phase
                                {
                                    if active_characters.get(&character) == Some(&old_addr) {
                                        active_characters.remove(&character);
                                        if let Some(runtime) = &mut encounters {
                                            runtime.stop(character);
                                        }
                                        deliveries.extend(scene.remove(character));
                                    }
                                }
                            }
                        }
                        generation = generation
                            .checked_add(1)
                            .ok_or_else(|| invalid("session generation exhausted"))?;
                        let id = (1..=u16::MAX)
                            .find(|id| !peers.values().any(|p| p.id == *id))
                            .ok_or_else(|| invalid("session IDs exhausted"))?;
                        let iteration = u16::try_from(generation)
                            .map_err(|_| invalid("restart required before iteration reuse"))?;
                        let secrets = crate::auth::random::<16>()?;
                        let cookie = u64::from_le_bytes(secrets[..8].try_into().unwrap());
                        let incoming = u32::from_le_bytes(secrets[8..12].try_into().unwrap());
                        let outgoing = u32::from_le_bytes(secrets[12..].try_into().unwrap());
                        let connect = wire::Connect {
                            server_time: world_clock.seconds(),
                            cookie,
                            net_id: u32::from(id),
                            outgoing_seed: outgoing,
                            incoming_seed: incoming,
                        };
                        let packet = wire::encode_handshake(
                            Header {
                                flags: wire::CONNECT,
                                recipient: id,
                                iteration,
                                ..Default::default()
                            },
                            &connect.encode(),
                        )
                        .map_err(other)?;
                        let previous = peers.insert(
                            addr,
                            Peer {
                                client_version: login.version.clone(),
                                session: Session::new(login.account.clone(), cookie, now_ms),
                                login_proof: proof,
                                transport: Transport::new(incoming, outgoing, id, iteration),
                                connect_packet: packet.clone(),
                                id,
                                iteration,
                                blob: 0,
                                selection_pending: None,
                                ordered_stamp: 0,
                                motion: None,
                                target: None,
                                combat: false,
                                pending_skill: None,
                                last_chat_ms: None,
                                emotes: crate::emotes::State::default(),
                                last_inventory_ms: None,
                                last_examine_ms: None,
                                last_world_use_ms: None,
            trade: Default::default(),
            deposits: BTreeMap::new(),
                                last_npc_dialogue_ms: None,
                                guide_page: 0,
                                npc_dialogue: Default::default(),
                                merchant_open_ms: None,
                                last_shop_refresh_ms: None,
                                native_shop: None,
                                unhandled_scripts: BTreeSet::new(),
                                scene_outbox: VecDeque::new(),
                                scene_bytes: 0,
                                last_time_sync_ms: now_ms,
                            },
                        );
                        if let Some(previous) = previous {
                            if let Phase::Entering { character } | Phase::InWorld { character } =
                                previous.session.phase
                            {
                                if active_characters.get(&character) == Some(&addr) {
                                    active_characters.remove(&character);
                                    if let Some(runtime) = &mut encounters {
                                        runtime.stop(character);
                                    }
                                    deliveries.extend(scene.remove(character));
                                }
                            }
                        }
                        counters.logins += 1;
                        return Ok(vec![packet]);
                    }
                    let peer = peers
                        .get_mut(&addr)
                        .ok_or_else(|| invalid("unknown peer"))?;
                    if h.recipient != peer.id || h.iteration != peer.iteration {
                        return Err(invalid("stale or wrong session"));
                    }
                    if h.flags == wire::CONNECT_ACK {
                        if port_index != 1 {
                            return Err(invalid("connection acknowledgement on wrong port"));
                        }
                        let (_, payload) = wire::decode_handshake(bytes).map_err(other)?;
                        let cookie = wire::decode_connect_ack(payload).map_err(other)?;
                        let first = peer.session.phase == Phase::Connecting;
                        peer.session.connect_ack(cookie, now_ms).map_err(other)?;
                        if !first {
                            return Ok(vec![peer.transport.send_time_sync(world_clock.seconds(), now).map_err(other)?]);
                        }
                        counters.connected += 1;
                        let characters: Vec<_> = store
                            .characters
                            .values()
                            .filter(|c| c.account == peer.session.account && !c.deleted)
                            .collect();
                        let list = if let Some(profile) = &bootstrap {
                            let selection: Vec<_> = characters
                                .iter()
                                .filter_map(|c| {
                                    c.creation
                                        .as_ref()
                                        .filter(|r| profile.supports(r))
                                        .map(|r| {let mut selection=profile.selection(c.id,r);selection.appearance=crate::starter_gear::appearance(c).ok();selection})
                                })
                                .collect();
                            world::character_set(peer.session.account.as_bytes(), 5, &selection)
                                .map_err(other)?
                        } else if let Some(did) = config.visual_did {
                            let selection: Vec<_> = characters
                                .iter()
                                .map(|c| world::SelectionCharacter {
                                    id: c.id,
                                    name: c.name.clone(),
                                    seconds_greyed_out: 0,
                                    parent_visual_did: did,
                                    appearance: None,
                                })
                                .collect();
                            world::character_set(peer.session.account.as_bytes(), 5, &selection)
                                .map_err(other)?
                        } else {
                            world::empty_character_set(peer.session.account.as_bytes(), 5)
                                .map_err(other)?
                        };
                        // Local asset mode: finish the zero-download DAT phase before
                        // selection can invoke CharacterGenSystem. OnEndDDD initializes
                        // the native VM; skipping it dereferences a null resolver.
                        peer.selection_pending =
                            Some(world::character_set_for(&peer.client_version, list));
                        let mut packets = vec![peer.transport.send_time_sync(world_clock.seconds(), now).map_err(other)?];
                        packets.extend(peer.send_to_queue(5, &0x1003bu32.to_le_bytes(), now)?);
                        return Ok(packets);
                    }
                    if peer.session.phase == Phase::Connecting || peer.session.expired(now_ms) {
                        return Err(invalid("session not connected"));
                    }
                    let received = peer.transport.receive(bytes, now).map_err(other)?;
                    recovery_packets.extend(received.retransmissions);
                    if received.logoff {
                        // Validation includes checksum and current session identity.
                        // The end-of-loop removal releases presence and ownership.
                        store.save()?;
                        failed_peers.insert(addr);
                        return Ok(Vec::new());
                    }
                    peer.session.touch(now_ms);
                    let mut replies = Vec::new();
                    // Wait for delivery of the DAT notification so a lost first
                    // packet cannot let queue1 race ahead of queue5 initialization.
                    if !peer.transport.has_unacknowledged() {
                        if let Some(list) = peer.selection_pending.take() {
                            replies.extend(peer.send_event(&list, now)?);
                        }
                    }
                    if h.sequence != 0 {
                        replies.push(peer.control(wire::ACK, &received.ack.to_le_bytes())?);
                    }
                    if let Some(echo) = received.echo_request {
                        let mut body = echo.to_le_bytes().to_vec();
                        body.extend(0.0f32.to_le_bytes());
                        replies.push(peer.control(wire::ECHO_RESPONSE, &body)?);
                    }
                    for message in received.messages {
                        if !matches!(message.queue, 3 | 4) {
                            counters.rejected += 1;
                            let opcode = message
                                .data
                                .get(..4)
                                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                                .unwrap_or(0);
                            if diagnosed_queues.len() < 16
                                && diagnosed_queues.insert((message.queue, opcode))
                            {
                                eprintln!(
                                    "client queue rejected: queue={} opcode={opcode:#x} bytes={} blob={:#x}",
                                    message.queue,
                                    message.data.len(),
                                    message.blob_id
                                );
                            }
                            continue;
                        }
                        counters.events += 1;
                        let event = match world::decode_client_event(&message.data) {
                            Ok(event) => event,
                            Err(error) => {
                                counters.rejected += 1;
                                let opcode = message
                                    .data
                                    .get(..4)
                                    .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                                    .unwrap_or(0);
                                if diagnosed_events.len() < 32 && diagnosed_events.insert(opcode) {
                                    eprintln!(
                                        "application decode rejected: opcode={opcode:#x} queue={} bytes={} reason={error}",
                                        message.queue,
                                        message.data.len()
                                    );
                                }
                                continue;
                            }
                        };
                        let application = (|| -> io::Result<Vec<Vec<u8>>> {
                            let mut replies = Vec::new();
                            let response = match event {
                                world::ClientEvent::ScriptCall {
                                    function_id,
                                    arguments,
                                } => {
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
                                            let restore=peer.combat && matches!(request,crate::loot::Request::Take{..}|crate::loot::Request::Transmute{..}) && store.characters[&character].progression.alive();
                                            if restore {
                                                if let Some(runtime)=&mut encounters{runtime.stop(character);}
                                                peer.combat=false;
                                                peer.queue_ordered_batch(vec![script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?])?;
                                                deliveries.extend(training_combat_visual(peer,character,&store,&mut scene,false,false,false)?);
                                            }
                                            let result=corpse_action(peer,character,request,&mut corpse_loot,&mut store,&mut scene,&inventory_metadata,now_ms);
                                            // Restore stance on success and rejection, without restarting an
                                            // attack against the selected corpse.
                                            if restore {
                                                peer.combat=true;
                                                peer.queue_ordered_batch(vec![script::private_call(script::COMBAT_STATE,&script::integer(1)).map_err(other)?])?;
                                                deliveries.extend(training_combat_visual(peer,character,&store,&mut scene,true,false,false)?);
                                            }
                                            match result{
                                                Ok(updates)=>deliveries.extend(updates),
                                                Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Loot rejected: {error}"),1).map_err(other)?])?,
                                            }
                                            return Ok(replies);
                                        }
                                        script::Request::Allegiance(query) => {
                                            let target = query.target.unwrap_or(character);
                                            if !store
                                                .characters
                                                .get(&target)
                                                .is_some_and(|saved| !saved.deleted)
                                                || (target != character
                                                    && !scene.can_observe(character, target))
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
                                        script::Request::Group(request) => {
                                            if !config.initialize_ui
                                                || !scene.can_observe(character, character)
                                            {
                                                return Err(invalid(
                                                    "Groups require a ready player with the HUD enabled.",
                                                ));
                                            }
                                            if group_requests.len() >= 4 {
                                                return Err(invalid(
                                                    "Too many group requests in one packet.",
                                                ));
                                            }
                                            group_requests.push((character, request));
                                            return Ok(replies);
                                        }
                                        script::Request::MerchantSell(request) => {
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
                                                    if merchant_catalog.is_none() {
                                                        return Err(invalid(
                                                            "Melting items is not enabled on this server.",
                                                        ));
                                                    }
                                                    if peer.last_inventory_ms.is_some_and(|last| {
                                                        now_ms.saturating_sub(last) < 250
                                                    }) {
                                                        return Err(invalid(
                                                            "Wait briefly before another inventory action.",
                                                        ));
                                                    }
                                                    let saved = &store.characters[&character];
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
                                                        &mut store,
                                                        &mut scene,
                                                        &inventory_metadata,
                                                    )?;
                                                    peer.last_inventory_ms = Some(now_ms);
                                                    Ok(updates)
                                                })(
                                                );
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                            return Ok(replies);
                                        }
                                        script::Request::MoneyWithdraw(request) => {
                                            let result =
                                                (|| -> io::Result<Vec<crate::scene::Delivery>> {
                                                    let coin = money_coin.as_ref().ok_or_else(|| {
                                                        invalid("The money bag is not enabled on this server.")
                                                    })?;
                                                    if peer.last_inventory_ms.is_some_and(|last| {
                                                        now_ms.saturating_sub(last) < 250
                                                    }) {
                                                        return Err(invalid(
                                                            "Wait briefly before another inventory action.",
                                                        ));
                                                    }
                                                    if peer.combat {return Err(invalid("Leave combat before taking money out."));}
                                                    let saved = &store.characters[&character];
                                                    let plan = crate::money::plan_withdraw(
                                                        character,
                                                        &request,
                                                        coin,
                                                        &saved.inventory,
                                                        &saved.progression,
                                                        store.next_item_id()?,
                                                    )
                                                    .map_err(io::Error::other)?;
                                                    let updates = item_service_transaction(
                                                        peer,
                                                        character,
                                                        &plan.items,
                                                        &plan.changes,
                                                        plan.events,
                                                        &mut store,
                                                        &mut scene,
                                                        &inventory_metadata,
                                                    )?;
                                                    peer.last_inventory_ms = Some(now_ms);
                                                    Ok(updates)
                                                })(
                                                );
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => {
                                                    // The bag stays ghosted until a WM_Money answer.
                                                    let mut events = vec![script::chat(&error.to_string(), 1).map_err(other)?];
                                                    events.push(crate::money::bag_done(character).map_err(other)?);
                                                    for event in events {
                                                        replies.extend(peer.send_event(&event, now)?);
                                                    }
                                                }
                                            }
                                            return Ok(replies);
                                        }
                                        script::Request::MerchantBuy(request) => {
                                            let result =
                                                (|| -> io::Result<Vec<crate::scene::Delivery>> {
                                                    if merchant_catalog.is_none()
                                                        || !peer.merchant_open_ms.is_some_and(
                                                            |last| {
                                                                now_ms.saturating_sub(last)
                                                                    <= 30_000
                                                            },
                                                        )
                                                    {
                                                        return Err(invalid(
                                                            "Open the nearby quartermaster before buying.",
                                                        ));
                                                    }
                                                    let merchant =
                                                        merchant_ready(character, &scene, peer.native_shop.as_ref().map(|shop| shop.merchant()))?;
                                                    let shop = peer.native_shop.as_ref().filter(|shop| shop.merchant() == merchant)
                                                    .ok_or_else(|| invalid("The shop has changed; speak to the quartermaster again."))?;
                                                    if peer.last_inventory_ms.is_some_and(|last| {
                                                        now_ms.saturating_sub(last) < 250
                                                    }) {
                                                        return Err(invalid(
                                                            "Wait briefly before buying again.",
                                                        ));
                                                    }
                                                    let saved = &store.characters[&character];
                                                    if peer.combat {return Err(invalid("Leave combat before trading."));}
                                                    let plan = if shop.is_trader() {
                                                        shop.plan_buy_dat(
                                                            character,
                                                            &request,
                                                            &saved.inventory,
                                                            &saved.progression,
                                                            store.next_item_id()?,
                                                            |definition, id, slot, quantity| {
                                                                trade_assets.as_ref()?.make(definition, id, slot, quantity)
                                                            },
                                                        )
                                                    } else {
                                                        shop.plan_buy(
                                                            character,
                                                            &request,
                                                            &saved.inventory,
                                                            &saved.progression,
                                                            store.next_item_id()?,
                                                        )
                                                    }
                                                    .map_err(io::Error::other)?;
                                                    let mut events = plan.events;
                                                    events.push(shop.refresh(merchant_catalog.as_ref().ok_or_else(|| invalid("Merchant catalog is not enabled."))?).map_err(other)?);
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
                                                        &mut store,
                                                        &mut scene,
                                                        &inventory_metadata,
                                                    )?;
                                                    peer.last_inventory_ms = Some(now_ms);
                                                    peer.merchant_open_ms = Some(now_ms);
                                                    Ok(updates)
                                                })(
                                                );
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                            return Ok(replies);
                                        }
                                        script::Request::MerchantRefresh(merchant) => {
                                            let result = (|| -> io::Result<()> {
                                                if peer.last_shop_refresh_ms.is_some_and(|last| now_ms.saturating_sub(last) < 100) {
                                                    return Err(invalid("Wait briefly before refreshing the shop."));
                                                }
                                                if peer.combat
                                                    || !store.characters[&character].progression.alive()
                                                    || !peer.merchant_open_ms.is_some_and(|last| now_ms.saturating_sub(last) <= 30_000)
                                                    || merchant_ready(character, &scene, peer.native_shop.as_ref().map(|shop| shop.merchant())).ok() != Some(merchant)
                                                {
                                                    return Err(invalid("Refresh requires a nearby active shop while alive and out of combat."));
                                                }
                                                let shop = peer.native_shop.as_ref()
                                                    .filter(|shop| shop.merchant() == merchant)
                                                    .ok_or_else(|| invalid("That shop is no longer open."))?;
                                                let catalog = merchant_catalog.as_ref()
                                                    .ok_or_else(|| invalid("Merchant catalog is not enabled."))?;
                                                peer.queue_ordered_batch(vec![shop.refresh(catalog).map_err(other)?])?;
                                                peer.merchant_open_ms = Some(now_ms);
                                                peer.last_shop_refresh_ms = Some(now_ms);
                                                Ok(())
                                            })();
                                            if let Err(error) = result {
                                                replies.extend(peer.send_event(&script::chat(&error.to_string(), 1).map_err(other)?, now)?);
                                            }
                                            return Ok(replies);
                                        }
                                        script::Request::MerchantClose(merchant) => {
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
                                            return Ok(replies);
                                        }
                                        script::Request::MerchantConsignment(merchant) => {
                                            let text = if peer
                                                .native_shop
                                                .as_ref()
                                                .is_some_and(|shop| shop.merchant() == merchant)
                                                && merchant_ready(character, &scene, peer.native_shop.as_ref().map(|shop| shop.merchant())).ok()
                                                    == Some(merchant)
                                            {
                                                "Consignment listings are not available at this training shop. Use the inventory Sell action for fixed buyback. No items or fees were taken."
                                            } else {
                                                "Speak to a nearby quartermaster to open a shop."
                                            };
                                            peer.queue_ordered_batch(vec![
                                                script::chat(text, 1).map_err(other)?
                                            ])?;
                                            return Ok(replies);
                                        }
                                        script::Request::ExamineClear => return Ok(replies),
                                        script::Request::CraftRaise(skill) => {
                                            // Development tradeskills profile (PYREALIUM_DEV_TRADESKILLS) keeps its own per-connection ranks.
                                            if trade_assets.is_some() {
                                                trade_actions::raise(peer, skill)?;
                                                return Ok(replies);
                                            }
                                            let result=(||->io::Result<()>{
                                                if !(config.training_kit||quest_gameplay)||!config.initialize_ui||peer.combat||!scene.can_observe(character,character){return Err(invalid("Crafting training requires a ready player in peace stance."))}
                                                let change=crate::professions::Change{skill,xp:0,time:crate::recovered_quests::now(),raise:true,activity:None};
                                                let next=store.characters[&character].professions.apply(&change)?;
                                                let event=crate::crafting::skill_update(skill,&next).map_err(other)?;peer.validate_scene_batch(&[event.clone()])?;
                                                store.profession_transaction(&peer.session.account,character,&[],&[],&change)?;
                                                peer.queue_ordered_batch(vec![event])?;Ok(())
                                            })();
                                            if let Err(e)=result{peer.queue_ordered_batch(vec![script::chat(&e.to_string(),1).map_err(other)?])?;}
                                            return Ok(replies);
                                        }
                                        script::Request::CraftValidate(request) => {
                                            let saved = &store.characters[&character];
                                            let allowed = (config.training_kit || quest_gameplay)
                                                && config.initialize_ui
                                                && scene.can_observe(character, character)
                                                && saved.progression.alive()
                                                && !peer.combat
                                                && saved.quests.arwic.allowed(request.recipe)
                                                && match (&trade_assets, crate::crafting::recipe(request.recipe)) {
                                                    (Some(assets), None) => trade_actions::validates(assets, &saved.inventory, &request),
                                                    _ => crate::professions::allowed(&request,&saved.inventory),
                                                };
                                            peer.queue_ordered_batch(vec![
                                                crate::crafting::validation_reply(
                                                    &request, allowed,
                                                )
                                                .map_err(other)?,
                                            ])?;
                                            return Ok(replies);
                                        }
                                        script::Request::CraftExecute(request) => {
                                            let before: std::collections::BTreeSet<u64> = store.characters[&character].inventory.items().map(|i| i.id).collect();
                                            let result = if (config.training_kit || quest_gameplay)
                                                && config.initialize_ui
                                            {
                                                match (&trade_assets, crate::crafting::recipe(request.recipe)) {
                                                    (Some(assets), None) => trade_actions::craft(
                                                        peer,
                                                        character,
                                                        &request,
                                                        assets,
                                                        &mut store,
                                                        &mut scene,
                                                        &inventory_metadata,
                                                        now_ms,
                                                    ),
                                                    _ => craft_item(
                                                    peer,
                                                    character,
                                                    &request,
                                                    &mut store,
                                                    &mut scene,
                                                    &inventory_metadata,
                                                    now_ms,
                                                    bootstrap.as_mut().and_then(|b|b.geometry.as_mut()),
                                                ),
                                                }
                                            } else {
                                                Err(invalid("Crafting is not enabled in this server profile."))
                                            };
                                            // A craft that made an item a quest asks for (`Crafted` sets).
                                            let result = result.and_then(|mut updates| {
                                                let made: Vec<u32> = store.characters[&character].inventory.items()
                                                    .filter(|i| !before.contains(&i.id)).map(|i| i.definition).collect();
                                                // The craft is committed; a quest step that cannot follow (a full bag) says so.
                                                match crafted_emotes(peer, character, &made, &mut store, &mut scene, &inventory_metadata, &mut encounters, now_ms) {
                                                    Ok(more) => updates.extend(more),
                                                    Err(error) => peer.queue_ordered_batch(vec![script::chat(&error.to_string(), 1).map_err(other)?])?,
                                                }
                                                Ok(updates)
                                            });
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
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
                                            return Ok(replies);
                                        }
                                        script::Request::Emote(token) => {
                                            let result = (|| -> io::Result<()> {
                                                let saved = &store.characters[&character];
                                                if !config.initialize_ui
                                                    || !scene.can_observe(character, character)
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
                                                        now_ms,
                                                        token,
                                                    )
                                                    .map_err(other)?;
                                                peer.validate_scene_batch(&[wire.clone()])?;
                                                let updates = scene
                                                    .broadcast(character, wire)
                                                    .map_err(other)?;
                                                peer.emotes = next;
                                                deliveries.extend(updates);
                                                Ok(())
                                            })(
                                            );
                                            if let Err(error) = result {
                                                replies.extend(
                                                    peer.send_event(
                                                        &script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                );
                                            }
                                        }
                                        script::Request::UseSkill {
                                            request_id,
                                            skill,
                                            target,
                                        } => {
                                            if store.characters[&character].skills.untraining_id()==skill {
                                                peer.queue_ordered_batch(vec![script::chat("This skill is being untrained.",1).map_err(other)?,script::skill_finished(request_id).map_err(other)?])?;
                                                return Ok(replies);
                                            }
                                            if peer.pending_skill.is_some() {
                                                peer.queue_ordered_batch(vec![
                                                    script::chat("Finish or cancel your current attack before using another skill.", 1).map_err(other)?,
                                                    script::skill_finished(request_id).map_err(other)?,
                                                ])?;
                                                return Ok(replies);
                                            }
                                            if abilities
                                                .as_ref()
                                                .is_some_and(|runtime| skill!=crate::skills::LIFESTONE_RECALL && runtime.supports(skill)
                                                    && !store.characters[&character].creation.as_ref().and_then(|r|crate::appearance::body(r.race,r.sex)).is_some_and(|b|b.starters.contains(&skill)))
                                            {
                                                let result = (|| -> io::Result<()> {
                                                    let saved = &store.characters[&character];
                                                    let aim = if [0, u64::MAX, character].contains(&target) {
                                                        crate::abilities::Aim::Own
                                                    } else if encounters
                                                        .as_ref()
                                                        .is_some_and(|runtime| runtime.is_creature(target))
                                                    {
                                                        crate::abilities::Aim::Creature(target)
                                                    } else if store.characters.contains_key(&target)
                                                        && scene.can_observe(character, target)
                                                    {
                                                        let distance = scene
                                                            .player_placement(character)
                                                            .zip(scene.player_placement(target))
                                                            .map_or(f32::MAX, |(a, b)| {
                                                                a.distance(b).unwrap_or(f32::MAX)
                                                            });
                                                        crate::abilities::Aim::Player(target, distance)
                                                    } else {
                                                        crate::abilities::Aim::Other
                                                    };
                                                    let aim = abilities.as_ref().unwrap().normalize_aim(skill, aim);
                                                    let hand = |slot| {
                                                        saved
                                                            .inventory
                                                            .items()
                                                            .find(|item| item.location == crate::inventory::Location::Equipped(slot))
                                                            .map_or(crate::abilities::Hand::Empty, |item| {
                                                                match inventory_metadata
                                                                    .get(&item.definition)
                                                                    .and_then(|entry| *entry)
                                                                    .and_then(|metadata| metadata.implement)
                                                                {
                                                                    Some(bits) => crate::abilities::Hand::Implement(bits),
                                                                    None => crate::abilities::Hand::Unknown,
                                                                }
                                                            })
                                                    };
                                                    if !matches!(aim,crate::abilities::Aim::Own) {
                                                        let from=scene.player_placement(character);
                                                        let to=scene.player_placement(target).or_else(||scene.content_placement(target));
                                                        if let Some(world)=bootstrap.as_mut().and_then(|b|b.geometry.as_mut()) {
                                                            if !from.zip(to).is_some_and(|(a,b)|world.clear(a,b,1.)) {return Err(invalid("Your target is obstructed."));}
                                                        }
                                                        if let crate::abilities::Aim::Player(_,_) = aim {
                                                            if !store.characters[&target].progression.alive() {return Err(invalid("Your target must be alive."));}
                                                        }
                                                    }
                                                    let cast = abilities
                                                        .as_mut()
                                                        .unwrap()
                                                        .prepare(
                                                            character,
                                                            skill,
                                                            aim,
                                                            (hand(0x40), hand(0x80)),
                                                            &saved.skills,
                                                            &saved.progression,
                                                            now_ms,
                                                        )
                                                        .map_err(io::Error::other)?;
                                                    // The use's world-clock second: SkillData's last-use stamp, from
                                                    // which the client counts the skill's reset down.
                                                    let used = world_clock.seconds();
                                                    let (story, fx) = cast.presentation();
                                                    let instance = saved.instance;
                                                    if let crate::abilities::Outcome::Travel(_) = cast.outcome() {
                                                        return Err(invalid("This recall destination is not yet available."));
                                                    }
                                                    if let Some(runtime)=encounters.as_ref() {runtime.admit_effects()?;}
                                                    if let crate::abilities::Outcome::Support {
                                                        target,
                                                        health,
                                                        vigor,
                                                        mend,
                                                        regen,
                                                    } = cast.outcome().clone()
                                                    {
                                                        let runtime = encounters
                                                            .as_mut()
                                                            .ok_or_else(|| invalid("Healing others requires combat rules on this server."))?;
                                                        let next = cast.next().clone();
                                                        let mut events = next.private_changes(cast.previous());
                                                        events.push(
                                                            crate::ui::skill_update(skill, &saved.skills, Some(used))
                                                                .map_err(other)?,
                                                        );
                                                        let mut restored = Vec::new();
                                                        if health > 0 {
                                                            restored.push(format!("{health} health"));
                                                        }
                                                        if vigor > 0 {
                                                            restored.push(format!("{vigor} vigor"));
                                                        }
                                                        let mut line = if restored.is_empty() {
                                                            "The effect continues on your target.".to_owned()
                                                        } else {
                                                            format!("You restore {} on your target.", restored.join(" and "))
                                                        };
                                                        if mend.is_some() || regen.is_some() {
                                                            line.push_str(" It keeps working for a while.");
                                                        }
                                                        let ratings = rating_sentence(&cast);
                                                        if !ratings.is_empty() {
                                                            line.push(' ');
                                                            line.push_str(&ratings);
                                                        }
                                                        events.push(script::chat(&line, 1).map_err(other)?);
                                                        events.push(script::skill_finished(request_id).map_err(other)?);
                                                        peer.validate_scene_batch(&events)?;
                                                        if !cast.commands().is_empty() {
                                                            store.progression_batch(&peer.session.account, character, cast.commands())?;
                                                        }
                                                        if health > 0 || vigor > 0 {
                                                            runtime.mend(
                                                                character,
                                                                skill,
                                                                target,
                                                                crate::encounters::Over { health, vigor, period_ms: 1000, duration_ms: 0 },
                                                                fx,
                                                                now_ms,
                                                            )?;
                                                        }
                                                        if let Some(over) = mend {
                                                            runtime.mend(character, skill, target, over, 0, now_ms)?;
                                                        }
                                                        if let Some(regen) = regen {
                                                            runtime.boost_regen(target, skill, regen, now_ms)?;
                                                        }
                                                        store_ratings(&mut encounters, &cast, now_ms)?;
                                                        store_effects(&mut encounters, &cast, now_ms);
                                                        cast.commit();
                                                        store.characters.get_mut(&character).unwrap().skill_used.insert(skill, used);
                                                        if story != 0 {
                                                            deliveries.extend(skill_visual(peer, &mut scene, character, instance, story));
                                                        }
                                                        deliveries.extend(scene.player_stats(character, &next));
                                                        peer.queue_ordered_batch(events)?;
                                                        return Ok(());
                                                    }
                                                    if let crate::abilities::Outcome::Strike {
                                                        target,
                                                        bonus,
                                                        vigor_damage,
                                                        weapon_damage,
                                                        cost,
                                                        range,
                                                        burn,
                                                    } = cast.outcome().clone()
                                                    {
                                                        let ratings = rating_sentence(&cast);
                                                        let mut events = vec![
                                                            crate::ui::skill_update(
                                                                skill,
                                                                &saved.skills,
                                                                Some(used),
                                                            )
                                                            .map_err(other)?,
                                                            script::private_call(
                                                                script::COMBAT_STATE,
                                                                &script::integer(1),
                                                            )
                                                            .map_err(other)?,
                                                        ];
                                                        if !ratings.is_empty() {
                                                            events.push(script::chat(&ratings, 1).map_err(other)?);
                                                        }
                                                        peer.validate_scene_batch(&events)?;
                                                        encounters
                                                            .as_mut()
                                                            .ok_or_else(|| invalid("Combat rules are not enabled on this server."))?
                                                            .strike(
                                                                character,
                                                                target,
                                                                now_ms,
                                                                &scene,
                                                                &store,
                                                                crate::encounters::Strike {
                                                                    ratings: cast.other_ratings().map(|(_,r,ms)|(r,ms)),
                                                                    skill,
                                                                    bonus,
                                                                    vigor_damage,
                                                                    weapon_damage,
                                                                    cost,
                                                                    range,
                                                                    behavior: story,
                                                                    fx,
                                                                    burn,
                                                                    effects: cast.creature_effects(),
                                                                },
                                                            )?;
                                                        store_ratings(&mut encounters, &cast, now_ms)?;
                                                        store_effects(&mut encounters, &cast, now_ms);
                                                        cast.commit();
                                                        store.characters.get_mut(&character).unwrap().skill_used.insert(skill, used);
                                                        peer.queue_ordered_batch(events)?;
                                                        peer.combat = true;
                                                        peer.target = Some(target);
                                                        // The strike's Swing or Stop finishes the skill.
                                                        peer.pending_skill = Some(request_id);
                                                        deliveries.extend(training_combat_visual(
                                                            peer, character, &store, &mut scene, true,
                                                            false, false,
                                                        )?);
                                                        return Ok(());
                                                    }
                                                    let next = cast.next().clone();
                                                    let mut events =
                                                        next.private_changes(cast.previous());
                                                    events.push(
                                                        crate::ui::skill_update(
                                                            skill,
                                                            &saved.skills,
                                                            Some(used),
                                                        )
                                                        .map_err(other)?,
                                                    );
                                                    let recovered_vigor = next
                                                        .vigor
                                                        .saturating_sub(cast.previous().vigor);
                                                    let ratings = rating_sentence(&cast);
                                                    let mut text = match (cast.healed(), recovered_vigor) {
                                                        (0, 0) => String::new(),
                                                        (0, vigor) => format!("You recover {vigor} vigor."),
                                                        (health, 0) => format!("You recover {health} health."),
                                                        (health, vigor) => format!("You recover {health} health and {vigor} vigor."),
                                                    };
                                                    if !ratings.is_empty() {
                                                        if !text.is_empty() { text.push(' '); }
                                                        text.push_str(&ratings);
                                                    }
                                                    if text.is_empty() {
                                                        text = "The skill takes effect.".into();
                                                    }
                                                    events.push(script::chat(&text, 1).map_err(other)?);
                                                    events.push(
                                                        script::skill_finished(request_id)
                                                            .map_err(other)?,
                                                    );
                                                    peer.validate_scene_batch(&events)?;
                                                    let healed = cast.healed();
                                                    let (mend, regen) = match cast.outcome() {
                                                        crate::abilities::Outcome::Restore { mend, regen, .. } => (*mend, *regen),
                                                        _ => (None, None),
                                                    };
                                                    if (mend.is_some() || regen.is_some() || cast.caster_ratings().is_some() || cast.other_ratings().is_some()) && encounters.is_none() {
                                                        return Err(invalid("Lasting skill effects require combat rules on this server."));
                                                    }
                                                    if !cast.commands().is_empty() {
                                                        store.progression_batch(
                                                            &peer.session.account,
                                                            character,
                                                            cast.commands(),
                                                        )?;
                                                    }
                                                    store_ratings(&mut encounters, &cast, now_ms)?;
                                                        store_effects(&mut encounters, &cast, now_ms);
                                                    cast.commit();
                                                    store.characters.get_mut(&character).unwrap().skill_used.insert(skill, used);
                                                    if let Some(runtime) = encounters.as_mut() {
                                                        if let Some(over) = mend {
                                                            runtime.mend(character, skill, character, over, 0, now_ms)?;
                                                        }
                                                        if let Some(regen) = regen {
                                                            runtime.boost_regen(character, skill, regen, now_ms)?;
                                                        }
                                                    }
                                                    if story != 0 {
                                                        deliveries.extend(skill_visual(peer, &mut scene, character, instance, story));
                                                    }
                                                    if fx != 0 {
                                                        deliveries.extend(player_fx(peer, &mut scene, character, instance, fx));
                                                    }
                                                    if healed > 0 {
                                                        if let Ok(healed) = i32::try_from(healed) {
                                                            deliveries.extend(
                                                                publish_health_story(
                                                                    peer, &mut scene, character,
                                                                    instance, character, healed,
                                                                ),
                                                            );
                                                        }
                                                    }
                                                    deliveries.extend(
                                                        scene.player_stats(character, &next),
                                                    );
                                                    peer.queue_ordered_batch(events)?;
                                                    Ok(())
                                                })(
                                                );
                                                if let Err(error) = result {
                                                    peer.queue_ordered_batch(vec![
                                                        script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        script::skill_finished(request_id)
                                                            .map_err(other)?,
                                                    ])?;
                                                }
                                                return Ok(replies);
                                            }
                                            if config.initialize_ui
                                                && starter_equipment
                                                && store.characters[&character].creation.as_ref()
                                                    .and_then(|r| crate::appearance::body(r.race, r.sex))
                                                    .is_some_and(|body| body.starters.contains(&skill))
                                            {
                                                let saved = &store.characters[&character];
                                                let attack = (|| -> io::Result<()> {
                                                    if saved.skills.get(skill).is_none() {
                                                        return Err(invalid(
                                                            "The development training attack is not learned.",
                                                        ));
                                                    }
                                                    let body = saved.creation.as_ref().and_then(|r| crate::appearance::body(r.race, r.sex)).unwrap();
                                                    let category = body.starters.iter().position(|id| *id == skill).unwrap();
                                                    if category != crate::starter_gear::equipped_category(&saved.inventory) {
                                                        return Err(invalid("Equip the matching melee, missile or magic weapon before using this skill."));
                                                    }
                                                    let events = vec![
                                                        crate::ui::skill_update(
                                                            skill,
                                                            &saved.skills,
                                                            saved.skill_used.get(&skill).copied(),
                                                        )
                                                        .map_err(other)?,
                                                        script::private_call(
                                                            script::COMBAT_STATE,
                                                            &script::integer(1),
                                                        )
                                                        .map_err(other)?,
                                                    ];
                                                    peer.validate_scene_batch(&events)?;
                                                    encounters
                                                        .as_mut()
                                                        .ok_or_else(|| invalid("Combat rules are not enabled on this server."))?
                                                        .basic_skill(character, target, now_ms, &scene, &store)?;
                                                    peer.queue_ordered_batch(events)?;
                                                    peer.combat = true;
                                                    peer.target = Some(target);
                                                    peer.pending_skill = Some(request_id);
                                                    deliveries.extend(training_combat_visual(
                                                        peer, character, &store, &mut scene, true,
                                                        false, false,
                                                    )?);
                                                    Ok(())
                                                })(
                                                );
                                                if let Err(error) = attack {
                                                    // A queued strike owns its completion until
                                                    // the tick accepts it or cancels it.
                                                    peer.pending_skill = None;
                                                    peer.queue_ordered_batch(vec![
                                                        script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        script::skill_finished(request_id)
                                                            .map_err(other)?,
                                                    ])?;
                                                }
                                                return Ok(replies);
                                            }
                                            if !config.initialize_ui
                                                || !config.lifestones
                                                || skill != crate::skills::LIFESTONE_RECALL
                                            {
                                                peer.queue_ordered_batch(vec![script::chat("This skill requires an effect handler that is not implemented yet.",1).map_err(other)?,script::skill_finished(request_id).map_err(other)?])?;
                                                return Ok(replies);
                                            }
                                            if store.characters[&character]
                                                .skills
                                                .get(skill)
                                                .is_none()
                                            {
                                                peer.queue_ordered_batch(vec![script::chat("Bind to a lifestone to unlock development Recall.",1).map_err(other)?,script::skill_finished(request_id).map_err(other)?])?;
                                                return Ok(replies);
                                            }
                                            match recall_character(
                                                peer,
                                                character,
                                                &mut store,
                                                &mut scene,
                                                &mut encounters,
                                                &mut bootstrap,
                                                now_ms,
                                            ) {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                            peer.queue_ordered_batch(vec![
                                                script::skill_finished(request_id)
                                                    .map_err(other)?,
                                            ])?;
                                        }
                                        script::Request::Examine(target) => {
                                            if !config.initialize_ui {
                                                return Err(invalid(
                                                    "examination requires initialized player UI",
                                                ));
                                            }
                                            if peer.last_examine_ms.is_some_and(|last| {
                                                now_ms.saturating_sub(last) < 100
                                            }) {
                                                return Err(invalid(
                                                    "examination request cooldown",
                                                ));
                                            }
                                            peer.last_examine_ms = Some(now_ms);
                                            match examine_target(
                                                character,
                                                target,
                                                &store,
                                                &scene,
                                                &examination_names,
                                                &corpse_loot,
                                                now_ms,
                                                &effect_lines(&encounters, &abilities, target, now_ms),
                                            ) {
                                                Ok(packet) => {
                                                    deliveries.push(crate::scene::Delivery {
                                                        recipient: character,
                                                        data: packet.into(),
                                                    })
                                                }
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                        }
                                        script::Request::UseObject(request) => {
                                            if request.item>>48==0x8190 || store.characters[&character].inventory.items().any(|i|i.id==request.item&&(crate::harvesting::map(i.definition).is_some()||crate::harvesting::tool(i.definition).is_some())){
                                                if request.owner!=character{return Err(invalid("Gathering owner mismatch"))}
                                                match use_gathering(peer,character,request.item,request.target,&mut store,&mut scene,&inventory_metadata,&corpse_loot,bootstrap.as_mut().and_then(|b|b.geometry.as_mut()),now_ms){
                                                    Ok(updates)=>deliveries.extend(updates),Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
                                                }return Ok(replies);
                                            }
                                            if let Some(assets) = &trade_assets {
                                                let map = trade_actions::is_map(&store.characters[&character].inventory, request.item);
                                                if request.owner == character && request.target == 0 && (map || peer.deposits.contains_key(&request.item)) {
                                                    let result = if !scene.can_observe(character, character) || !store.characters[&character].progression.alive() {
                                                        Err(invalid("Use tradeskills with your living character."))
                                                    } else if map {
                                                        trade_actions::use_map(peer, character, request.item, assets, &mut store, &mut scene, &inventory_metadata, &examination_names)
                                                    } else {
                                                        trade_actions::mine(peer, character, request.item, assets, &mut store, &mut scene, &inventory_metadata)
                                                    };
                                                    match result {
                                                        Ok(updates) => deliveries.extend(updates),
                                                        Err(error) => peer.queue_ordered_batch(vec![script::chat(&error.to_string(), 1).map_err(other)?])?,
                                                    }
                                                    return Ok(replies);
                                                }
                                                // A hunting knife used on a corpse (`butchery.rs`).
                                                if request.owner == character && request.target != 0 && trade_actions::is_knife(&store.characters[&character].inventory, request.item) {
                                                    let result = if !scene.can_observe(character, character) || !store.characters[&character].progression.alive() {
                                                        Err(invalid("Butcher with your living character."))
                                                    } else {
                                                        trade_actions::butcher(peer, character, request.target, assets, &mut store, &mut scene, &inventory_metadata)
                                                    };
                                                    match result {
                                                        Ok(updates) => deliveries.extend(updates),
                                                        Err(error) => peer.queue_ordered_batch(vec![script::chat(&error.to_string(), 1).map_err(other)?])?,
                                                    }
                                                    return Ok(replies);
                                                }
                                            }
                                            if corpse_loot.contains(request.item,now_ms) || scene.examination_content(character,request.item)
                                                .is_some_and(|o|matches!(o.service,Some(crate::services::Service::Chest{..}))){
                                                if request.owner!=character{return Err(invalid("Container owner mismatch"));}
                                                match corpse_action(peer,character,crate::loot::Request::Open(request.item),&mut corpse_loot,&mut store,&mut scene,&inventory_metadata,now_ms){
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Loot rejected: {error}"),1).map_err(other)?])?,
                                                }
                                                return Ok(replies);
                                            }
                                            if request.item>>48==0x8180 {
                                                if !config.initialize_ui || request.owner!=character || request.target!=0 {return Err(invalid("Invalid ground item pickup."));}
                                                match pickup_ground_item(peer,character,request.item,&mut store,&mut scene,&inventory_metadata,now_ms) {
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Pickup rejected: {error}"),1).map_err(other)?])?,
                                                }
                                                return Ok(replies);
                                            }
                                            // Quest emotes (`quest_emotes`): a world object with a Use set that answers now. The
                                            // quest tables go first; the built-in quests below answer only what they do not.
                                            if shelter_gameplay && scene.examination_content(character,request.item).is_some_and(|o|crate::quest_emotes::tables().is_some_and(|t|
                                                t.answers(crate::quest_emotes::Trigger::Use,o.entity,request.item,None,&store.characters[&character].quests,crate::recovered_quests::now()))) {
                                                if request.owner != character {return Err(invalid("Quest use owner mismatch."));}
                                                match use_emote(peer,character,request.item,crate::quest_emotes::Trigger::Use,None,&mut store,&mut scene,&inventory_metadata,&mut encounters,&mut bootstrap,now_ms) {
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,now)?),
                                                }
                                                return Ok(replies);
                                            }
                                            let quest_target=if request.target!=0 && store.characters[&character].inventory.items().any(|i|i.id==request.item&&i.definition==crate::recovered_quests::BAG) {request.target}else{request.item};
                                            if quest_gameplay && scene.examination_content(character,quest_target).is_some_and(|o|crate::recovered_quests::delivery_npc(o.entity)) {
                                                if request.owner!=character{return Err(invalid("Quest use owner mismatch"));}
                                                match use_recovered_delivery(peer,character,quest_target,&mut store,&mut scene,&inventory_metadata) {
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
                                                }
                                                return Ok(replies);
                                            }
                                            let arwic_target=if request.target!=0 && store.characters[&character].inventory.items().any(|i|
                                                i.id==request.item && matches!(i.definition,crate::arwic_mines::HAMMER|crate::arwic_mines::SWORD) && matches!(i.location,crate::inventory::Location::Bag(_))) {request.target}else{request.item};
                                            if quest_gameplay && scene.examination_content(character,arwic_target).is_some_and(|o|o.entity==crate::arwic_mines::NPC) {
                                                if request.owner!=character || (request.target!=0 && arwic_target!=request.target) {return Err(invalid("Quest use owner or target mismatch."));}
                                                let result=(|| {
                                                    let event=npc_use_completion(character,arwic_target,&scene)?;
                                                    peer.admit_npc_dialogue(now_ms)?;
                                                    arwic_exchange(peer,character,crate::arwic_mines::Action::Talk,vec![event],&mut store,&mut scene,&inventory_metadata)
                                                })();
                                                match result {Ok(updates)=>deliveries.extend(updates),Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,}
                                                return Ok(replies);
                                            }
                                            // A bag item used on a world object with quest emotes for it (`ItemUsedOn`).
                                            let used_on=(request.target!=0).then(||store.characters[&character].inventory.items().find(|i|i.id==request.item && matches!(i.location,crate::inventory::Location::Bag(_))).map(|i|i.definition)).flatten()
                                                .filter(|_|scene.examination_content(character,request.target).is_some_and(|o|crate::quest_emotes::tables().is_some_and(|t|t.reacts(o.entity,request.target,crate::quest_emotes::Trigger::ItemUsedOn))));
                                            if request.target!=0 && !(shelter_gameplay && used_on.is_some()) {
                                                return Err(invalid("Targeted use is only supported for a quest item on its quest object."));
                                            }
                                            let service_entity = scene.examination_content(character, request.item)
                                                .map(|object| object.entity);
                                            if quest_gameplay && store.characters[&character].inventory.items().any(|i|
                                                i.id==request.item && i.definition==crate::arwic_mines::GUIDE && matches!(i.location,crate::inventory::Location::Bag(_))) {
                                                if request.owner!=character || !store.characters[&character].progression.alive() {return Err(invalid("Read a book from your living character's backpack."));}
                                                peer.admit_npc_dialogue(now_ms)?;
                                                peer.queue_ordered_batch(vec![crate::arwic_mines::read_page(peer.guide_page).map_err(other)?])?;
                                                peer.guide_page=(peer.guide_page+1)%crate::arwic_mines::content().guide_pages.len();
                                                return Ok(replies);
                                            }
                                            if quest_gameplay && service_entity.is_some_and(crate::exploration_quests::supported) {
                                                if request.owner!=character || request.target!=0 {return Err(invalid("Quest use owner or target mismatch."));}
                                                match use_exploration_npc(peer,character,request.item,&mut store,&mut scene,now_ms) {
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
                                                }
                                                return Ok(replies);
                                            }
                                            if quest_gameplay && service_entity.is_some_and(crate::hunting_quests::supported) {
                                                if request.owner!=character || request.target!=0 {return Err(invalid("Quest use owner or target mismatch."));}
                                                match use_hunting_npc(peer,character,request.item,&mut store,&mut scene,now_ms) {
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,
                                                }
                                                return Ok(replies);
                                            }
                                            if scene.examination_content(character,request.item).is_some_and(|o|o.service==Some(crate::services::Service::Sign)) {
                                                if request.owner!=character || !store.characters[&character].progression.alive(){return Err(invalid("Use the sign with your living character."));}
                                                if !scene.combat_distance(character,request.item).is_some_and(|d|d<=4.) {
                                                    peer.queue_ordered_batch(vec![script::chat("Move within four metres to read this sign.",1).map_err(other)?])?;
                                                } else if let Some(message)=service_entity.and_then(|id|examination_names.description_chat(id)) {
                                                    peer.queue_ordered_batch(vec![message.map_err(other)?])?;
                                                } else {
                                                    peer.queue_ordered_batch(vec![examine_target(character,request.item,&store,&scene,&examination_names,&corpse_loot,now_ms,&effect_lines(&encounters,&abilities,request.item,now_ms))?])?;
                                                }
                                                return Ok(replies);
                                            }
                                            if scene.examination_content(character,request.item).is_some_and(|o|matches!(o.service,Some(crate::services::Service::Door{..}))) {
                                                let result=(||->io::Result<Vec<crate::scene::Delivery>>{
                                                    if request.owner!=character || !store.characters[&character].progression.alive() {return Err(invalid("Use a door with your living character."));}
                                                    if peer.last_world_use_ms.is_some_and(|last|now_ms.saturating_sub(last)<750) {return Err(invalid("Wait for the door to finish moving."));}
                                                    let updates=scene.use_door(character,request.item)?;peer.last_world_use_ms=Some(now_ms);Ok(updates)
                                                })();
                                                match result {Ok(updates)=>deliveries.extend(updates),Err(error)=>peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?,}
                                                return Ok(replies);
                                            }
                                            if service_entity == Some(TRAINING_MENTOR_ENTITY)
                                                && training_quest_enabled
                                            {
                                                if request.owner != character {
                                                    return Err(invalid(
                                                        "Mentor use owner mismatch.",
                                                    ));
                                                }
                                                match use_training_mentor(
                                                    peer,
                                                    character,
                                                    request.item,
                                                    &mut store,
                                                    &mut scene,
                                                    &inventory_metadata,
                                                    now_ms,
                                                ) {
                                                    Ok(updates) => deliveries.extend(updates),
                                                    Err(error) => replies.extend(
                                                        peer.send_event(
                                                            &script::chat(&error.to_string(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?,
                                                    ),
                                                }
                                                return Ok(replies);
                                            }
                                            if scene.examination_content(character,request.item).is_some_and(|o|o.service==Some(crate::services::Service::Vendor))
                                                && merchant_catalog.is_some()
                                            {
                                                if request.owner != character {
                                                    return Err(invalid(
                                                        "Merchant use owner mismatch.",
                                                    ));
                                                }
                                                match merchant_action(
                                                    peer,
                                                    character,
                                                    crate::merchants::Action::List,
                                                    &mut store,
                                                    &mut scene,
                                                    &inventory_metadata,
                                                    merchant_catalog.as_ref().ok_or_else(|| {
                                                        invalid("Merchant catalog is not enabled.")
                                                    })?,
                                                    &mut next_offer,
                                                    now_ms,
                                                    Some(request.item),
                                                    trade_assets.as_ref(),
                                                    &examination_names,
                                                ) {
                                                    Ok(updates) => deliveries.extend(updates),
                                                    Err(error) => replies.extend(
                                                        peer.send_event(
                                                            &script::chat(&error.to_string(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?,
                                                    ),
                                                }
                                                return Ok(replies);
                                            }
                                            if store.characters[&character].inventory.items().any(|i|i.id==request.item&&crate::recovered_quests::use_profile(i.definition).is_some()) {
                                                if request.owner!=character{return Err(invalid("Quest elixir owner mismatch"));}
                                                if let Err(error)=use_recovered_elixir(peer,character,request.item,&mut store,&mut scene) {
                                                    replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,now)?);
                                                }
                                                return Ok(replies);
                                            }
                                            if store.characters[&character].inventory.items().any(
                                                |item| {
                                                    item.id == request.item
                                                        && matches!(
                                                            item.definition,
                                                            crate::item_services::POTION_DEFINITION
                                                                | crate::crafting::VIGOR_OUTPUT
                                                        )
                                                },
                                            ) {
                                                if !config.initialize_ui
                                                    || !starter_equipment
                                                    || request.owner != character
                                                {
                                                    return Err(invalid(
                                                        "Training potion use is not enabled or not owned by this character.",
                                                    ));
                                                }
                                                match use_training_potion(
                                                    peer,
                                                    character,
                                                    request.item,
                                                    &mut store,
                                                    &mut scene,
                                                    now_ms,
                                                ) {
                                                    Ok(updates) => deliveries.extend(updates),
                                                    Err(error) => replies.extend(
                                                        peer.send_event(
                                                            &script::chat(&error.to_string(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?,
                                                    ),
                                                }
                                                return Ok(replies);
                                            }
                                            if shelter_gameplay && used_on.is_some() {
                                                if request.owner!=character {return Err(invalid("Item owner mismatch"));}
                                                match use_emote(peer,character,request.target,crate::quest_emotes::Trigger::ItemUsedOn,Some(request.item),&mut store,&mut scene,&inventory_metadata,&mut encounters,&mut bootstrap,now_ms) {
                                                    Ok(updates)=>deliveries.extend(updates),
                                                    Err(error)=>replies.extend(peer.send_event(&script::chat(&error.to_string(),1).map_err(other)?,now)?),
                                                }
                                                return Ok(replies);
                                            }
                                            if let Some(route) = portal_routes.get(request.item) {
                                                if request.owner != character {
                                                    return Err(invalid(
                                                        "portal owner differs from active character",
                                                    ));
                                                }
                                                match use_portal(
                                                    peer,
                                                    character,
                                                    request.item,
                                                    route,
                                                    &mut store,
                                                    &mut scene,
                                                    &mut encounters,
                                                    &mut bootstrap,
                                                    now_ms,
                                                ) {
                                                    Ok(updates) => deliveries.extend(updates),
                                                    Err(error) => replies.extend(
                                                        peer.send_event(
                                                            &script::chat(&error.to_string(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?,
                                                    ),
                                                }
                                                return Ok(replies);
                                            }
                                            if config.portals
                                                && scene
                                                    .examination_content(character, request.item)
                                                    .is_some_and(|object| {
                                                        matches!(
                                                            object.entity,
                                                            0x4700084d
                                                                | 0x47001451
                                                                | 0x47001454
                                                                | 0x4700369a
                                                        )
                                                    })
                                            {
                                                replies.extend(peer.send_event(&script::chat("This portal's destination is unavailable.",1).map_err(other)?,now)?);
                                                return Ok(replies);
                                            }
                                            if config.lifestones
                                                && scene
                                                    .examination_content(character, request.item)
                                                    .is_some_and(|object| {
                                                        object.entity == 0x470007de
                                                    })
                                            {
                                                let result = (|| -> io::Result<Vec<Vec<u8>>> {
                                                    if request.owner != character
                                                        || peer.combat
                                                        || !store.characters[&character]
                                                            .progression
                                                            .alive()
                                                    {
                                                        return Err(invalid(
                                                            "Bind while alive and out of combat.",
                                                        ));
                                                    }
                                                    if peer.last_world_use_ms.is_some_and(|last| {
                                                        now_ms.saturating_sub(last) < 1000
                                                    }) {
                                                        return Err(invalid(
                                                            "Wait before using the lifestone again.",
                                                        ));
                                                    }
                                                    let player = scene
                                                        .player_placement(character)
                                                        .ok_or_else(|| {
                                                            invalid("Player is not ready.")
                                                        })?;
                                                    let stone = scene
                                                        .content_placement(request.item)
                                                        .ok_or_else(|| {
                                                            invalid("Lifestone is not available.")
                                                        })?;
                                                    let distance: f64 = player
                                                        .coordinates
                                                        .into_iter()
                                                        .zip(stone.coordinates)
                                                        .map(|(a, b)| {
                                                            (f64::from(a) - f64::from(b)).powi(2)
                                                        })
                                                        .sum();
                                                    if player.cell >> 16 != stone.cell >> 16 || distance > 16.0
                                                    {
                                                        return Err(invalid(
                                                            "Move within four meters of the lifestone.",
                                                        ));
                                                    }
                                                    let saved = &store.characters[&character];
                                                    let skills = saved
                                                        .skills
                                                        .with_lifestone_recall(
                                                            &saved.progression,
                                                            saved
                                                                .creation
                                                                .as_ref()
                                                                .map_or(1, |request| request.race),
                                                        )
                                                        .map_err(other)?;
                                                    let mut events = vec![crate::ui::skill_update(crate::skills::LIFESTONE_RECALL,&skills,saved.skill_used.get(&crate::skills::LIFESTONE_RECALL).copied()).map_err(other)?,crate::ui::lifestone_bound(character,request.item,player).map_err(other)?,script::chat("You are bound to this lifestone. Recall is available in your skill list; @recall also returns under development travel rules.",1).map_err(other)?];
                                                    if saved.quests.journey_phase() == 1 {
                                                        events.extend(saved.quests.apply(&crate::quests::Command::BoundLifestone).map_err(other)?.updates_since(&saved.quests).map_err(other)?);
                                                    }
                                                    peer.validate_scene_batch(&events)?;
                                                    store.bind_lifestone(
                                                        &peer.session.account,
                                                        character,
                                                        player,
                                                    )?;
                                                    peer.last_world_use_ms = Some(now_ms);
                                                    Ok(events)
                                                })(
                                                );
                                                match result {
                                                    Ok(events) => {
                                                        peer.queue_ordered_batch(events)?
                                                    }
                                                    Err(error) => replies.extend(
                                                        peer.send_event(
                                                            &script::chat(&error.to_string(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?,
                                                    ),
                                                }
                                                return Ok(replies);
                                            }
                                            if quest_gameplay && service_entity.is_some_and(|d|crate::npc_dialogue::profiles().contains_key(&d))
                                                && encounters.as_ref().is_none_or(|e|!e.is_creature(request.item)) {
                                                if request.owner!=character {return Err(invalid("NPC use owner mismatch."));}
                                                if let Err(error)=use_npc_dialogue(peer,character,request.item,&store,&scene,now_ms) {
                                                    peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?;
                                                }
                                                return Ok(replies);
                                            }
                                            if service_entity.is_some() {
                                                if request.owner!=character || !store.characters[&character].progression.alive(){return Err(invalid("Use the object with your living character."));}
                                                if !scene.combat_distance(character,request.item).is_some_and(|d|d<=4.) {
                                                    peer.queue_ordered_batch(vec![script::chat("Move within four metres to use this object.",1).map_err(other)?])?;
                                                }else{
                                                    let guidance=crate::professions::catalog().workshops.iter().find(|w|Some(w.id)==service_entity).map(|w|match w.effect{
                                                        0x6f002ee6=>"Refining workshop: open Skills > Craft, select Refining, then choose a recipe and quantity. Stay within eight meters while crafting.",
                                                        0x6f002ee7=>"Spellbinding workshop: enchantment extraction and totem application are not implemented yet. Materials will not be consumed.",
                                                        _=>"Crafting workshop: open Skills > Craft and choose a profession. Select a recipe and level, fill its ingredients, then craft while within eight meters. Earn crafting XP and use Raise to advance.",
                                                    }).unwrap_or("Examining this object. Additional scripted actions are not available yet.");
                                                    peer.queue_ordered_batch(vec![examine_target(character,request.item,&store,&scene,&examination_names,&corpse_loot,now_ms,&effect_lines(&encounters,&abilities,request.item,now_ms))?,script::chat(guidance,1).map_err(other)?])?;
                                                }
                                                return Ok(replies);
                                            }
                                            let result = if config.initialize_ui {
                                                change_hand_item(
                                                    peer,
                                                    character,
                                                    &request,
                                                    HandChange::Toggle,
                                                    &mut store,
                                                    &mut scene,
                                                    &inventory_metadata,
                                                    now_ms,
                                                )
                                            } else {
                                                Err(invalid("Inventory is not initialized."))
                                            };
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(
                                                            &format!("Item use rejected: {error}"),
                                                            1,
                                                        )
                                                        .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                        }
                                        script::Request::Equip(request) => {
                                            let result =
                                                (|| -> io::Result<Vec<crate::scene::Delivery>> {
                                                    if !config.initialize_ui {
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
                                                        &mut store,
                                                        &mut scene,
                                                        &inventory_metadata,
                                                        now_ms,
                                                    )
                                                })(
                                                );
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => {
                                                    peer.queue_ordered_batch(vec![script::chat(
                                                        &format!("Equip rejected: {error}"),
                                                        1,
                                                    )
                                                    .map_err(other)?])?
                                                }
                                            }
                                            return Ok(replies);
                                        }
                                        script::Request::UnEquip(request) => {
                                            let result =
                                                (|| -> io::Result<Vec<crate::scene::Delivery>> {
                                                    if !config.initialize_ui {
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
                                                        &mut store,
                                                        &mut scene,
                                                        &inventory_metadata,
                                                        now_ms,
                                                    )
                                                })(
                                                );
                                            match result {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(
                                                            &format!("Unequip rejected: {error}"),
                                                            1,
                                                        )
                                                        .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                        }
                                        script::Request::InventoryDrop(request) => {
                                            if !config.initialize_ui {return Err(invalid("Inventory is not initialized."));}
                                            match drop_ground_item(peer,character,&request,&mut store,&mut scene,&inventory_metadata,now_ms) {
                                                Ok(updates)=>deliveries.extend(updates),
                                                Err(error)=>peer.queue_ordered_batch(vec![script::chat(&format!("Drop rejected: {error}"),1).map_err(other)?])?,
                                            }
                                        }
                                        script::Request::InventoryMove(request) => {
                                            if peer.last_inventory_ms.is_some_and(|last| {
                                                now_ms.saturating_sub(last) < 100
                                            }) {
                                                return Err(invalid("inventory action cooldown"));
                                            }
                                            let partial = config.initialize_ui && store.characters[&character].inventory.items().any(|i|
                                                i.id==request.item && i.location==crate::inventory::Location::Bag(request.source) && request.quantity<i.quantity);
                                            let merge = config.initialize_ui && merge_target(&store.characters[&character].inventory,&request).is_some();
                                            if merge {
                                                if let Err(error)=merge_stacks(peer,character,&request,&mut store,&mut scene,&inventory_metadata,now_ms) {
                                                    replies.extend(peer.send_event(&script::chat(&format!("Item move rejected: {error}"),1).map_err(other)?,now)?);
                                                }
                                            } else if partial {
                                                if let Err(error)=split_stack(peer,character,&request,&mut store,&mut scene,&inventory_metadata,now_ms) {
                                                    replies.extend(peer.send_event(&script::chat(&format!("Item move rejected: {error}"),1).map_err(other)?,now)?);
                                                }
                                            } else {
                                            let bag_items = store.characters[&character].inventory.items()
                                                .filter(|i|matches!(i.location,crate::inventory::Location::Bag(_))).count();
                                            let request = request.clamped(bag_items);
                                            let result = (|| -> io::Result<(crate::inventory::Command, Vec<u8>)> {
                                                if !config.initialize_ui { return Err(invalid("Inventory is not initialized.")); }
                                                let saved = &store.characters[&character];
                                                if !saved.progression.alive() { return Err(invalid("You cannot move items while dead.")); }
                                                let command = request.checked_command(character, &saved.inventory).map_err(other)?;
                                                let candidate = saved.inventory.apply(&command).map_err(other)?;
                                                let item = candidate.items().find(|item| item.id == request.item).ok_or_else(||invalid("Item is not owned."))?;
                                                let metadata = inventory_metadata.get(&item.definition).and_then(|entry| *entry).ok_or_else(||invalid("Bag sorting is unavailable for this item definition."))?;
                                                let acknowledgment = request.acknowledgment(item, metadata).map_err(other)?;
                                                Ok((command, acknowledgment))
                                            })();
                                            match result {
                                                Ok((command, acknowledgment)) => {
                                                    let events = vec![acknowledgment];
                                                    peer.validate_scene_batch(&events)?;
                                                    store.inventory_command(
                                                        &peer.session.account,
                                                        character,
                                                        command,
                                                    )?;
                                                    peer.last_inventory_ms = Some(now_ms);
                                                    peer.queue_ordered_batch(events)?;
                                                }
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(
                                                            &format!("Item move rejected: {error}"),
                                                            1,
                                                        )
                                                        .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                            }
                                        }
                                        script::Request::StoryPlayed => {
                                            // Nothing a retail server did with the list is known (`crate::story`); it is accepted.
                                            eprintln!("story: played-scene list accepted for character {character} ({} bytes)", arguments.len());
                                        }
                                        script::Request::Resurrect => {
                                            match resurrect_character(
                                                peer,
                                                character,
                                                &mut store,
                                                &mut scene,
                                                &mut encounters,
                                                config.lifestones,
                                                &mut bootstrap,
                                            ) {
                                                Ok(updates) => deliveries.extend(updates),
                                                Err(error) => replies.extend(
                                                    peer.send_event(
                                                        &script::chat(&error.to_string(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                ),
                                            }
                                        }
                                        script::Request::Skill { action, skill } => {
                                            if !config.initialize_ui {
                                                replies.extend(peer.send_event(&script::chat("Skills are not available in this session.",1).map_err(other)?,now)?);
                                                return Ok(replies);
                                            }
                                            if action == crate::skills::Action::Raise {
                                                if peer.last_inventory_ms.is_some_and(|last| {
                                                    now_ms.saturating_sub(last) < 100
                                                }) {
                                                    return Err(invalid(
                                                        "skill advancement cooldown",
                                                    ));
                                                }
                                                let saved = &store.characters[&character];
                                                let race = saved
                                                    .creation
                                                    .as_ref()
                                                    .map_or(1, |request| request.race);
                                                match saved.skills.advance(
                                                    skill,
                                                    &saved.progression,
                                                    race,
                                                ) {
                                                    Ok((skills, progression)) => {
                                                        let mut events = progression
                                                            .private_updates()
                                                            .into_iter()
                                                            .filter(|wire| {
                                                                wire[..4]
                                                                    == 0x100d4u32.to_le_bytes()
                                                            })
                                                            .collect::<Vec<_>>();
                                                        events.push(
                                                            crate::ui::skill_update(
                                                                skill,
                                                                &skills,
                                                                saved.skill_used.get(&skill).copied(),
                                                            )
                                                            .map_err(other)?,
                                                        );
                                                        peer.validate_scene_batch(&events)?;
                                                        store.raise_skill(
                                                            &peer.session.account,
                                                            character,
                                                            skill,
                                                        )?;
                                                        peer.last_inventory_ms = Some(now_ms);
                                                        peer.queue_ordered_batch(events)?;
                                                    }
                                                    Err(error) => replies.extend(
                                                        peer.send_event(
                                                            &script::chat(error.message(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?,
                                                    ),
                                                }
                                                return Ok(replies);
                                            }
                                            if store.debit_untraining(character)? {
                                                for wire in store.characters[&character].progression.private_updates() {
                                                    if wire[..4]==0x100d4u32.to_le_bytes(){deliveries.push(crate::scene::Delivery{recipient:character,data:wire.into()});}
                                                }
                                                let saved=&store.characters[&character];
                                                deliveries.push(crate::scene::Delivery{recipient:character,
                                                    data:crate::ui::skills_state(&saved.skills,&saved.progression,&saved.skill_used).map_err(other)?.into()});
                                            }
                                            let command = match action {
                                                crate::skills::Action::Train => {
                                                    crate::skills::Command::Train(skill)
                                                }
                                                crate::skills::Action::Untrain => {
                                                    crate::skills::Command::Untrain(skill)
                                                }
                                                crate::skills::Action::CancelUntrain => crate::skills::Command::CancelUntrain,
                                                crate::skills::Action::Raise => {
                                                    return Err(invalid(
                                                        "advancement dispatch invariant",
                                                    ));
                                                }
                                            };
                                            let saved = &store.characters[&character];
                                            let race = saved
                                                .creation
                                                .as_ref()
                                                .map_or(1, |request| request.race);
                                            if let Err(error) = saved.skills.apply(
                                                command,
                                                &saved.progression,
                                                race,
                                            ) {
                                                replies.extend(
                                                    peer.send_event(
                                                        &script::chat(error.message(), 1)
                                                            .map_err(other)?,
                                                        now,
                                                    )?,
                                                );
                                                return Ok(replies);
                                            }
                                            store.skill_command(
                                                &peer.session.account,
                                                character,
                                                command,
                                            )?;
                                            let saved = &store.characters[&character];
                                            let wire = crate::ui::skills_state(&saved.skills, &saved.progression, &saved.skill_used).map_err(other)?;
                                            deliveries.push(crate::scene::Delivery {
                                                recipient: character,
                                                data: wire.into(),
                                            });
                                        }
                                        script::Request::GameplayOptions(options) => {
                                            store.update_gameplay_options(&peer.session.account, character, options)?;
                                        }
                                        script::Request::UiSettings(settings) => {
                                            store.update_ui(
                                                &peer.session.account,
                                                character,
                                                settings,
                                            )?;
                                        }
                                        script::Request::Hotbar(update) => {
                                            store.update_hotbar(
                                                &peer.session.account,
                                                character,
                                                update,
                                            )?;
                                        }
                                        script::Request::Target(target) => {
                                            if target.is_some_and(|target| {
                                                !scene.can_observe(character, target)
                                            }) {
                                                return Err(invalid("target is not visible"));
                                            }
                                            let packet = match target {
                                                Some(target) => selection_status(
                                                    character, target, &scene, &store,
                                                )?,
                                                None => crate::ui::selection(u64::MAX, None)
                                                    .map_err(other)?,
                                            };
                                            peer.queue_ordered_batch(vec![packet])?;
                                            peer.target = target;
                                            if peer.combat {
                                                if let Some(target) = target.filter(|target| {
                                                    encounters
                                                        .as_ref()
                                                        .is_some_and(|runtime| runtime.is_creature(*target))
                                                }) {
                                                    if let Err(error) = encounters.as_mut().unwrap().start(
                                                        character,
                                                        target,
                                                        now_ms,
                                                        &scene,
                                                        &store,
                                                    ) {
                                                        replies.extend(peer.send_event(
                                                            &script::chat(&error.to_string(), 1)
                                                                .map_err(other)?,
                                                            now,
                                                        )?);
                                                    }
                                                } else {
                                                    if let Some(runtime) = encounters.as_mut() {
                                                        runtime.stop(character);
                                                    }
                                                }
                                            }
                                        }
                                        request @ (script::Request::Combat(_)
                                        | script::Request::Attack(_)
                                        | script::Request::StopAttack(_)) => {
                                            // A delayed stop for an old target must not
                                            // cancel a newly selected/current attack.
                                            if let script::Request::StopAttack(target) = &request {
                                                let current = encounters
                                                    .as_ref()
                                                    .and_then(|runtime| {
                                                        runtime.attacking_target(character)
                                                    })
                                                    .or(peer.target);
                                                if current != Some(*target) {
                                                    return Ok(replies);
                                                }
                                            }
                                            if let Some(request_id) = peer.pending_skill.take() {
                                                peer.queue_ordered_batch(vec![
                                                    script::skill_finished(request_id)
                                                        .map_err(other)?,
                                                ])?;
                                            }
                                            let enabled = match request {
                                                script::Request::StopAttack(_) => {
                                                    if let Some(runtime) = &mut encounters {
                                                        runtime.stop(character);
                                                    }
                                                    false
                                                }
                                                script::Request::Combat(enabled) => {
                                                    let enabled = enabled;
                                                    if enabled
                                                        && !store.characters[&character]
                                                            .progression
                                                            .alive()
                                                    {
                                                        replies.extend(
                                                            peer.send_event(
                                                                &script::chat(
                                                                    "You cannot fight while dead.",
                                                                    1,
                                                                )
                                                                .map_err(other)?,
                                                                now,
                                                            )?,
                                                        );
                                                        return Ok(replies);
                                                    }
                                                    if !enabled {
                                                        if let Some(runtime) = &mut encounters {
                                                            runtime.stop(character);
                                                        }
                                                    } else if let Some(target) = peer.target.filter(|target| {
                                                        encounters.as_ref().is_some_and(|runtime| {
                                                            runtime.is_creature(*target)
                                                        })
                                                    }) {
                                                        if let Err(error) = encounters.as_mut().unwrap().start(
                                                            character, target, now_ms, &scene, &store,
                                                        ) {
                                                            // Stance and selection survive an interrupted attack.
                                                            encounters.as_mut().unwrap().stop(character);
                                                            replies.extend(peer.send_event(
                                                                &script::chat(&error.to_string(), 1)
                                                                    .map_err(other)?,
                                                                now,
                                                            )?);
                                                        }
                                                    } else if encounters.is_some() {
                                                        replies.extend(peer.send_event(
                                                            &script::chat(
                                                                "Combat stance. Select a creature and move within your weapon's reach.",
                                                                1,
                                                            )
                                                            .map_err(other)?,
                                                            now,
                                                        )?);
                                                    }
                                                    enabled
                                                }
                                                script::Request::Attack(target) => {
                                                    if target.is_some_and(|target| {
                                                        target == character
                                                            || !scene.can_observe(character, target)
                                                    }) {
                                                        return Err(invalid(
                                                            "attack target is not a visible other object",
                                                        ));
                                                    }
                                                    if let Some(target) = target {
                                                        let started=encounters.as_mut().ok_or_else(||invalid("Combat rules are not enabled on this server."))
                                                            .and_then(|runtime|runtime.start(character,target,now_ms,&scene,&store));
                                                        if let Err(error) = started {
                                                            if let Some(runtime) = &mut encounters {
                                                                runtime.stop(character);
                                                            }
                                                            peer.target = Some(target);
                                                            replies.extend(
                                                                peer.send_event(
                                                                    &script::chat(
                                                                        &error.to_string(),
                                                                        1,
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
                                                                )?,
                                                            );
                                                            replies.extend(
                                                                peer.send_event(
                                                                    &script::private_call(
                                                                        script::COMBAT_STATE,
                                                                        &script::integer(
                                                                            u32::from(peer.combat),
                                                                        ),
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
                                                                )?,
                                                            );
                                                            // A rejected target stops attacking. Publish the
                                                            // matching pose as well as the private HUD state,
                                                            // including to observers and late scene snapshots.
                                                            if config.initialize_ui {
                                                                deliveries.extend(training_combat_visual(
                                                                    peer, character, &store, &mut scene,
                                                                    peer.combat, false, false,
                                                                )?);
                                                            }
                                                            return Ok(replies);
                                                        }
                                                    } else if let Some(runtime) = &mut encounters {
                                                        runtime.stop(character);
                                                    }
                                                    peer.target = target;
                                                    true
                                                }
                                                _ => unreachable!(),
                                            };
                                            let response = script::private_call(
                                                script::COMBAT_STATE,
                                                &script::integer(u32::from(enabled)),
                                            )
                                            .map_err(other)?;
                                            replies.extend(peer.send_event(&response, now)?);
                                            peer.combat = enabled;
                                            if config.initialize_ui {
                                                deliveries.extend(training_combat_visual(
                                                    peer, character, &store, &mut scene, enabled,
                                                    false, false,
                                                )?);
                                            }
                                        }
                                        request @ (script::Request::Say(_)
                                        | script::Request::Tell { .. }) => {
                                            if peer.last_chat_ms.is_some_and(|last| {
                                                now_ms.saturating_sub(last) < 500
                                            }) {
                                                return Err(invalid("chat rate limit"));
                                            }
                                            let name = &store.characters[&character].name;
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
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let result = action.map_err(io::Error::other).and_then(|action| {
                                                            if merchant_catalog.is_none() { return Err(invalid("Vendors are not enabled for this content profile.")); }
                                                            merchant_action(peer, character, action, &mut store, &mut scene, &inventory_metadata, merchant_catalog.as_ref().ok_or_else(|| invalid("Merchant catalog is not enabled."))?, &mut next_offer, now_ms, None, trade_assets.as_ref(), &examination_names)
                                                        });
                                                        match result {
                                                            Ok(updates) => {
                                                                deliveries.extend(updates)
                                                            }
                                                            Err(error) => replies.extend(
                                                                peer.send_event(
                                                                    &script::chat(
                                                                        &error.to_string(),
                                                                        1,
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
                                                                )?,
                                                            ),
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    let words =
                                                        text.split_whitespace().collect::<Vec<_>>();
                                                    if text.trim().eq_ignore_ascii_case("@talk") && quest_gameplay {
                                                        peer.last_chat_ms=Some(now_ms);
                                                        let result=(|| -> io::Result<()> {
                                                            let target=peer.target.ok_or_else(||invalid("Select an NPC to speak with."))?;
                                                            if encounters.as_ref().is_some_and(|e|e.is_creature(target)) {
                                                                return Err(invalid("That creature cannot hold a conversation."));
                                                            }
                                                            use_npc_dialogue(peer,character,target,&store,&scene,now_ms)
                                                        })();
                                                        if let Err(error)=result {
                                                            peer.queue_ordered_batch(vec![script::chat(&error.to_string(),1).map_err(other)?])?;
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if text.trim().eq_ignore_ascii_case("@spawns") {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let lines=if !config.map_travel {
                                                            vec!["Spawn surveying is available only with local test map travel.".into()]
                                                        } else if let Some((runtime,point))=encounters.as_ref().zip(scene.player_placement(character)) {
                                                            runtime.outdoor_survey(&scene,point)
                                                        } else {
                                                            vec!["No encounter population is loaded.".into()]
                                                        };
                                                        for line in lines {
                                                            replies.extend(peer.send_event(&script::chat(&line,1).map_err(other)?,now)?);
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if words.first().is_some_and(|word| word.eq_ignore_ascii_case("@maptravel")) {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let result = (|| -> io::Result<Vec<crate::scene::Delivery>> {
                                                            let landscape = map_landscape.as_mut().ok_or_else(|| invalid("Map travel is disabled on this server."))?;
                                                            let land = crate::map_travel::point(&words)?;
                                                            let heading = scene.player_placement(character).ok_or_else(|| invalid("Player is not ready."))?.heading;
                                                            let destination = landscape.destination(land, heading)?;
                                                            map_travel_character(peer, character, destination, &mut store, &mut scene, &mut encounters, &mut bootstrap, now_ms)
                                                        })();
                                                        match result {
                                                            Ok(updates) => deliveries.extend(updates),
                                                            Err(error) => replies.extend(peer.send_event(&script::chat(&error.to_string(), 1).map_err(other)?, now)?),
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if words.first().is_some_and(|word| {
                                                        word.eq_ignore_ascii_case("@craft")
                                                    }) {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let result = (|| -> io::Result<Vec<crate::scene::Delivery>> {
                                                            if !config.initialize_ui || !(config.training_kit || quest_gameplay) {
                                                                return Err(invalid("Crafting is not enabled."));
                                                            }
                                                            let recipe = match words.as_slice() {
                                                                [_, name] if name.eq_ignore_ascii_case("health") => crate::crafting::HEALTH_RECIPE,
                                                                [_, name] if name.eq_ignore_ascii_case("vigor") => crate::crafting::VIGOR_RECIPE,
                                                                _ => return Err(invalid("Use @craft health or @craft vigor. Training recipes consume one Mucor Mushroom and 100 Tallow; no craft skill or chance roll is required.")),
                                                            };
                                                            let request = crate::crafting::select_inventory(recipe, &store.characters[&character].inventory, true)
                                                                .map_err(|_| invalid("You need one Mucor Mushroom and a full 100-unit Tallow bundle in your bag."))?;
                                                            craft_item(peer, character, &request, &mut store, &mut scene, &inventory_metadata, now_ms, bootstrap.as_mut().and_then(|b|b.geometry.as_mut()))
                                                        })();
                                                        match result {
                                                            Ok(updates) => {
                                                                deliveries.extend(updates)
                                                            }
                                                            Err(error) => replies.extend(
                                                                peer.send_event(
                                                                    &script::chat(
                                                                        &error.to_string(),
                                                                        1,
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
                                                                )?,
                                                            ),
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if words.first().is_some_and(|word| {
                                                        word.eq_ignore_ascii_case("@group")
                                                    }) && config.initialize_ui
                                                    {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let online = group_online(
                                                            &active_characters,
                                                            &store,
                                                            &scene,
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
                                                            if group_requests.len() >= 4 {
                                                                return Err(invalid(
                                                                    "Too many group requests in one packet.",
                                                                ));
                                                            }
                                                            group_requests
                                                                .push((character, request));
                                                        } else {
                                                            replies.extend(peer.send_event(&script::chat("Fellowship commands: @group create NAME; invite/accept/decline/dismiss PLAYER; leave; disband. Use @g MESSAGE for group chat. Named players must be online.", 1).map_err(other)?, now)?);
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if words.first().is_some_and(|word| {
                                                        word.eq_ignore_ascii_case("@g")
                                                    }) && config.initialize_ui
                                                    {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let members = groups.members(character);
                                                        if members.is_empty() || words.len() < 2 {
                                                            replies.extend(peer.send_event(&script::chat("Join a group and use @g MESSAGE.", 1).map_err(other)?, now)?);
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
                                                                deliveries.push(
                                                                    crate::scene::Delivery {
                                                                        recipient,
                                                                        data: packet.clone(),
                                                                    },
                                                                );
                                                            }
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if words.first().is_some_and(|word| word.eq_ignore_ascii_case("@mentor")) && config.initialize_ui && config.training_kit {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        let result = (|| -> io::Result<()> {
                                                            let saved = &store.characters[&character];
                                                            if peer.combat || !saved.progression.alive() { return Err(invalid("Speak to a mentor while alive and out of combat.")); }
                                                            let mentor = scene.nearby_service(character, TRAINING_MENTOR_ENTITY, None).ok_or_else(|| invalid("Move within four metres of a training mentor."))?;
                                                            let race = saved.creation.as_ref().map_or(1, |request| request.race);
                                                            let text = crate::training_dialogue::topic(&words[1..].join(" "), race, &saved.quests).ok_or_else(|| invalid("Ask @mentor help, gear, combat, healing, supplies, crafting, travel or quests."))?;
                                                            peer.admit_npc_dialogue(now_ms)?;
                                                            let events = mentor_interaction_events(character, mentor, &scene, &text)?;
                                                            peer.queue_ordered_batch(events)?;
                                                            Ok(())
                                                        })();
                                                        if let Err(error) = result { replies.extend(peer.send_event(&script::chat(&error.to_string(), 1).map_err(other)?, now)?); }
                                                        return Ok(replies);
                                                    }
                                                    if words.first().is_some_and(|word| {
                                                        word.eq_ignore_ascii_case("@quest")
                                                    }) && config.initialize_ui
                                                    {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        if !training_quest_enabled
                                                            && store.characters[&character]
                                                                .quests
                                                                .get()
                                                                .is_none()
                                                        {
                                                            replies.extend(peer.send_event(&script::chat("The training quest is not enabled on this server.",1).map_err(other)?,now)?);
                                                            return Ok(replies);
                                                        }
                                                        let action = words[1..].join(" ");
                                                        match quest_action(
                                                            peer, character, &action, &mut store,
                                                            &mut scene, None,
                                                        ) {
                                                            Ok(updates) => {
                                                                deliveries.extend(updates)
                                                            }
                                                            Err(error) => replies.extend(
                                                                peer.send_event(
                                                                    &script::chat(
                                                                        &error.to_string(),
                                                                        1,
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
                                                                )?,
                                                            ),
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if text.trim().eq_ignore_ascii_case("@recall")
                                                        && config.lifestones
                                                    {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        match recall_character(
                                                            peer,
                                                            character,
                                                            &mut store,
                                                            &mut scene,
                                                            &mut encounters,
                                                            &mut bootstrap,
                                                            now_ms,
                                                        ) {
                                                            Ok(updates) => {
                                                                deliveries.extend(updates)
                                                            }
                                                            Err(error) => replies.extend(
                                                                peer.send_event(
                                                                    &script::chat(
                                                                        &error.to_string(),
                                                                        1,
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
                                                                )?,
                                                            ),
                                                        }
                                                        return Ok(replies);
                                                    }
                                                    if text
                                                        .trim()
                                                        .eq_ignore_ascii_case("@resurrect")
                                                        && encounters.is_some()
                                                    {
                                                        peer.last_chat_ms = Some(now_ms);
                                                        match resurrect_character(
                                                            peer,
                                                            character,
                                                            &mut store,
                                                            &mut scene,
                                                            &mut encounters,
                                                            config.lifestones,
                                                            &mut bootstrap,
                                                        ) {
                                                            Ok(updates) => {
                                                                deliveries.extend(updates)
                                                            }
                                                            Err(error) => replies.extend(
                                                                peer.send_event(
                                                                    &script::chat(
                                                                        &error.to_string(),
                                                                        1,
                                                                    )
                                                                    .map_err(other)?,
                                                                    now,
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
                                                    deliveries.extend(
                                                        scene
                                                            .broadcast(character, packet)
                                                            .map_err(other)?,
                                                    );
                                                }
                                                script::Request::Tell { recipient, text } => {
                                                    let recipient_id = store
                                                        .characters
                                                        .values()
                                                        .find(|c| {
                                                            !c.deleted
                                                                && c.name.eq_ignore_ascii_case(
                                                                    &recipient,
                                                                )
                                                                && active_characters
                                                                    .contains_key(&c.id)
                                                                && scene.can_observe(c.id, c.id)
                                                        })
                                                        .map(|c| c.id);
                                                    if let Some(recipient_id) = recipient_id {
                                                        let packet = script::chat(
                                                            &format!("{name} tells you: {text}"),
                                                            0x20,
                                                        )
                                                        .map_err(other)?;
                                                        deliveries.push(crate::scene::Delivery {
                                                            recipient: recipient_id,
                                                            data: packet.into(),
                                                        });
                                                        replies.extend(peer.send_event(&script::chat(&format!("You tell {recipient}: {text}"), 0x20).map_err(other)?, now)?);
                                                    } else {
                                                        replies.extend(
                                                            peer.send_event(
                                                                &script::chat(
                                                                    "That player is not online.",
                                                                    1,
                                                                )
                                                                .map_err(other)?,
                                                                now,
                                                            )?,
                                                        );
                                                    }
                                                }
                                                _ => unreachable!(),
                                            }
                                            peer.last_chat_ms = Some(now_ms);
                                        }
                                    }
                                    return Ok(replies);
                                }
                                world::ClientEvent::DeleteCharacter {
                                    account,
                                    character_id,
                                } => {
                                    if peer.session.phase != Phase::CharacterSelection
                                        || account != peer.session.account.as_bytes()
                                        || active_characters.contains_key(&character_id)
                                    {
                                        return Err(invalid(
                                            "deletion outside owned inactive selection",
                                        ));
                                    }
                                    let list = world::character_set_for(&peer.client_version, selection_list_excluding(
                                        &store, &peer.session.account, bootstrap.as_ref(),
                                        config.visual_did, Some(character_id),
                                    )?);
                                    let characters: Vec<_> = store.characters.values()
                                        .filter(|c| c.account == peer.session.account && !c.deleted && c.id != character_id)
                                        .map(|c| world::MinimalCharacter { id: c.id, name: c.name.clone() })
                                        .collect();
                                    let minimal = world::minimal_character_set(peer.session.account.as_bytes(), 5, &characters).map_err(other)?;
                                    // Prepare all reliable output before persisting deletion. On failure,
                                    // restore the transport's counters, cipher and pending packet window.
                                    let saved_transport = peer.transport.clone();
                                    let saved_blob = peer.blob;
                                    let prepared = (|| -> io::Result<Vec<Vec<u8>>> {
                                        let mut packets = peer.send_event(&world::character_deleted(character_id), now)?;
                                        packets.extend(peer.send_event(&minimal, now)?);
                                        packets.extend(peer.send_event(&list, now)?);
                                        store.delete(&peer.session.account, character_id)?;
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
                                        if !store.characters[&character].progression.alive() {
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
                                        && !bootstrap
                                            .as_mut()
                                            .is_some_and(|p| p.contains_cell(update.placement.cell))
                                    {
                                        return Err(invalid(
                                            "movement references unavailable cell",
                                        ));
                                    }
                                    scene.prepare_cell(update.placement.cell)?;
                                    motion.accept(update).map_err(other)?;
                                    let fall_damage = motion.fall.resolve_landing(
                                        update.placement.coordinates[2],
                                        store.characters[&character].progression.level().health,
                                        ||bootstrap.as_mut().is_some_and(|b|b.terrain_contact(update.placement)),
                                    );
                                    store.set_placement(character, motion.placement)?;
                                    deliveries.extend(
                                        scene
                                            .update(character, motion.position_stamp, &update)
                                            .map_err(other)?,
                                    );
                                    if fall_damage>0 && matches!(peer.session.phase,Phase::InWorld{..}) {
                                        let old=store.characters[&character].progression.clone();
                                        store.progression_command(&peer.session.account,character,crate::progression::Command::Damage(fall_damage))?;
                                        let state=store.characters[&character].progression.clone();
                                        deliveries.extend(scene.player_stats(character,&state));
                                        let mut events=state.private_changes(&old);
                                        events.push(script::chat(&format!("You take {} fall damage.",old.health-state.health),1).map_err(other)?);
                                        if !state.alive(){
                                            if let Some(runtime)=&mut encounters {runtime.stop(character);}
                                            peer.combat=false;peer.target=None;
                                            if let Some(request)=peer.pending_skill.take(){events.push(script::skill_finished(request).map_err(other)?);}
                                            events.push(script::private_call(script::COMBAT_STATE,&script::integer(0)).map_err(other)?);
                                            events.push(crate::ui::death_notice("You died from a fall.").map_err(other)?);
                                            events.push(crate::ui::lock_player(true).map_err(other)?);
                                            deliveries.extend(player_body_alive(peer, &mut scene, character, store.characters[&character].instance, false));
                                        }
                                        deliveries.extend(publish_health_story(peer,&mut scene,character,store.characters[&character].instance,character,-((old.health-state.health) as i32)));
                                        peer.queue_ordered_batch(events)?;
                                    }
                                    // The player walked into a portal: it travels as a use does.
                                    // A refused travel (level, combat, the five-second wait) says
                                    // nothing here, as the body keeps touching the portal.
                                    if matches!(peer.session.phase, Phase::InWorld { .. })
                                        && store.characters[&character].progression.alive()
                                    {
                                        let saved = &store.characters[&character];
                                        let visual = saved
                                            .creation
                                            .as_ref()
                                            .and_then(|c| crate::appearance::body(c.race, c.sex))
                                            .map(|body| body.visual)
                                            .or(config.visual_did);
                                        let placement = peer.motion.as_ref().map(|m| m.placement);
                                        let touched = placement
                                            .zip(visual)
                                            .zip(portal_contacts.as_mut())
                                            .and_then(|((placement, visual), contacts)| {
                                                contacts.touched(visual, placement)
                                            });
                                        if let Some((portal, route)) = touched
                                            .and_then(|portal| Some((portal, portal_routes.get(portal)?)))
                                        {
                                            if let Ok(updates) = use_portal(
                                                peer,
                                                character,
                                                portal,
                                                route,
                                                &mut store,
                                                &mut scene,
                                                &mut encounters,
                                                &mut bootstrap,
                                                now_ms,
                                            ) {
                                                deliveries.extend(updates);
                                            }
                                        }
                                    }
                                    // Quest emotes' Enter sets (`quest_emotes`): the player came within the
                                    // radius of their world object, and a set answers for this player now.
                                    if shelter_gameplay && matches!(peer.session.phase, Phase::InWorld { .. })
                                        && store.characters[&character].progression.alive()
                                    {
                                        let hit = crate::quest_emotes::tables().and_then(|t| {
                                            let me = scene.player_placement(character)?;
                                            let book = &store.characters[&character].quests;
                                            let now = crate::recovered_quests::now();
                                            t.enter_points().find(|(iid, range)| {
                                                scene.examination_content(character, *iid).is_some_and(|o| {
                                                    me.distance(o.placement).is_some_and(|d| d <= *range)
                                                        && t.answers(crate::quest_emotes::Trigger::Enter, o.entity, *iid, None, book, now)
                                                })
                                            }).map(|(iid, _)| iid)
                                        });
                                        if let Some(point) = hit {
                                            if let Ok(updates) = use_emote(peer, character, point, crate::quest_emotes::Trigger::Enter, None,
                                                &mut store, &mut scene, &inventory_metadata, &mut encounters, &mut bootstrap, now_ms) {
                                                deliveries.extend(updates);
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
                                    deliveries
                                        .extend(scene.look_at(character, target).map_err(other)?);
                                    return Ok(replies);
                                }
                                world::ClientEvent::ForceCreate { target } => {
                                    // Answered only in the world; see `Scene::force_create`.
                                    if let Phase::InWorld { character } = peer.session.phase {
                                        deliveries.extend(scene.force_create(character, target));
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
                                    deliveries
                                        .extend(scene.look(character, yaw, pitch).map_err(other)?);
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
                                        bootstrap.as_ref().filter(|p| p.supports(&request))
                                    {
                                        if store
                                            .characters
                                            .values()
                                            .any(|c| c.name.eq_ignore_ascii_case(&request.name))
                                        {
                                            world::reject_character_creation(
                                                world::CreationRejection::NameInUse,
                                            )
                                        } else if store
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
                                            match store
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
                                        if bootstrap.is_none() {
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
                                        && store.characters.get(&character_id).is_some_and(|c| {
                                            c.account == peer.session.account && !c.deleted
                                        });
                                    let error = if owned {
                                        if peer.session.phase != Phase::CharacterSelection {
                                            return Err(invalid("world entry outside selection"));
                                        }
                                        if active_characters.contains_key(&character_id) {
                                            return Err(invalid("character already active"));
                                        }
                                        if let Some(profile) = &mut bootstrap {
                                            if !profile.supports_client(&peer.client_version) {
                                                eprintln!(
                                                    "world entry refused: client {} cannot load this DAT bootstrap's GameplayOptionsProfile; use a matching client/data build",
                                                    peer.client_version
                                                );
                                                replies.extend(peer.send_event(
                                                &world::character_error(
                                                    world::CharacterError::CouldNotPlaceCharacter,
                                                ),
                                                now,
                                            )?);
                                                return Ok(replies);
                                            }
                                            if let Some(request) = store
                                                .characters
                                                .get(&character_id)
                                                .and_then(|c| c.creation.as_ref())
                                                .filter(|r| profile.supports(r))
                                                .cloned()
                                            {
                                                let saved_placement = store
                                                    .placement(character_id)
                                                    .unwrap_or(profile.start(&request));
                                                let placement=if store.characters[&character_id].progression.alive() &&
                                                    scene.content_placement(0x8140000000030048).is_some() {
                                                    crate::shelter::recover_vault_arrival(saved_placement)
                                                } else {saved_placement};
                                                if placement!=saved_placement {
                                                    store.relocate(&peer.session.account,character_id,placement)?;
                                                    eprintln!("Recovered character {character_id} from obstructed Vault arrival");
                                                }
                                                if !profile.contains_cell(placement.cell) {
                                                    return Err(invalid(
                                                        "saved placement references unavailable cell",
                                                    ));
                                                }
                                                if starter_equipment {
                                                    match store.grant_starter_gear(&peer.session.account, character_id) {
                                                        Ok(true) => eprintln!("Racial starter gear saved for character {character_id}"),
                                                        Ok(false) => {},
                                                        Err(error) => eprintln!("Starter gear deferred for character {character_id}: {error}"),
                                                    }
                                                    if request.race == 1 && store.characters[&character_id].training_kit {
                                                        store.ensure_training_attack(&peer.session.account, character_id)?;
                                                    }
                                                }
                                                if config.training_kit {
                                                    let saved = &store.characters[&character_id];
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
                                                        if store.grant_training_supplies(
                                                            &peer.session.account,
                                                            character_id,
                                                        )? {
                                                            eprintln!(
                                                                "Development training supplies saved for character {character_id}"
                                                            );
                                                        }
                                                    }
                                                }
                                                if config.lifestones {
                                                    let saved = &store.characters[&character_id];
                                                    if saved.binding.is_some()
                                                        && saved.progression.alive()
                                                        && saved
                                                            .skills
                                                            .get(crate::skills::LIFESTONE_RECALL)
                                                            .is_none()
                                                        && saved.skills.entries().count() < 128
                                                    {
                                                        store.ensure_lifestone_recall(
                                                            &peer.session.account,
                                                            character_id,
                                                        )?;
                                                    }
                                                }
                                                if store.characters[&character_id].skills.entries().count() + 3 <= 128
                                                    && store.ensure_starter_skills(
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
                                                if let Some(assets) = &trade_assets {
                                                    for material in &crate::tradeskills::MATERIALS {
                                                        let saved = &store.characters[&character_id];
                                                        if !saved.progression.alive()
                                                            || !saved.inventory.dense_bags()
                                                            || saved.inventory.items().any(|i| i.definition == material.starter_tool)
                                                        {
                                                            continue;
                                                        }
                                                        let Some(slot) = saved.inventory.free_slot() else { break };
                                                        let id = store.next_item_id()?;
                                                        if let Some(item) = assets.make(material.starter_tool, id, slot, 1) {
                                                            store.inventory_command(&peer.session.account, character_id, crate::inventory::Command::Grant(item))?;
                                                        }
                                                    }
                                                }
                                                // Development knob (PYREALIUM_DEV_CRAFT_SUPPLIES=1): 100 coal (`wsl_coal`) and 100
                                                // flawless pyreal motes, for tests that skip the traders. The DAT's source
                                                // is the raw-material traders' stores (`store_cragstone.wc` and others sell
                                                // coal at half a coin, `store_cavendo.wc` pyreal at 50: `Assets::trader_store`).
                                                // (`700005ec` is a craft trait record, not a loot table.)
                                                if let (Some(assets), Some(_)) = (&trade_assets, std::env::var_os("PYREALIUM_DEV_CRAFT_SUPPLIES")) {
                                                    for definition in [0x4700_2ad4u32, 0x4700_10fa] {
                                                        let saved = &store.characters[&character_id];
                                                        if !saved.progression.alive()
                                                            || !saved.inventory.dense_bags()
                                                            || saved.inventory.items().any(|i| i.definition == definition)
                                                        {
                                                            continue;
                                                        }
                                                        let Some(slot) = saved.inventory.free_slot() else { break };
                                                        let id = store.next_item_id()?;
                                                        if let Some(item) = assets.make(definition, id, slot, 100) {
                                                            store.inventory_command(&peer.session.account, character_id, crate::inventory::Command::Grant(item))?;
                                                        }
                                                    }
                                                }
                                                // Development knob for the live client tests (never set by a player's
                                                // server): a new character with no experience starts with this much.
                                                if let Some(xp) = std::env::var("PYREALIUM_DEV_START_XP").ok().and_then(|v| v.parse::<u64>().ok()) {
                                                    if xp > 0 && store.characters[&character_id].progression.xp == 0 {
                                                        store.progression_command(
                                                            &peer.session.account,
                                                            character_id,
                                                            crate::progression::Command::AwardXp(xp),
                                                        )?;
                                                        if let Some(damage) = std::env::var("PYREALIUM_DEV_START_DAMAGE").ok().and_then(|v| v.parse::<u32>().ok()).filter(|d| *d > 0) {
                                                            store.progression_command(
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
                                                    if gold > 0 && store.characters[&character_id].progression.gold == 0 {
                                                        store.progression_command(
                                                            &peer.session.account,
                                                            character_id,
                                                            crate::progression::Command::AwardGold(gold),
                                                        )?;
                                                    }
                                                }
                                                if training_quest_enabled
                                                    && store.characters[&character_id]
                                                        .quests
                                                        .get()
                                                        .is_none()
                                                    && store.characters[&character_id]
                                                        .progression
                                                        .alive()
                                                {
                                                    store.quest_command(
                                                        &peer.session.account,
                                                        character_id,
                                                        crate::quests::Command::Accept,
                                                    )?;
                                                }
                                                let instance = store.begin_instance(
                                                    &peer.session.account,
                                                    character_id,
                                                )?;
                                                let mut events = profile
                                                    .events_at_state(
                                                        character_id,
                                                        &request,
                                                        placement,
                                                        instance,
                                                        &store.characters[&character_id]
                                                            .progression,
                                                    )
                                                    .map_err(other)?;
                                                let visual=crate::starter_gear::appearance(&store.characters[&character_id]).map_err(other)?;
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
                                                active_characters.insert(character_id, addr);
                                                scene
                                                    .enter_hidden(
                                                        crate::scene::Body::new(
                                                            character_id,
                                                            placement,
                                                            events[1].clone(),
                                                        )
                                                        .map_err(other)?
                                                        .with_progression(
                                                            &store.characters[&character_id]
                                                                .progression,
                                                        )
                                                        .with_hands(store.characters[&character_id].inventory.items()
                                                            .filter(|item| matches!(item.location, crate::inventory::Location::Equipped(_)))
                                                            .filter(|item| inventory_metadata.get(&item.definition).and_then(|entry| *entry).and_then(|entry| entry.equipment).is_some_and(|hand| hand.hand_slot(item).is_some()))
                                                            .map(|item| inventory_spawn(item, character_id, instance, instance, &inventory_metadata))
                                                            .collect::<io::Result<Vec<_>>>()?)
                                                        .map_err(other)?,
                                                    )
                                                    .map_err(other)?;
                                                for (index, event) in events.iter().enumerate() {
                                                    replies.extend(peer.send_world_event(
                                                        event,
                                                        index == 2,
                                                        now,
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
                                    store.save()?;
                                    replies.extend(peer.send_event(&world::char_exit_game(), now)?);
                                    peer.session.leave_world(character_id).map_err(other)?;
                                    peer.clear_scene_outbox();
                                    peer.motion = None;
                                    peer.target = None;
                                    peer.combat = false;
                                    active_characters.remove(&character_id);
                                    deliveries.extend(scene.remove(character_id));
                                    let list = world::character_set_for(
                                        &peer.client_version,
                                        selection_list(
                                            &store,
                                            &peer.session.account,
                                            bootstrap.as_ref(),
                                            config.visual_did,
                                        )?,
                                    );
                                    replies.extend(peer.send_event(&list, now)?);
                                    return Ok(replies);
                                }
                                world::ClientEvent::SceneRenderingComplete => {
                                    let first =
                                        matches!(peer.session.phase, Phase::Entering { .. });
                                    if first {
                                        // Policy (`crate::story`): a character with no scene earned is given the Prologue's.
                                        if let Phase::Entering { character } = peer.session.phase {
                                            if store.characters.get(&character).is_some_and(|c| c.progression.alive() && c.quests.scenes().next().is_none()) {
                                                for scene in crate::story::PROLOGUE {
                                                    store.quest_command(&peer.session.account, character, crate::quests::Command::Scene(scene))?;
                                                }
                                            }
                                        }
                                        // The quest book's effects join the ledger the login lists.
                                        if let (Phase::Entering { character }, Some(runtime)) = (&peer.session.phase, encounters.as_mut()) { let character = *character;
                                            if let Some(c) = store.characters.get(&character) {
                                                let unix = crate::recovered_quests::now();
                                                runtime.sync_quest_effects(character, &c.quests.quest_effects(unix), unix, now_ms, false);
                                            }
                                        }
                                        if let Some(packet) = initial_ui.as_deref() {
                                            let saved = match peer.session.phase {
                                                Phase::Entering { character } => {
                                                    store.characters.get(&character).filter(|c| {
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
                                                let hotbar = if config.training_kit
                                                    && saved.hotbar.is_empty()
                                                {
                                                    &training_bar
                                                } else {
                                                    &saved.hotbar
                                                };
                                                // The effects still on the character, with the time they were cast at
                                                // on the world's clock.
                                                let login_effects: Vec<crate::ui::EffectView> = encounters
                                                    .as_ref()
                                                    .map(|e| e.applied_to(saved.id, now_ms))
                                                    .unwrap_or_default()
                                                    .into_iter()
                                                    .map(|a| crate::ui::EffectView {
                                                        eid: a.eid,
                                                        did: a.did,
                                                        time_cast: world_clock.seconds() - now_ms.saturating_sub(a.cast_ms) as f64 / 1000.0,
                                                        duration: a.duration_ms as f32 / 1000.0,
                                                        power: a.power,
                                                        pulsed: a.pulsed,
                                                    })
                                                    .collect();
                                                let restored = crate::ui::player_login(
                                                    saved.ui.as_ref(),
                                                    Some(hotbar),
                                                    Some(&saved.inventory),
                                                    Some(&saved.progression),
                                                    Some(&saved.skills),
                                                    Some(&saved.quests),
                                                    saved.gameplay_options.as_ref(),
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
                                                            &inventory_metadata,
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
                                        deliveries
                                            .extend(scene.activate(character).map_err(other)?);
                                        if first {
                                            deliveries.push(crate::scene::Delivery {
                                                recipient: character,
                                                data: script::chat(&crate::licensing::notice(), 1)
                                                    .map_err(other)?
                                                    .into(),
                                            });
                                        }
                                        if first && config.map_travel {
                                            peer.queue_ordered_batch(vec![script::chat(crate::map_travel::CAPABILITY, 1).map_err(other)?])?;
                                        }
                                        if first && config.initialize_ui {
                                            if let Some(assets) = &trade_assets {
                                                deliveries.extend(trade_actions::load(peer, character, &mut store, &mut scene, assets)?);
                                            }
                                            let packets = match &trade_assets {
                                                Some(assets) => trade_actions::registry(peer, assets)?,
                                                None => crate::crafting::initialization_with_quests(config.training_kit || quest_gameplay, &store.characters[&character].professions, &store.characters[&character].quests.arwic)
                                                    .map_err(other)?,
                                            };
                                            for packet in packets {
                                                deliveries.push(crate::scene::Delivery {
                                                    recipient: character,
                                                    data: packet.into(),
                                                });
                                            }
                                        }
                                        if first && config.training_kit {
                                            deliveries.push(crate::scene::Delivery { recipient: character, data: script::chat(
                                                "Training: equip the sword in your bag. Click an Idol Spawn, stand within three metres, and enter combat. Your attacks repeat until you leave combat. The first hotbar button swings once with the training attack skill. Speak to Pectar for the three-kill reward. Tebor opens the shop. A once-only supply pack provides two health potions, one vigor potion, 1 Mucor Mushroom and 100 Tallow. Use Crafting to make another potion. Lifestones bind your return; portals lead outside and back.", 1).map_err(other)?.into() });
                                        }
                                        eprintln!(
                                            "Scene ready for character {character}: cell={:08x}, visible content={}",
                                            scene
                                                .player_placement(character)
                                                .map_or(0, |point| point.cell),
                                            scene.visible_content_count(character).unwrap_or(0)
                                        );
                                        if first
                                            && !store.characters[&character].progression.alive()
                                        {
                                            for wire in [
                                                crate::ui::death_notice("You have fallen.")
                                                    .map_err(other)?,
                                                crate::ui::lock_player(true).map_err(other)?,
                                            ] {
                                                deliveries.push(crate::scene::Delivery {
                                                    recipient: character,
                                                    data: wire.into(),
                                                });
                                            }
                                            deliveries.extend(player_body_alive(peer, &mut scene, character, store.characters[&character].instance, false));
                                        }
                                    }
                                    eprintln!("client reported SceneRenderingComplete");
                                    return Ok(replies);
                                }
                            };
                            replies.extend(peer.send_event(&response, now)?);
                            Ok(replies)
                        })();
                        match application {
                            Ok(events) => replies.extend(events),
                            Err(error) => {
                                counters.rejected += 1;
                                let reason: String = error.to_string().chars().take(512).collect();
                                if diagnosed_application_errors.len() < 32
                                    && diagnosed_application_errors.insert(reason)
                                {
                                    eprintln!("application event rejected: {error}");
                                }
                            }
                        }
                    }
                    Ok(replies)
                })();
                for packet in recovery_packets {
                    primary.send_to(&packet, addr)?;
                }
                match result {
                    Ok(replies) => {
                        for reply in replies {
                            primary.send_to(&reply, addr)?;
                        }
                    }
                    Err(error) => {
                        counters.rejected += 1;
                        let reason: String = error.to_string().chars().take(512).collect();
                        if diagnosed_packet_errors.len() < 32
                            && diagnosed_packet_errors.insert(reason)
                        {
                            eprintln!(
                                "packet rejected: flags={:#x} sequence={} bytes={} reason={error} state={}",
                                h.flags,
                                h.sequence,
                                len,
                                peers
                                    .get(&addr)
                                    .map(|peer| peer.transport.diagnostic())
                                    .unwrap_or_else(|| "no peer".into())
                            );
                        }
                    }
                }
                for (character, request) in group_requests {
                    let online = group_online(&active_characters, &store, &scene);
                    // A refused request answers with the client's own line (`GroupError::wire`).
                    let result = (|| -> io::Result<Result<(), crate::groups::GroupError>> {
                        let plan = match groups.prepare(character, &request, now_ms, &online) {
                            Ok(plan) => plan,
                            Err(refused) => return Ok(Err(refused)),
                        };
                        let packets = group_admit(plan.deliveries(), &active_characters, &peers)?;
                        groups.commit(plan).map_err(io::Error::other)?;
                        group_publish(packets, &mut peers)?;
                        Ok(Ok(()))
                    })();
                    let reply = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(refused)) => Some(refused.wire().map_err(other)?),
                        Err(error) => Some(script::chat(&error.to_string(), 1).map_err(other)?),
                    };
                    if let Some(reply) = reply {
                        if let Some(peer) = active_characters
                            .get(&character)
                            .and_then(|addr| peers.get_mut(addr))
                        {
                            if peer.queue_ordered_batch(vec![reply]).is_err() {
                                failed_peers.insert(addr);
                            }
                        }
                    }
                }
                deliver_scene(
                    deliveries,
                    &active_characters,
                    &mut peers,
                    &primary,
                    now,
                    &mut failed_peers,
                )?;
            }
        }
        if let Some(admin) = &admin {
            admin.players.store(active_characters.len(), std::sync::atomic::Ordering::Relaxed);
            // Live edits arrive prepared; applying one costs microseconds per object.
            while let Ok(job) = admin.jobs.try_recv() {
                let started = Instant::now();
                let reply = match crate::live_edit::apply(&job.change, &mut scene, encounters.as_mut(), &mut corpse_loot, now_ms) {
                    Ok((deliveries, mut summary)) => {
                        summary.apply_us = started.elapsed().as_micros() as u64;
                        deliver_scene(deliveries, &active_characters, &mut peers, &primary, now, &mut failed_peers)?;
                        println!("Live edit: +{} -{} ~{} objects, {} rule targets, applied in {} µs.",
                            summary.added, summary.removed, summary.replaced, summary.rules, summary.apply_us);
                        Ok(summary)
                    }
                    Err(error) => Err(error.to_string()),
                };
                let _ = job.reply.send(reply);
            }
        }
        if now_ms>=next_resource_cleanup{
            next_resource_cleanup=now_ms+10_000;let time=crate::recovered_quests::now();
            let live:std::collections::BTreeSet<_>=store.characters.values().filter(|c|!c.deleted).flat_map(|c|c.professions.1.jobs.iter()).filter(|(_,j)|j.point.is_some()&&j.expires>time&&j.remaining>0).map(|(id,_)|crate::harvesting::node_id(*id)).collect();
            let expired:Vec<_>=scene.content_objects().filter(|o|o.id>>48==0x8190&&!live.contains(&o.id)).map(|o|o.id).collect();
            for id in expired{let updates=scene.remove_content_live(id)?;deliver_scene(updates,&active_characters,&mut peers,&primary,now,&mut failed_peers)?;}
        }
        let chest_updates=scene.expire_chest_views(now_ms,|id|store.characters.get(&id).is_some_and(|c|!c.deleted && c.progression.alive()))?;
        deliver_scene(chest_updates,&active_characters,&mut peers,&primary,now,&mut failed_peers)?;
        if let Some(runtime) = &mut encounters {
            runtime.set_shared_xp(groups.sharing_players());
            let events = runtime.tick(
                now_ms,
                &scene,
                &mut store,
                active_characters.keys().copied(),
                bootstrap.as_mut().and_then(|b|b.geometry.as_mut()),
            )?;
            let mut updates = Vec::new();
            for event in events {
                match event {
                    crate::encounters::Event::IdolVisual(actor, action) => {
                        match scene.idol_presentation(actor, action) {
                            Ok(deliveries) => updates.extend(deliveries),
                            Err(error) => {
                                let reason = format!("Creature presentation {actor:#x}: {error}");
                                if diagnosed_application_errors.len() < 32
                                    && diagnosed_application_errors.insert(reason.clone())
                                {
                                    eprintln!("{reason}");
                                }
                            }
                        }
                    }
                    crate::encounters::Event::Hit {
                        victim,
                        attacker,
                        health_change,
                    } => {
                        if store.characters.contains_key(&victim) {
                            if let Some(peer) = active_characters
                                .get(&victim)
                                .and_then(|addr| peers.get_mut(addr))
                            {
                                updates.extend(publish_health_story(
                                    peer,
                                    &mut scene,
                                    victim,
                                    store.characters[&victim].instance,
                                    attacker,
                                    health_change,
                                ));
                            }
                        } else {
                            if let Some((_,fx))=store.characters.get(&attacker)
                                .filter(|c|crate::starter_gear::equipped_category(&c.inventory)==2)
                                .and_then(|c|c.creation.as_ref()).and_then(|r|crate::combat_visual::magic_effects(r.race)) {
                                updates.extend(scene.content_effect(victim,fx)?);
                            }
                            match scene.content_damage_story(victim, attacker, health_change) {
                                Ok(deliveries) => updates.extend(deliveries),
                                Err(error) => {
                                    let reason = format!("Damage story {victim:#x}: {error}");
                                    if diagnosed_application_errors.len() < 32
                                        && diagnosed_application_errors.insert(reason.clone())
                                    {
                                        eprintln!("{reason}");
                                    }
                                }
                            }
                        }
                    }
                    crate::encounters::Event::Move {actor,placement} => {
                        // Native PositionPack time shares the synchronized world clock;
                        // pursuit scheduling above remains on process monotonic time.
                        updates.extend(scene.move_creature(actor,placement,(world_clock.seconds()*1000.0) as u64)?);
                    }
                    crate::encounters::Event::Warp {actor,placement} => {
                        updates.extend(scene.warp_creature(actor,placement,(world_clock.seconds()*1000.0) as u64)?);
                    }
                    crate::encounters::Event::Projectile { player, target, profile, flight_ms } => {
                        // A saturated cosmetic pool must never terminate the world server.
                        match projectiles.fire(&scene, player, target, profile, flight_ms, now_ms) {
                            Ok(deliveries) => updates.extend(deliveries),
                            Err(error) => {
                                let reason=format!("Projectile presentation: {error}");
                                if diagnosed_application_errors.len()<32 && diagnosed_application_errors.insert(reason.clone()) {
                                    eprintln!("{reason}");
                                }
                            }
                        }
                    }
                    crate::encounters::Event::SkillSwing{player,behavior,skill,target} => {
                        if let Some(peer)=active_characters.get(&player).and_then(|addr|peers.get_mut(addr)) {
                            if let Some(request)=peer.pending_skill.take() {
                                updates.push(crate::scene::Delivery{recipient:player,data:script::skill_finished(request).map_err(other)?.into()});
                            }
                            let instance=store.characters[&player].instance;
                            if let Some(hook)=crate::skill_missiles::missile_hook(skill).filter(|_|behavior!=0) {
                                updates.extend(skill_missile_visual(peer,&mut scene,player,instance,behavior,skill,target,hook));
                            }
                            else if behavior!=0 { updates.extend(skill_visual(peer,&mut scene,player,instance,behavior)); }
                            else {updates.extend(training_combat_visual(peer,player,&store,&mut scene,true,true,true)?);}
                        }
                    }
                    crate::encounters::Event::Fx{target,fx} => {
                        if let Some(peer)=active_characters.get(&target).and_then(|addr|peers.get_mut(addr)) {
                            updates.extend(player_fx(peer,&mut scene,target,store.characters[&target].instance,fx));
                        } else if let Ok(deliveries)=scene.content_effect(target,fx) {updates.extend(deliveries);}
                    }
                    crate::encounters::Event::Swing(player) => {
                        if let Some(addr) = active_characters.get(&player) {
                            if let Some(peer) = peers.get_mut(addr) {
                                let casting=peer.pending_skill.is_some();
                                if let Some(request_id) = peer.pending_skill.take() {
                                    updates.push(crate::scene::Delivery {
                                        recipient: player,
                                        data: script::skill_finished(request_id)
                                            .map_err(other)?
                                            .into(),
                                    });
                                }
                                match training_combat_visual(
                                    peer, player, &store, &mut scene, true, true, casting,
                                ) {
                                    Ok(deliveries) => updates.extend(deliveries),
                                    Err(error) => {
                                        eprintln!(
                                            "Combat presentation failed for character {player}: {error}"
                                        );
                                        failed_peers.insert(*addr);
                                    }
                                }
                            }
                        }
                    }
                    crate::encounters::Event::Loot {actor,player,entity,ai}=>{
                        let saved=&store.characters[&player];
                        let hammer=entity==crate::arwic_mines::OUTCAST && !ai.loot_proof
                            && scene.examination_content(player,actor).is_some_and(|o|o.placement.cell>>16==0x7cb9 && o.placement.cell&65535>=256)
                            && saved.quests.arwic.can_loot(&saved.inventory);
                        let quest_item=if hammer {Some(crate::arwic_mines::item(crate::arwic_mines::HAMMER,1,0,1)?)}else{None};
                        let mut count=corpse_loot.add_with_quest(actor,player,now_ms,entity,ai,quest_item);
                        if count==0 && std::env::var("PYREALIUM_TEST_CORPSE_LOOT").as_deref()==Ok("1") {count=corpse_loot.add_test_potions(actor,player,now_ms,2);}
                        if count>0 {updates.push(crate::scene::Delivery{recipient:player,data:script::chat("Loot remains on the defeated creature. Double-click its body to open it.",1).map_err(other)?.into()});}
                    }
                    crate::encounters::Event::Item {
                        player,
                        item,
                        instance,
                    } => {
                        let metadata = inventory_metadata
                            .get(&item.definition)
                            .and_then(|entry| *entry)
                            .ok_or_else(|| {
                                invalid("Encounter item metadata was not initialized")
                            })?;
                        let spawn =
                            world::create_inventory_item(&item, player, instance).map_err(other)?;
                        let acquired = crate::item_services::acquired(player, &item, metadata)
                            .map_err(other)?;
                        scene
                            .inventory_granted(player, item.id, instance)
                            .map_err(other)?;
                        updates.push(crate::scene::Delivery {
                            recipient: player,
                            data: spawn.into(),
                        });
                        updates.push(crate::scene::Delivery {
                            recipient: player,
                            data: acquired.into(),
                        });
                        updates.push(crate::scene::Delivery {
                            recipient: player,
                            data: script::chat(
                                match item.definition {crate::shelter::TOOL=>"Collected Imbued Digging Tool: return to the Shelter Scout.",crate::shelter::GLYPH=>"Collected Menhir Glyph: target the Menhir Portal and use the glyph.",crate::item_services::POTION_DEFINITION=>"Training reward: a health potion was placed in your bag.",_=>"Collected equipment: open your backpack to inspect and equip it."},
                                1,
                            )
                            .map_err(other)?
                            .into(),
                        });
                    }
                    crate::encounters::Event::EscortLost { player, entity } => {
                        // The escort falls in a fight, so this is not an item service (those wait for
                        // a player out of combat): only quest progress, lines and tells, as a
                        // fellow's kill credit is applied.
                        let unix = crate::recovered_quests::now();
                        if let Some((tables, c)) = crate::quest_emotes::tables().zip(store.characters.get(&player).filter(|c| !c.deleted)) {
                            if let Some(outcome) = tables.run(crate::quest_emotes::Trigger::EscortLost, entity, 0, None, &c.quests, &c.inventory,
                                &c.progression, 0, 0., unix, &crate::quest_emotes::item).ok().flatten()
                                .filter(|o| o.items.is_empty() && o.changes.is_empty())
                            {
                                let previous = c.quests.clone();
                                let account = c.account.clone();
                                if store.emote_transaction(&account, player, &outcome).is_ok() {
                                    for line in &outcome.lines {
                                        updates.push(crate::scene::Delivery { recipient: player, data: script::chat(line, 1).map_err(other)?.into() });
                                    }
                                    for (table, key) in &outcome.tells {
                                        updates.push(crate::scene::Delivery { recipient: player, data: crate::quest_emotes::tell(*table, *key).map_err(other)?.into() });
                                    }
                                    for event in store.characters[&player].quests.updates_since(&previous).map_err(other)? {
                                        updates.push(crate::scene::Delivery { recipient: player, data: event.into() });
                                    }
                                }
                            }
                        }
                    }
                    crate::encounters::Event::Kill { player, entity } => {
                        // A fellowship quest's kill counts for the killer's fellows in the world
                        // (registry credit `fellowship`; Pyrealium policy: no range limit is recovered).
                        let unix = crate::recovered_quests::now();
                        for fellow in groups.members(player).into_iter().filter(|f| *f != player && active_characters.contains_key(f)) {
                            let Some(c) = store.characters.get(&fellow).filter(|c| !c.deleted && c.progression.alive()) else { continue };
                            let Some(outcome) = crate::quest_emotes::fellow_kill(&c.quests, &c.inventory, &c.progression, entity, unix) else { continue };
                            // Only quest progress is shared: items and experience stay the killer's.
                            if !outcome.items.is_empty() || !outcome.changes.is_empty() { continue; }
                            let previous = c.quests.clone();
                            let account = c.account.clone();
                            if store.emote_transaction(&account, fellow, &outcome).is_err() { continue; }
                            for (table, key) in &outcome.tells {
                                updates.push(crate::scene::Delivery { recipient: fellow, data: crate::quest_emotes::tell(*table, *key).map_err(other)?.into() });
                            }
                            for event in store.characters[&fellow].quests.updates_since(&previous).map_err(other)? {
                                updates.push(crate::scene::Delivery { recipient: fellow, data: event.into() });
                            }
                        }
                    }
                    crate::encounters::Event::KillXp { player, xp } => {
                        updates.extend(share_kill_xp(player, xp, &groups, &mut store, &mut scene, &active_characters, config.initialize_ui)?);
                    }
                    crate::encounters::Event::Quest { player, previous, book } => {
                        for event in book.updates_since(&previous).map_err(other)? {
                            updates.push(crate::scene::Delivery {recipient:player,data:event.into()});
                        }
                        if book.ready() {
                            updates.push(crate::scene::Delivery {recipient:player,data:script::chat(&book.guidance(),1).map_err(other)?.into()});
                        }
                    }
                    crate::encounters::Event::Vigor { actor, vigor } => {
                        updates.extend(scene.content_stat(actor, 0x103, vigor)?);
                    }
                    crate::encounters::Event::Health { actor, health } => {
                        updates.extend(scene.content_stat(actor, 0x102, health)?);
                        if health==0 {
                            // The body is the creature at zero health. A tool is used on a target
                            // whose `Usage_TargetType` (int 721) shares a bit with the tool's
                            // `Usage_ValidTargetTypes` (722; the hunting knives' DAT value is 8,
                            // `SkillValidTargets::MonsterItem`), so the body carries that bit and a
                            // double-clicked knife can be used on it by a click.
                            updates.extend(scene.content_stat(actor, crate::butchery::TARGET_TYPE_QUALITY, crate::butchery::BUTCHERABLE_TARGET)?);
                            updates.extend(scene.corpse_name(actor,&examination_names)?);
                        }
                        for (&recipient, addr) in &active_characters {
                            if peers
                                .get(addr)
                                .is_some_and(|peer| peer.target == Some(actor))
                                && scene.can_observe(recipient, actor)
                            {
                                updates.push(crate::scene::Delivery {
                                    recipient,
                                    data: selection_status(recipient, actor, &scene, &store)?
                                        .into(),
                                });
                            }
                        }
                    }
                    crate::encounters::Event::Stats { actor, max_health, health, level } => {
                        updates.extend(scene.content_stat(actor, 0x107, max_health)?);
                        updates.extend(scene.content_stat(actor, 0x102, health)?);
                        updates.extend(scene.content_stat(actor, 0x12e, level.unwrap_or(0))?);
                    }
                    crate::encounters::Event::Despawn(actor) => {
                        if scene.content_placement(actor).is_some(){updates.extend(scene.despawn_content(actor)?);}
                        runtime.removed(actor,now_ms);
                    }
                    crate::encounters::Event::Respawn { actor, health, vigor } => {
                        corpse_loot.remove(actor);
                        scene.content_stat(actor, 0x102, health)?;
                        if let Some(max_vigor) = vigor {
                            scene.content_stat(actor, 0x103, max_vigor)?;
                        }
                        updates.extend(scene.respawn_content(actor)?);
                        runtime.spawned(actor)?;
                    }
                    crate::encounters::Event::Owner {
                        player,
                        previous,
                        state,
                    } => {
                        updates.extend(scene.player_stats(player, &state));
                        let saved = &store.characters[&player];
                        let skills = &saved.skills;
                        if config.initialize_ui
                            && skills.available_credits(&previous)
                                != skills.available_credits(&state)
                        {
                            updates.push(crate::scene::Delivery {
                                recipient: player,
                                data: crate::ui::skills_state(skills, &state, &saved.skill_used)
                                    .map_err(other)?
                                    .into(),
                            });
                        }
                        for wire in state.private_changes(&previous) {
                            updates.push(crate::scene::Delivery {
                                recipient: player,
                                data: wire.into(),
                            });
                        }
                        if previous.alive() && !state.alive() {
                            for wire in [
                                crate::ui::death_notice("You were defeated by a creature.")
                                    .map_err(other)?,
                                crate::ui::lock_player(true).map_err(other)?,
                            ] {
                                updates.push(crate::scene::Delivery {
                                    recipient: player,
                                    data: wire.into(),
                                });
                            }
                            if let Some(peer) = active_characters.get(&player).and_then(|addr| peers.get_mut(addr)) {
                                updates.extend(player_body_alive(peer, &mut scene, player, store.characters[&player].instance, false));
                            }
                        }
                    }
                    crate::encounters::Event::Note { player, text } => {
                        updates.push(crate::scene::Delivery {
                            recipient: player,
                            data: script::chat(&text, 1).map_err(other)?.into(),
                        })
                    }
                    crate::encounters::Event::EffectAdded { owner, effect } => {
                        // Only players have a list on their client; a creature's is shown in its
                        // examination.
                        if store.characters.contains_key(&owner) {
                            let cast = world_clock.seconds() - now_ms.saturating_sub(effect.cast_ms) as f64 / 1000.0;
                            updates.push(crate::scene::Delivery {
                                recipient: owner,
                                data: crate::ui::effect_added(&crate::ui::EffectView {
                                    eid: effect.eid,
                                    did: effect.did,
                                    time_cast: cast,
                                    duration: effect.duration_ms as f32 / 1000.0,
                                    power: effect.power,
                                    pulsed: effect.pulsed,
                                })
                                .map_err(other)?
                                .into(),
                            });
                        }
                    }
                    crate::encounters::Event::EffectRemoved { owner, eid, expired: _ } => {
                        if store.characters.contains_key(&owner) && active_characters.contains_key(&owner) {
                            updates.push(crate::scene::Delivery {
                                recipient: owner,
                                data: crate::ui::effect_removed(eid).map_err(other)?.into(),
                            });
                        }
                    }
                    crate::encounters::Event::Stop(player) => {
                        if let Some(peer) = active_characters
                            .get(&player)
                            .and_then(|addr| peers.get_mut(addr))
                        {
                            if !store.characters[&player].progression.alive(){peer.combat=false;}
                            if let Some(request_id) = peer.pending_skill.take() {
                                updates.push(crate::scene::Delivery {
                                    recipient: player,
                                    data: script::skill_finished(request_id).map_err(other)?.into(),
                                });
                            }
                            match training_combat_visual(
                                peer, player, &store, &mut scene, peer.combat, false, false,
                            ) {
                                Ok(deliveries) => updates.extend(deliveries),
                                Err(_) => {
                                    if let Some(addr) = active_characters.get(&player) {
                                        failed_peers.insert(*addr);
                                    }
                                }
                            }
                        }
                        let combat=active_characters.get(&player).and_then(|addr|peers.get(addr)).is_some_and(|p|p.combat);
                        updates.push(crate::scene::Delivery {recipient:player,
                            data:script::private_call(script::COMBAT_STATE,&script::integer(u32::from(combat))).map_err(other)?.into()});
                    }
                }
            }
            deliver_scene(
                updates,
                &active_characters,
                &mut peers,
                &primary,
                now,
                &mut failed_peers,
            )?;
        }
        deliver_scene(projectiles.retire(now_ms), &active_characters, &mut peers, &primary, now, &mut failed_peers)?;
        if now_ms >= next_untraining_tick {
            next_untraining_tick = now_ms + 250;
            let mut updates=Vec::new();
            for &character in active_characters.keys() {
                if store.debit_untraining(character)? {
                    let saved=&store.characters[&character];
                    updates.push(crate::scene::Delivery {recipient:character,
                        data:crate::ui::skills_state(&saved.skills,&saved.progression,&saved.skill_used).map_err(other)?.into()});
                    for wire in saved.progression.private_updates() {
                        if wire[..4]==0x100d4u32.to_le_bytes() {
                            updates.push(crate::scene::Delivery {recipient:character,data:wire.into()});
                        }
                    }
                }
            }
            deliver_scene(updates,&active_characters,&mut peers,&primary,now,&mut failed_peers)?;
        }
        let mut remove = std::mem::take(&mut failed_peers);
        for (addr, peer) in &mut peers {
            if peer.native_shop.is_some() {
                let valid = match peer.session.phase {
                    Phase::InWorld { character } => {
                        store
                            .characters
                            .get(&character)
                            .is_some_and(|saved| saved.progression.alive())
                            && !peer.combat
                            && peer
                                .merchant_open_ms
                                .is_some_and(|last| now_ms.saturating_sub(last) <= 30_000)
                            && merchant_ready(character, &scene, peer.native_shop.as_ref().map(|shop| shop.merchant())).ok()
                                == peer.native_shop.as_ref().map(|shop| shop.merchant())
                    }
                    _ => false,
                };
                if !valid {
                    let merchant = peer.native_shop.take().unwrap().merchant();
                    peer.merchant_open_ms = None;
                    if peer
                        .queue_ordered_batch(
                            vec![crate::merchants::close(merchant).map_err(other)?],
                        )
                        .is_err()
                    {
                        remove.insert(*addr);
                    }
                }
            }
            if peer.session.expired(now_ms) {
                remove.insert(*addr);
                continue;
            }
            if peer.session.phase != Phase::Connecting && now_ms.saturating_sub(peer.last_time_sync_ms) >= 5000 &&
                peer.transport.send_available(8).map_err(other)? {
                primary.send_to(&peer.transport.send_time_sync(world_clock.seconds(), now).map_err(other)?, addr)?;
                peer.last_time_sync_ms = now_ms;
            }
            match peer.transport.retransmit_due(now) {
                Ok(packets) => {
                    for packet in packets {
                        primary.send_to(&packet, addr)?;
                    }
                }
                Err(_) => {
                    remove.insert(*addr);
                }
            }
            if !remove.contains(addr) && peer.flush_scene(&primary, addr, now).is_err() {
                remove.insert(*addr);
            }
        }
        let mut departures = Vec::new();
        for addr in remove {
            if let Some(peer) = peers.remove(&addr) {
                if let Phase::Entering { character } | Phase::InWorld { character } =
                    peer.session.phase
                {
                    if active_characters.get(&character) == Some(&addr) {
                        active_characters.remove(&character);
                        if let Some(runtime) = &mut encounters {
                            runtime.stop(character);
                        }
                        departures.extend(scene.remove(character));
                    }
                }
            }
        }
        let online = group_online(&active_characters, &store, &scene);
        // Invitations whose time ran out close the recruit's dialog (`groups::EXPIRED`).
        match groups.prepare_expiry(now_ms, &online) {
            Ok(Some(plan)) => {
                let result = group_admit(plan.deliveries(), &active_characters, &peers)
                    .and_then(|packets| {
                        groups.commit(plan).map_err(io::Error::other)?;
                        group_publish(packets, &mut peers)
                    });
                if let Err(error) = result {
                    eprintln!("Fellowship invitation expiry deferred: {error}");
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("Fellowship invitation expiry deferred: {error}"),
        }
        for player in groups
            .participant_ids()
            .into_iter()
            .filter(|id| !online.contains_key(id))
        {
            let result = (|| -> io::Result<()> {
                let plan = groups
                    .prepare_disconnect(player, now_ms, &online)
                    .map_err(io::Error::other)?;
                let packets = group_admit(plan.deliveries(), &active_characters, &peers)?;
                groups.commit(plan).map_err(io::Error::other)?;
                group_publish(packets, &mut peers)
            })();
            if let Err(error) = result {
                eprintln!("Group departure deferred: {error}");
            }
        }
        if now_ms >= next_group_refresh {
            let mut refreshed = BTreeSet::new();
            for player in groups.member_ids() {
                if refreshed.contains(&player) {
                    continue;
                }
                let members = groups.members(player);
                let snapshot = members.iter().any(|id| {
                    online.get(id).is_none_or(|member| {
                        group_cache
                            .get(id)
                            .is_none_or(|old| old.name != member.name)
                    })
                });
                if !snapshot
                    && members
                        .iter()
                        .all(|id| group_cache.get(id) == online.get(id))
                {
                    refreshed.extend(members);
                    continue;
                }
                let result = (|| -> io::Result<()> {
                    let updates = if snapshot {
                        groups.refresh(player, &online).map_err(io::Error::other)?
                    } else {
                        let mut updates = Vec::new();
                        for id in &members {
                            let member = online
                                .get(id)
                                .ok_or_else(|| invalid("Group member is unavailable."))?;
                            updates.extend(
                                groups
                                    .member_changes(*id, member, group_cache.get(id))
                                    .map_err(io::Error::other)?,
                            );
                        }
                        updates
                    };
                    let packets = group_admit(&updates, &active_characters, &peers)?;
                    group_publish(packets, &mut peers)
                })();
                if result.is_ok() {
                    for id in members {
                        refreshed.insert(id);
                        if let Some(m) = online.get(&id) {
                            group_cache.insert(id, m.clone());
                        }
                    }
                }
            }
            group_cache.retain(|id, _| groups.contains(*id));
            next_group_refresh = now_ms.saturating_add(1000);
        }
        deliver_scene(
            departures,
            &active_characters,
            &mut peers,
            &primary,
            now,
            &mut failed_peers,
        )?;
        if now_ms >= next_checkpoint {
            store.save_due()?;
            next_checkpoint = now_ms + 5000;
        }
        if now_ms >= next_report {
            println!(
                "sessions={} received={} rejected={} logins={} connected={} events={}",
                peers.len(),
                counters.received,
                counters.rejected,
                counters.logins,
                counters.connected,
                counters.events
            );
            next_report = now_ms + 10_000;
        }
        if now_ms >= next_gun_publish {
            next_gun_publish = now_ms + 500;
            // Friends see a character in the world as logged in and in game.
            crate::gun::publish_online(active_characters.keys().filter_map(|id| {
                store
                    .characters
                    .get(id)
                    .map(|character| (character.name.clone(), character.account.clone()))
            }));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    store.save_now()?;
    println!(
        "Stopped: received={} rejected={} logins={} connected={} events={}",
        counters.received, counters.rejected, counters.logins, counters.connected, counters.events
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_player_cannot_advance_dialogue_or_queue_speech() {
        let path = crate::db::testdb::fresh();
        let mut store=Store::open(&path).unwrap();
        let character=store.create("local","Fallen Reader").unwrap().id;
        store.progression_command("local",character,crate::progression::Command::Damage(100000)).unwrap();
        let mut peer=peer();
        let scene=crate::scene::Scene::with_content(crate::content::Content::default());
        let error=use_npc_dialogue(&mut peer,character,10,&store,&scene,1000).unwrap_err();
        assert!(error.to_string().contains("alive"));
        assert_eq!(peer.npc_dialogue.page(character,10),0);
        assert!(peer.scene_outbox.is_empty() && peer.last_npc_dialogue_ms.is_none());
        drop(store);
    }

    #[test]
    fn selected_creature_vitals_use_current_scene_values() {
        let path = crate::db::testdb::fresh();
        let store=Store::open(&path).unwrap();
        let target=crate::content::NAMESPACE+7;
        let point=Placement::HUMAN_START;
        let mut scene=crate::scene::Scene::with_content(crate::content::Content::fixture(target,point,None));
        let bytes=world::create_stationary_object(&world::StationaryObject {
            character_id:1,parent_visual_did:1,entity_did:2,cell_id:point.cell,
            position:point.coordinates,orientation:point.orientation(),instance_stamp:1,
            visual_ordering_stamp:0,timestamps:[0;4]
        }).unwrap();
        scene.enter(crate::scene::Body::new(1,point,bytes).unwrap()).unwrap();
        for (key,value) in [(0x102,100),(0x107,100),(0x103,30),(0x108,164)] {
            scene.content_stat(target,key,value).unwrap();
        }
        assert_eq!(selection_status(1,target,&scene,&store).unwrap(),crate::ui::selection(target,Some([100,100,30,164])).unwrap());
        scene.content_stat(target,0x103,28).unwrap();
        assert_eq!(selection_status(1,target,&scene,&store).unwrap(),crate::ui::selection(target,Some([100,100,28,164])).unwrap());
        drop(store);
    }

    fn peer() -> Peer {
        Peer {
            client_version: "7148".into(),
            session: Session::new("local".into(), 1, 0),
            transport: Transport::new(1, 2, 1, 1),
            connect_packet: Vec::new(),
            login_proof: String::new(),
            id: 1,
            iteration: 1,
            blob: 0,
            selection_pending: None,
            ordered_stamp: 0,
            motion: None,
            target: None,
            combat: false,
            pending_skill: None,
            last_chat_ms: None,
            emotes: crate::emotes::State::default(),
            last_inventory_ms: None,
            last_examine_ms: None,
            last_world_use_ms: None,
            trade: Default::default(),
            deposits: BTreeMap::new(),
                                last_npc_dialogue_ms: None,
                                guide_page: 0,
                                npc_dialogue: Default::default(),
            merchant_open_ms: None,
            last_shop_refresh_ms: None,
            native_shop: None,
            unhandled_scripts: BTreeSet::new(),
            scene_outbox: VecDeque::new(),
            scene_bytes: 0,
            last_time_sync_ms: 0,
        }
    }

    #[test]
    fn rejected_sends_do_not_consume_blob_or_world_order_stamps() {
        let now = Instant::now();
        let mut peer = peer();
        for _ in 0..256 {
            peer.send_world_event(b"fill", false, now).unwrap();
        }
        assert_eq!((peer.blob, peer.ordered_stamp), (256, 0));
        assert!(peer.send_world_event(b"ordered", true, now).is_err());
        assert!(peer.send_to_queue(1, b"selection", now).is_err());
        assert_eq!((peer.blob, peer.ordered_stamp), (256, 0));
        peer.transport.acknowledge(257).unwrap();
        let packets = peer.send_world_event(b"ordered", true, now).unwrap();
        let fragment = wire::Fragment::decode(&packets[0][wire::HEADER_LEN..]).unwrap();
        assert_eq!(fragment.blob_id, (0x0100_0001u64 << 32) | 257);
        assert_eq!(fragment.data, b"ordered");
        assert_eq!((peer.blob, peer.ordered_stamp), (257, 1));
        peer.send_to_queue(1, b"selection", now).unwrap();
        assert_eq!((peer.blob, peer.ordered_stamp), (258, 1));

        // Invalid payloads leave both counters intact. World ordering is a
        // wrapping ushort; successful sends continue through zero.
        assert!(peer.send_world_event(b"", true, now).is_err());
        assert!(peer.send_to_queue(1, b"", now).is_err());
        assert_eq!((peer.blob, peer.ordered_stamp), (258, 1));
        peer.ordered_stamp = u16::MAX;
        assert!(peer.send_world_event(b"wrapped", true, now).is_ok());
        assert_eq!((peer.blob, peer.ordered_stamp), (259, 0));
        peer.blob = u16::MAX.into();
        assert!(peer.send_to_queue(1, b"selection after wrap", now).is_ok());
        assert_eq!(peer.blob, 65536);
        peer.blob = u32::MAX;
        assert!(peer.send_to_queue(1, b"exhausted", now).is_err());
        assert_eq!(peer.blob, u32::MAX);
    }
}
