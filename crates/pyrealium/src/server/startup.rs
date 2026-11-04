//! Server start: checks the configuration, loads the data and content, opens the store and
//! the sockets, and builds the running state (`Server`) the loop serves.
use super::*;

pub(super) fn run_auth_mode(config: Config, tickets: bool, local_accounts: bool) -> io::Result<()> {
    println!("{}", crate::licensing::notice());
    std::fs::create_dir_all(&config.data)?;
    // The game's own connection for spending login tickets (the account service issues them).
    let auth_data = if tickets {
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
    let map_landscape = if config.map_travel {
        println!("Local testing: double-click map travel enabled.");
        Some(crate::map_travel::Landscape::load(config.game_data.as_deref().unwrap())?)
    } else { None };
    let bootstrap = config
        .game_data
        .as_deref()
        .map(crate::bootstrap::Bootstrap::load)
        .transpose()?;
    // Replaying trained skills can initialize the process-wide skill catalog.
    // Select the native data profile before opening the character store.
    // The scripted quests' tables (`quest_emotes`), before the journal replays; a bad row refuses
    // the start. The shipped database (prepared, or `quests.sql` beside the world table), unless
    // the world builder's overlay has its own beside it (`NAME.quests.sql`, saved by live edits),
    // which replaces it whole.
    let shipped_quests = if let Some(prepared) = &config.prepared {
        let text = prepared.quest_sql().ok_or_else(|| invalid("the prepared content has no quest database (quests.sql)"))?;
        Some((text.to_owned(), "the prepared content".to_owned()))
    } else if let Some(world) = config.content.as_deref().filter(|p| p.is_file()) {
        let directory = world.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        let path = directory.join(crate::quest_emotes::db::DUMP);
        Some((std::fs::read_to_string(&path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?, path.display().to_string()))
    } else {
        None
    };
    // The world builder's quest edits (`NAME.quests.sql`, saved by live edits) are a layer: with
    // the shipped dump they were edited from (`NAME.quests.base.sql`), they are merged row by row
    // over the shipped dump as it is now (`quest_emotes::db::merge`), so shipped changes to rows
    // the edits left alone still arrive.
    let quests_sidecar = config.content_overlay.as_deref().map(|path| path.with_extension("quests.sql"));
    let quests_base = config.content_overlay.as_deref().map(|path| path.with_extension("quests.base.sql"));
    let shipped_canonical = match &shipped_quests {
        Some((text, origin)) => crate::quest_emotes::db::canonical(text).map_err(|e| io::Error::other(format!("{origin}: {e}")))?,
        None => String::new(),
    };
    let read = |path: &std::path::Path| std::fs::read_to_string(path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())));
    let overlay_quests = match quests_sidecar.as_deref().filter(|p| p.is_file()) {
        None => None,
        Some(path) => {
            let overlay = read(path)?;
            let origin = path.display().to_string();
            match quests_base.as_deref().filter(|p| p.is_file()) {
                Some(base) => {
                    let merged = crate::quest_emotes::db::merge(&read(base)?, &shipped_canonical, &overlay)
                        .map_err(|e| io::Error::other(format!("{origin} over the shipped quests: {e}")))?;
                    if !merged.conflicts.is_empty() {
                        println!("World-builder quests {origin} and the shipped quests both changed {} rows; the world builder's are kept: {}.",
                            merged.conflicts.len(), merged.conflicts.join(", "));
                    }
                    Some((merged.text, format!("{origin} over the shipped quests")))
                }
                None => {
                    // Saved before edits were layers: it replaces the shipped quests whole.
                    let (overlay_tables, shipped_tables) = (crate::quest_emotes::Tables::from_sql(&overlay)?, crate::quest_emotes::Tables::from_sql(&shipped_canonical)?);
                    let missing: Vec<&str> = shipped_tables.quest_names().filter(|q| overlay_tables.quest(q).is_none()).collect();
                    if !missing.is_empty() {
                        println!("World-builder quests {origin} have no base file and replace the shipped quest tables whole, leaving out {} of their quests: {}.",
                            missing.len(), missing.join(", "));
                    }
                    Some((overlay, origin))
                }
            }
        }
    };
    let quest_text = overlay_quests.or(shipped_quests);
    if let Some((text, origin)) = &quest_text {
        let tables = crate::quest_emotes::Tables::from_sql(text).map_err(|e| io::Error::other(format!("{origin}: {e}")))?;
        // Each item a set grants needs a profile: the Shelter's pinned five, or one read from the
        // DATs with the inventory metadata (`quest_emotes::install_items`, below), which refuses
        // the start on an item it cannot read.
        crate::quest_emotes::check_runnable(&tables).map_err(|e| invalid(&e.to_string()))?;
        println!("Quest tables: {} quests from {origin}.", tables.quests().count());
        crate::quest_emotes::install(tables)?;
    }
    let mut store = Store::open(&config.database)?;
    store.set_save_interval(config.save_interval);
    // The hunts the AC2 Vault rows replace (`hunting_quests::retired`; `check_runnable` made sure
    // their rows are loaded): each character's state of them moves onto those rows' quests.
    if crate::quest_emotes::tables().is_some() {
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
                for (definition,metadata) in crate::quest_emotes::install_items(&tables,&mut portal)? {inventory_metadata.entry(definition).or_insert(Some(metadata));}
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
    // An empty overlay (live edits saved before any placement) adds nothing, as the editor reads it.
    let empty = |path: &std::path::Path| std::fs::read_to_string(path).is_ok_and(|text| text.trim().is_empty());
    if let Some(path) = config.content_overlay.as_deref().filter(|path| path.exists() && !empty(path)) {
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
    let projectiles = crate::projectiles::Projectiles::default();
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
    let portal_contacts = if portal_routes.is_empty() {
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
    let peers: BTreeMap<SocketAddr, Peer> = BTreeMap::new();
    let active_characters = BTreeMap::new();
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
    let abilities = if starter_equipment {
        let runtime = crate::abilities::Runtime::load().map_err(io::Error::other)?;
        println!("Loaded {} executable DAT skill-effect profiles; specialized effects and recall entitlements remain incomplete.", runtime.supported());
        Some(runtime)
    } else {
        None
    };
    let failed_peers = BTreeSet::new();
    let next_offer = 0x8160000000000000u64;
    let groups = crate::groups::Runtime::default();
    let trades = crate::trade::Runtime::default();
    let group_cache: BTreeMap<u64, crate::groups::Member> = BTreeMap::new();
    let next_group_refresh = 0u64;
    let login_limits = crate::auth::RateLimit::new(60);
    let generation = 0u64;
    let counters = Counters::default();
    let buffer = [0u8; 65536];
    let next_report = 10_000;
    let next_untraining_tick = 0;
    let next_stop_check = 0;
    let next_checkpoint = 5000;
    let next_gun_publish = 0;
    let diagnosed_events = BTreeSet::new();
    let diagnosed_application_errors = BTreeSet::new();
    let diagnosed_packet_errors = BTreeSet::new();
    let diagnosed_queues = BTreeSet::new();
    let admin = match (config.admin_bind, config.content_overlay.clone(), sidecar) {
        (Some(bind), Some(world), Some(rules)) => Some(crate::admin::start(bind, &config.data,
            config.game_data.clone().ok_or_else(|| invalid("--admin-bind requires --game-data"))?,
            crate::admin::Paths { world, rules,
                quests: quests_sidecar.clone().ok_or_else(|| invalid("--admin-bind needs --content-overlay"))?,
                quests_base: quests_base.clone().ok_or_else(|| invalid("--admin-bind needs --content-overlay"))? },
            base_objects, quest_text.map(|(text, _)| text), shipped_canonical.clone())?),
        _ => None,
    };
    let next_resource_cleanup=0;
    let srv = Server {
        config,
        local_accounts,
        auth_data,
        map_landscape,
        bootstrap,
        store,
        inventory_metadata,
        merchant_catalog,
        money_coin,
        trade_assets,
        quest_gameplay,
        starter_equipment,
        shelter_gameplay,
        initial_ui,
        projectiles,
        corpse_loot,
        encounters,
        portal_routes,
        portal_contacts,
        examination_names,
        primary,
        secondary,
        start,
        world_clock,
        active_characters,
        scene,
        training_quest_enabled,
        abilities,
        failed_peers,
        next_offer,
        groups,
        trades,
        group_cache,
        next_group_refresh,
        login_limits,
        generation,
        counters,
        next_report,
        next_untraining_tick,
        next_stop_check,
        next_checkpoint,
        next_gun_publish,
        diagnosed_events,
        diagnosed_application_errors,
        diagnosed_packet_errors,
        diagnosed_queues,
        admin,
        next_resource_cleanup,
        next_quest_expiry: 0,
        quest_expiries: BTreeSet::new(),
        now: start,
        now_ms: 0,
    };
    serve(srv, peers, buffer)
}
