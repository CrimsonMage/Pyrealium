use pyrealium::{
    config::{self, Auth, Settings},
    db::Database,
    server::{self, Config},
    store::Store,
};
use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn usage() {
    println!(
        "Pyrealium - Asheron's Call 2 server (build 7230 data)\n\n  pyrealium [run] [--config pyrealium.toml]\n    Start the game server (and its account service) from the settings file.\n  pyrealium init [--config pyrealium.toml]\n    Write a commented settings file to edit before the first start.\n  pyrealium db migrate|status|drop\n    Create/upgrade the PostgreSQL schema, show its version, or drop the database.\n  pyrealium stop [--seconds N]\n    Ask the running server to save and stop.\n  pyrealium accounts\n    Run only the account service (when it is hosted apart from the game server).\n\nEvery command reads the database from --database URL, else PYREALIUM_DATABASE_URL, else the settings file.\n"
    );
    println!("  serve --local-accounts PATH --bind 127.0.0.1:19070 --account local\n    Loopback password accounts plus the existing configured developer profile. Mutually exclusive with --auth-data.");
    println!("  inventory-starter --account NAME --character-id N --data PATH --game-data PATH\n    Offline once-only racial starter gear; retains existing possessions and equipment.");
    println!(
        "  account-web --auth-data PATH --data GAME_STORE --web-bind 127.0.0.1:19400 --public-origin https://accounts.example.com --public-host game.example.com --public-port 19370\n    Account registration and HTTPS-proxied launcher login backend.\n  account-create --auth-data PATH --account NAME\n    Offline owner account provisioning with hidden password prompts. Stop the account backend first.\n  serve-training --auth-data PATH --bind 0.0.0.0:19370 --game-data PATH\n    Public native-ticket mode; rejects developer/no-ticket authentication."
    );
    println!(
        "{}\n  license / --license: display the complete AGPL-3.0 license.\n",
        pyrealium::licensing::notice()
    );
    println!(
        "  serve --gun-adapter-bind ADDR|off\n    Opt-in custom TCP/XML social service; disabled by default. Requires a compatible adapter (not included). Stock GunDll.dll uses a different UDP protocol: -gunserver/-gunport alone cannot connect to this service. --gun-bind remains an alias. Developer tickets require loopback; account:token tickets require a separate single-use account ticket. See docs/gun-protocol.md."
    );
    println!(
        "  serve --initialize-ui true --portals true --game-data PATH\n    Explicit development instant portal routes; enabled by serve-training."
    );
    println!(
        "  serve --initialize-ui true --lifestones true --game-data PATH\n    Development lifestone binding, @recall, and bound resurrection. Enabled by serve-training."
    );
    println!(
        "  serve-training --game-data PATH [--account local --data PATH --bind 127.0.0.1:9000]\n    Development preset: initialized HUD, bundled training encounter, and once-per-character hand item."
    );
    println!(
        "  serve --initialize-ui true --training-kit true --game-data PATH\n    Explicit development kit: grants one DAT-backed hand item once per human character. Disabled by default."
    );
    println!(
        "  skill-state --character-id N [--event t,SKILL_HEX|g,SKILL_HEX|u,SKILL_HEX|r,SKILL_HEX] --data PATH\n    Offline skill inspection, training, explicit free grant, untraining, or table-backed advancement."
    );
    println!(
        "  player-state --character-id N [--event COMMAND] --data PATH\n    Offline progression inspection/edit: x,XP a,SPEND_XP g,GOLD s,GOLD d,DAMAGE h,HEAL v,COST r,RECOVERY z(resurrect)."
    );
    println!(
        "  serve --game-data PATH [--content PATH|observed|training|none] [--encounters PATH] [--content-overlay PATH] [--world-rules PATH] [--admin-bind 127.0.0.1:PORT]\n    Encounters use explicit development rules. training selects the authored starter encounter.\n    --content-overlay adds world-builder placements; its NAME.rules.tsv (or --world-rules) sets custom creature stats, AI, loot, patrols and spawn groups.\n    --admin-bind enables local live world editing from the content editor (token in DATA/admin.token)."
    );
    println!(
        "  inventory-reorder --character-id N --request PATH\n    Offline import of one reassembled native inventory sorting message (296 bytes)."
    );
    println!(
        "Offline inventory commands (server must be stopped):\n  inventory-list --character-id N\n  inventory-grant --character-id N --game-data PATH --id DEFINITION_HEX --visual-did VISUAL_HEX [--quantity N --stack-limit N --equipment-mask HEX --slot N]\n  inventory-move --character-id N --item-id IID --source-slot N --slot N\n  inventory-equip --character-id N --item-id IID --equipment-mask HEX\n  inventory-unequip --character-id N --item-id IID\n  inventory-split --character-id N --item-id IID --quantity N [--slot N]\n  inventory-consume --character-id N --item-id IID --quantity N\nAll accept --account local --data PATH. These manage stored state; native item replication remains unfinished.\n"
    );
    println!(
        "Pyrealium - AC2 build 7148 emulator\n\n  pyrealium serve [--bind 127.0.0.1:9000] [--account local] [--data data] [--seconds N] [--visual-did HEX] [--game-data PATH] [--initialize-ui true] [--test-map-travel true (loopback only)]\n  pyrealium stop [--data data]\n  pyrealium character-create --account local --name Explorer [--data data]\n  pyrealium character-create-human --account local --name Explorer [--data data]\n  pyrealium character-list --account local [--data data]\n  pyrealium character-restore --account local --character-id 1 [--data data]\n  pyrealium legacy-inspect --landblocks PATH --id 0299FFFF\n  pyrealium legacy-validate --landblocks PATH\n\nLocal developer authentication only (native auth type 1). Optional DAT world bootstrap is experimental. Character list/restore commands require the server to be stopped. Terrain commands inspect extracted data; they do not establish gameplay compatibility."
    );
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "run".into());
    if command == "prepare-content" {
        return prepare_content(args.collect());
    }
    if command == "license" || command == "--license" {
        println!(
            "{}\n\n{}",
            pyrealium::licensing::notice(),
            pyrealium::licensing::LICENSE_TEXT
        );
        return Ok(());
    }
    if command == "--help" || command == "help" {
        usage();
        return Ok(());
    }
    let mut bind: SocketAddr = "127.0.0.1:9000".parse()?;
    let mut account = "local".to_string();
    let mut auth_data = None;
    let mut local_accounts = None;
    let mut web_bind: SocketAddr = "127.0.0.1:19400".parse()?;
    let mut public_origin = None;
    let mut public_host = None;
    let mut public_port = None;
    let mut data = PathBuf::from("run");
    let mut config_path: Option<PathBuf> = None;
    let mut database_url: Option<String> = None;
    let mut db_action: Option<String> = None;
    let mut seconds = None;
    let mut visual = None;
    let mut name = None;
    let mut game_data = None;
    let mut content = None;
    let mut encounters = None;
    let mut content_overlay = None;
    let mut world_rules = None;
    let mut admin_bind = None;
    let mut landblocks = None;
    let mut record_id = None;
    let mut character_id = None;
    let mut initialize_ui = None;
    let mut training_kit = None;
    let mut lifestones = None;
    let mut portals = None;
    let mut map_travel = false;
    // This is an adapter transport, not the retail UDP service. Never implicitly
    // expose it on the game server's interface or advertise retail compatibility.
    let mut gun_bind: Option<SocketAddr> = None;
    let mut item_id = None;
    let mut slot = None;
    let mut source_slot = None;
    let mut inventory_request = None;
    let mut progression_event = None;
    let mut quantity = 1u16;
    let mut stack_limit = 1u16;
    let mut equipment_mask = 0u16;
    if command == "db" {
        db_action = args.next();
    }
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--config" => config_path = Some(value.into()),
            "--database" => database_url = Some(value),
            "--bind" => bind = value.parse()?,
            "--account" => account = value,
            "--auth-data" => auth_data = Some(PathBuf::from(value)),
            "--local-accounts" => local_accounts = Some(PathBuf::from(value)),
            "--web-bind" => web_bind = value.parse()?,
            "--public-origin" => public_origin = Some(value),
            "--public-host" => public_host = Some(value),
            "--public-port" => public_port = Some(value.parse::<u16>()?),
            "--data" => data = value.into(),
            "--seconds" => seconds = Some(value.parse::<u64>()?),
            "--visual-did" => {
                visual = Some(u32::from_str_radix(value.trim_start_matches("0x"), 16)?)
            }
            "--name" => name = Some(value),
            "--game-data" => game_data = Some(PathBuf::from(value)),
            "--content" => content = Some(PathBuf::from(value)),
            "--encounters" => encounters = Some(PathBuf::from(value)),
            "--content-overlay" => content_overlay = Some(PathBuf::from(value)),
            "--world-rules" => world_rules = Some(PathBuf::from(value)),
            "--admin-bind" => admin_bind = Some(value.parse()?),
            "--landblocks" => landblocks = Some(PathBuf::from(value)),
            "--id" => record_id = Some(u32::from_str_radix(value.trim_start_matches("0x"), 16)?),
            "--character-id" => character_id = Some(value.parse::<u64>()?),
            "--initialize-ui" => initialize_ui = Some(value.parse::<bool>()?),
            "--training-kit" => training_kit = Some(value.parse::<bool>()?),
            "--lifestones" => lifestones = Some(value.parse::<bool>()?),
            "--portals" => portals = Some(value.parse::<bool>()?),
            "--test-map-travel" => map_travel = value.parse::<bool>()?,
            "--gun-adapter-bind" | "--gun-bind" if value == "off" => gun_bind = None,
            "--gun-adapter-bind" | "--gun-bind" => gun_bind = Some(value.parse()?),
            "--item-id" => {
                item_id = Some(if let Some(hex) = value.strip_prefix("0x") {
                    u64::from_str_radix(hex, 16)?
                } else {
                    value.parse()?
                })
            }
            "--slot" => slot = Some(value.parse::<u16>()?),
            "--source-slot" => source_slot = Some(value.parse::<u16>()?),
            "--request" => inventory_request = Some(PathBuf::from(value)),
            "--event" => progression_event = Some(value),
            "--quantity" => quantity = value.parse()?,
            "--stack-limit" => stack_limit = value.parse()?,
            "--equipment-mask" => {
                equipment_mask = u16::from_str_radix(value.trim_start_matches("0x"), 16)?
            }
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    if command == "init" {
        let path = config_path.unwrap_or_else(|| PathBuf::from(config::FILE_NAME));
        if path.exists() {
            return Err(format!("{} already exists; edit it or remove it first", path.display()).into());
        }
        std::fs::write(&path, config::TEMPLATE)?;
        println!("Wrote {}. Set dat_dir and the database url, then start with `pyrealium`.", path.display());
        return Ok(());
    }
    let located = Settings::locate(config_path.as_deref());
    let settings = match &located {
        Some(path) => Settings::load(path)?,
        None if command == "run" => {
            return Err(format!(
                "no {} found here or beside the binary; create one with `pyrealium init`",
                config::FILE_NAME
            )
            .into());
        }
        None => Settings::default(),
    };
    let database = Database::new(
        database_url
            .or_else(|| env::var("PYREALIUM_DATABASE_URL").ok())
            .unwrap_or_else(|| settings.database.url.clone()),
    )?;
    if located.is_some() && command != "serve" && command != "serve-training" {
        data = settings.server.run_dir.clone();
    }
    match command.as_str() {
        "run" => return run_settings(settings, database),
        "db" => match db_action.as_deref() {
            Some("migrate") => {
                database.ensure_exists()?;
                let version = database.migrate()?;
                println!("Database {:?} is at schema version {version}.", database.name());
            }
            Some("status") => {
                let mut client = database.connect()?;
                let version = pyrealium::db::schema_version(&mut client)?;
                let characters: i64 = if version > 0 {
                    client.query_one("SELECT count(*) FROM shard.characters", &[])?.get(0)
                } else {
                    0
                };
                println!("Database {:?}: schema version {version}, {characters} characters.", database.name());
            }
            Some("drop") => {
                if !database.exists()? {
                    println!("Database {:?} does not exist.", database.name());
                    return Ok(());
                }
                if server_running(&database)? {
                    return Err(format!("a server is running on database {:?}; stop it first", database.name()).into());
                }
                database.drop_database()?;
                println!("Dropped database {:?}.", database.name());
            }
            _ => return Err("usage: pyrealium db migrate|status|drop".into()),
        },
        "accounts" => {
            std::fs::create_dir_all(&settings.server.run_dir)?;
            pyrealium::licensing::set_public_origin(settings.public_origin());
            pyrealium::account_web::run(pyrealium::account_web::Config {
                bind: settings.accounts.web_bind,
                database,
                run_dir: settings.server.run_dir.clone(),
                stop: None,
                origin: settings.public_origin(),
                game_host: settings.server.game_host.clone(),
                game_port: settings.server.bind.port(),
            })?;
        }
        "skill-state" => {
            let mut store = Store::open(&database)?;
            let id = character_id.ok_or("--character-id is required")?;
            if let Some(event) = progression_event {
                if let Some(skill) = event.strip_prefix("r,") {
                    store.raise_skill(&account, id, u32::from_str_radix(skill, 16)?)?;
                } else {
                    store.skill_command(
                        &account,
                        id,
                        pyrealium::skills::Command::decode(&event)?,
                    )?;
                }
            }
            let saved = store
                .characters
                .get(&id)
                .filter(|c| c.account == account && !c.deleted)
                .ok_or("active character not owned")?;
            println!(
                "{}: {} available skill credits",
                saved.name,
                saved.skills.available_credits(&saved.progression)
            );
            for (id, entry) in saved.skills.entries() {
                println!(
                    "{id:#x}\trank={}\txp={}\tcredits={}\tlocked={}",
                    entry.rank, entry.xp, entry.credits, entry.locked
                );
            }
        }
        "player-state" => {
            let mut store = Store::open(&database)?;
            let id = character_id.ok_or("--character-id is required")?;
            if let Some(event) = progression_event {
                store.progression_command(
                    &account,
                    id,
                    pyrealium::progression::Command::decode(&event)?,
                )?;
            }
            let character = store
                .characters
                .get(&id)
                .filter(|c| c.account == account && !c.deleted)
                .ok_or("active character not owned")?;
            println!(
                "{} level={} state={:?}",
                character.name,
                character.progression.level().number,
                character.progression
            );
        }
        "inventory-list" | "inventory-grant" | "inventory-move" | "inventory-equip"
        | "inventory-unequip" | "inventory-split" | "inventory-consume" | "inventory-reorder"
        | "inventory-starter" => {
            use pyrealium::inventory::{Command, Item, Location};
            let mut store = Store::open(&database)?;
            let character = character_id.ok_or("--character-id is required")?;
            let owned = store
                .characters
                .get(&character)
                .filter(|c| c.account == account && !c.deleted)
                .ok_or("active character not owned")?;
            if command == "inventory-starter" {
                let directory = game_data.ok_or("--game-data is required to validate starter assets")?;
                let mut portal = pyrealium::dat::Dat::open(directory.join("portal.dat"))?;
                pyrealium::starter_gear::validate(&mut portal)?;
                let granted = store.grant_starter_gear(&account, character)?;
                println!("Starter gear for character {character}: {}", if granted { "saved" } else { "already granted" });
                return Ok(());
            }
            if command == "inventory-list" {
                for item in owned.inventory.items() {
                    println!(
                        "{:#018x}\t{:#010x}\t{}\t{:?}",
                        item.id, item.definition, item.quantity, item.location
                    );
                }
                return Ok(());
            }
            let operation = match command.as_str() {
                "inventory-reorder" => {
                    use pyrealium::inventory_request::{MESSAGE_BYTES, MoveRequest};
                    use std::io::Read;
                    let path = inventory_request.ok_or("--request is required")?;
                    let mut bytes = Vec::with_capacity(MESSAGE_BYTES + 1);
                    std::fs::File::open(path)?
                        .take((MESSAGE_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)?;
                    MoveRequest::decode(&bytes)?.checked_command(character, &owned.inventory)?
                }
                "inventory-grant" => {
                    let directory =
                        game_data.ok_or("--game-data is required to validate grant assets")?;
                    let definition =
                        record_id.ok_or("--id must specify the item definition DID")?;
                    let visual = visual.ok_or("--visual-did is required")?;
                    let mut portal = pyrealium::dat::Dat::open(directory.join("portal.dat"))?;
                    portal.read(definition)?;
                    portal.read(visual)?;
                    let id = store.next_item_id()?;
                    let destination = slot
                        .or_else(|| owned.inventory.free_slot())
                        .ok_or("inventory is full")?;
                    println!(
                        "Development grant: {id:#018x}; stack/equipment rules supplied explicitly by the operator."
                    );
                    Command::Grant(Item {
                        roll: None,
                        id,
                        definition,
                        visual,
                        quantity: quantity.into(),
                        stack_limit: stack_limit.into(),
                        equipment_mask,
                        location: Location::Bag(destination),
                    })
                }
                "inventory-move" => Command::Move {
                    item: item_id.ok_or("--item-id is required")?,
                    source: source_slot.ok_or("--source-slot is required")?,
                    destination: slot.ok_or("--slot is required")?,
                },
                "inventory-equip" => Command::Equip {
                    item: item_id.ok_or("--item-id is required")?,
                    slot: equipment_mask,
                },
                "inventory-unequip" => Command::Unequip {
                    item: item_id.ok_or("--item-id is required")?,
                },
                "inventory-split" => Command::Split {
                    item: item_id.ok_or("--item-id is required")?,
                    new_id: store.next_item_id()?,
                    quantity: quantity.into(),
                    destination: slot
                        .or_else(|| owned.inventory.free_slot())
                        .ok_or("inventory is full")?,
                },
                "inventory-consume" => Command::Consume {
                    item: item_id.ok_or("--item-id is required")?,
                    quantity: quantity.into(),
                },
                _ => unreachable!(),
            };
            store.inventory_command(&account, character, operation)?;
            println!(
                "Inventory change saved. Native inventory initialization requires --initialize-ui true and remains unverified."
            );
        }
        "character-restore" => {
            let mut store = Store::open(&database)?;
            let id = character_id.ok_or("--character-id is required")?;
            store.restore(&account, id)?;
            println!("Restored character {id} for {account}.");
        }
        "character-list" => {
            let store = Store::open(&database)?;
            for c in store.characters.values().filter(|c| c.account == account) {
                println!(
                    "{}\t{}\t{}",
                    c.id,
                    if c.deleted { "deleted" } else { "active" },
                    c.name
                );
            }
        }
        "legacy-validate" => {
            let root = landblocks.ok_or("--landblocks is required")?;
            let result = pyrealium::legacy::validate_terrain(&root)?;
            println!(
                "Validated {} terrain records; {} missing/empty linked info records. Terrain format validation only; gameplay compatibility is unverified.",
                result.terrain_records, result.missing_info_records
            );
            if result.missing_info_records > 0 {
                return Err("legacy terrain has missing/empty linked info records".into());
            }
        }
        "legacy-inspect" => {
            let root = landblocks.ok_or("--landblocks is required")?;
            let id = record_id.ok_or("--id is required")?;
            let bytes = pyrealium::legacy::read_record(&root, id)?;
            let terrain = pyrealium::legacy::Terrain::decode(&bytes, id)?;
            println!(
                "Terrain 0x{:08X}: 17 x 17 samples, info 0x{:08X}, raw height range {:?}, {} bytes. Raw heights require the separate height table for world-space elevation.",
                terrain.id,
                terrain.info_id,
                terrain.height_range(),
                bytes.len()
            );
        }
        "account-web" => {
            std::fs::create_dir_all(&data)?;
            if let Some(origin) = &public_origin {
                pyrealium::licensing::set_public_origin(origin.clone());
            }
            pyrealium::account_web::run(pyrealium::account_web::Config {
                bind: web_bind,
                database,
                run_dir: data,
                stop: None,
                origin: public_origin.ok_or("--public-origin is required")?,
                game_host: public_host.ok_or("--public-host is required")?,
                game_port: public_port.unwrap_or(bind.port()),
            })?;
        }
        "account-stop" => {
            std::fs::write(data.join("account-stop.request"), b"stop\n")?;
            println!("Account service stop requested.");
        }
        "account-create" => {
            let mut accounts = pyrealium::auth::Accounts::open(&database)?;
            let account = account.to_ascii_lowercase();
            if pyrealium::account_web::reserved(&mut accounts, &account)? {
                return Err("That account name is reserved or owns existing characters.".into());
            }
            let password = zeroize::Zeroizing::new(rpassword::prompt_password("Password: ")?);
            let confirm =
                zeroize::Zeroizing::new(rpassword::prompt_password("Confirm password: ")?);
            if *password != *confirm {
                return Err("Passwords do not match".into());
            }
            accounts.create(&account, &password)?;
            println!("Account created. No password was written to command-line arguments or logs.");
        }
        "serve" | "serve-training" => {
            // Game data means the normal world: HUD, equipment, travel, and combat.
            // A bare serve without game data remains character selection only.
            let world = game_data.is_some() || command == "serve-training";
            if command == "serve-training" && game_data.is_none() {
                return Err("--game-data is required".into());
            }
            if world {
                content.get_or_insert_with(|| PathBuf::from("training"));
                encounters.get_or_insert_with(|| PathBuf::from("training"));
            }
            let initialize_ui = initialize_ui.unwrap_or(world);
            let training_kit = training_kit.unwrap_or(world);
            let lifestones = lifestones.unwrap_or(world);
            let portals = portals.unwrap_or(world);
            if auth_data.is_some() && local_accounts.is_some() {
                return Err("--auth-data and --local-accounts are mutually exclusive".into());
            }
            // --auth-data / --local-accounts now only select the login mode: accounts and
            // tickets live in the database.
            let auth = if local_accounts.is_some() {
                Auth::LocalAccounts
            } else if auth_data.is_some() {
                Auth::Tickets
            } else {
                Auth::Developer
            };
            std::fs::create_dir_all(&data)?;
            let gun_service = start_gun(gun_bind, auth, &database)?;
            let config = Config {
                    bind,
                    account,
                    database,
                    save_interval: pyrealium::store::SAVE_INTERVAL,
                    data,
                    run_seconds: seconds,
                    visual_did: visual,
                    game_data,
                    initialize_ui,
                    training_kit,
                    lifestones,
                    portals,
                    map_travel,
                    content,
                    encounters,
                    content_overlay,
                    world_rules,
                    admin_bind,
                    prepared: None,
                };
            serve(config, auth)?;
            drop(gun_service);
        }
        "stop" => {
            if !server_running(&database)? {
                return Err(format!("no server is running on database {:?}", database.name()).into());
            }
            std::fs::create_dir_all(&data)?;
            std::fs::write(data.join("stop.request"), b"stop\n")?;
            if let Some(seconds) = seconds {
                wait_for_stop(&database, seconds)?;
            }
            println!("Graceful stop requested.");
        }
        "character-create" | "character-create-human" => {
            let mut store = Store::open(&database)?;
            let name = name.ok_or("--name is required")?;
            let c = if command == "character-create-human" {
                store.create_native(
                    &account,
                    &pyrealium::world::CharacterCreationRequest {
                        name,
                        entity_did: pyrealium::bootstrap::PLAYER_ENTITY,
                        start_area: 0,
                        race: 1,
                        sex: 0x1000,
                        class: 0,
                        implement: 0,
                        physical_values: Vec::new(),
                    },
                )?
            } else {
                store.create(&account, &name)?
            };
            println!(
                "Saved character {} ({}) for {}. World entry requires the matching local DAT bootstrap profile.",
                c.name, c.id, c.account
            );
        }
        _ => return Err(format!("unknown command {command}").into()),
    }
    Ok(())
}

/// `pyrealium prepare-content DAT_DIR OUTPUT [--strip-training-placements] [--no-shelter-quest]
/// [--content DIR]`: the preparation as files (the server itself stores it in the database).
fn prepare_content(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut options = pyrealium::content_prep::Options { training_placements: true, shelter_quest: true };
    let mut content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--strip-training-placements" => options.training_placements = false,
            "--no-shelter-quest" => options.shelter_quest = false,
            "--content" => content = args.next().ok_or("--content needs a directory")?.into(),
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}").into()),
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let [directory, output] = <[PathBuf; 2]>::try_from(positional)
        .map_err(|_| "usage: pyrealium prepare-content DAT_DIR OUTPUT [--strip-training-placements] [--no-shelter-quest] [--content DIR]")?;
    let prepared = pyrealium::content_prep::prepare(&directory, &content, options)?;
    pyrealium::content_prep::write_files(&prepared, &output)?;
    for line in &prepared.summary {
        println!("{line}");
    }
    println!("Wrote {} and its report.", output.display());
    Ok(())
}

fn serve(config: Config, auth: Auth) -> std::io::Result<()> {
    match auth {
        Auth::Developer => server::run(config),
        Auth::Tickets => server::run_with_auth(config, true),
        Auth::LocalAccounts => server::run_local_accounts(config),
    }
}

/// Explicit custom transport only. No native GunDll adapter is bundled.
fn start_gun(
    bind: Option<SocketAddr>,
    auth: Auth,
    database: &Database,
) -> std::io::Result<Option<pyrealium::gun::GunHandle>> {
    let Some(bind) = bind else { return Ok(None) };
    let service = pyrealium::gun::start(pyrealium::gun::GunConfig {
        bind,
        database: database.clone(),
        auth: if auth == Auth::Developer { pyrealium::gun::GunAuth::Developer } else { pyrealium::gun::GunAuth::Tickets },
    })?;
    println!("GUN adapter listening on TCP {} (custom XML; incompatible with stock GunDll.dll).", service.addr);
    Ok(Some(service))
}

/// `pyrealium run`: the game server, its account service and GUN, all from the settings file.
fn run_settings(settings: Settings, database: Database) -> Result<(), Box<dyn std::error::Error>> {
    pyrealium::licensing::set_public_origin(settings.public_origin());
    std::fs::create_dir_all(&settings.server.run_dir)?;
    let world = settings.server.dat_dir.is_some();
    let path = |value: &Option<String>| value.as_ref().map(PathBuf::from);
    let mut content = path(&settings.world.content);
    let mut encounters = path(&settings.world.encounters);
    if world {
        content.get_or_insert_with(|| PathBuf::from("training"));
        encounters.get_or_insert_with(|| PathBuf::from("training"));
    }
    let auth = settings.server.auth;
    // The game server takes the database lock first, so a second `run` fails before it binds
    // the account port.
    database.ensure_exists()?;
    if server_running(&database)? {
        return Err(format!("a server is already running on database {:?}; `pyrealium stop` first", database.name()).into());
    }
    if let Some(dat_dir) = &settings.server.dat_dir {
        let is_7230 = pyrealium::bootstrap::is_7230(dat_dir)
            .map_err(|e| format!("cannot read the DATs in {}: {e}", dat_dir.display()))?;
        if !is_7230 {
            return Err(format!(
                "{} does not hold the Asheron's Call 2 build 7230 DATs; Pyrealium serves build 7230 data only",
                dat_dir.display()
            )
            .into());
        }
    }
    // With no explicit content, the world comes prepared from the database, prepared again
    // whenever the content packs or the DATs changed.
    let prepared = match (&settings.server.dat_dir, &settings.world.content) {
        (Some(dat_dir), None) => Some(prepared_world(&settings, dat_dir, &database)?),
        _ => None,
    };
    if prepared.is_some() {
        content = None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let accounts = if settings.accounts.enabled && auth != Auth::Developer {
        let config = pyrealium::account_web::Config {
            bind: settings.accounts.web_bind,
            database: database.clone(),
            run_dir: settings.server.run_dir.clone(),
            stop: Some(Arc::clone(&stop)),
            origin: settings.public_origin(),
            game_host: settings.server.game_host.clone(),
            game_port: settings.server.bind.port(),
        };
        Some(std::thread::Builder::new().name("pyrealium-accounts".into()).spawn(move || {
            if let Err(e) = pyrealium::account_web::run(config) {
                eprintln!("Account service stopped: {e}");
            }
        })?)
    } else {
        None
    };
    let gun = start_gun(settings.gun.bind, auth, &database)?;
    let config = Config {
        bind: settings.server.bind,
        account: settings.server.account.clone(),
        database,
        save_interval: Duration::from_secs(settings.save.interval_seconds),
        data: settings.server.run_dir.clone(),
        run_seconds: None,
        visual_did: None,
        game_data: settings.server.dat_dir.clone(),
        initialize_ui: settings.world.initialize_ui && world,
        training_kit: settings.world.training_kit && world,
        lifestones: settings.world.lifestones && world,
        portals: settings.world.portals && world,
        map_travel: settings.world.test_map_travel,
        content,
        encounters,
        content_overlay: settings.world.content_overlay.clone(),
        world_rules: settings.world.world_rules.clone(),
        admin_bind: settings.world.admin_bind,
        prepared,
    };
    let result = serve(config, auth);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(accounts) = accounts {
        let _ = accounts.join();
    }
    drop(gun);
    Ok(result?)
}

/// The prepared world in the database, preparing it first when its inputs changed.
fn prepared_world(
    settings: &Settings,
    dat_dir: &std::path::Path,
    database: &Database,
) -> Result<pyrealium::content_prep::World, Box<dyn std::error::Error>> {
    use pyrealium::content_prep as prep;
    let content = settings.content_dir();
    let options = prep::Options {
        training_placements: settings.world.training_world,
        shelter_quest: settings.world.shelter_quest,
    };
    let key = prep::input_key(dat_dir, &content, options)?;
    database.migrate()?;
    let mut client = database.connect()?;
    if prep::stored_key(&mut client)?.as_deref() != Some(key.as_str()) {
        println!("Preparing world content from {} for {} ...", content.display(), dat_dir.display());
        let prepared = prep::prepare(dat_dir, &content, options)?;
        for line in &prepared.summary {
            println!("{line}");
        }
        prep::store(&mut client, &key, &prepared)?;
        println!("Prepared content saved to the database.");
    }
    Ok(prep::load(&mut client)?.ok_or("the database holds no prepared world")?)
}

/// Whether a game server holds the database's server lock.
fn server_running(database: &Database) -> std::io::Result<bool> {
    let mut client = database.connect()?;
    let (high, low) = ((pyrealium::db::SERVER_LOCK >> 32) as u32, pyrealium::db::SERVER_LOCK as u32);
    let held: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted
                AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
                AND classid::bigint = $1 AND objid::bigint = $2 AND objsubid = 1)",
            &[&i64::from(high), &i64::from(low)],
        )
        .map_err(pyrealium::db::error)?
        .get(0);
    Ok(held)
}

fn wait_for_stop(database: &Database, seconds: u64) -> Result<(), Box<dyn std::error::Error>> {
    if !(1..=60).contains(&seconds) {
        return Err("Stop wait must be 1-60 seconds.".into());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while server_running(database)? {
        if std::time::Instant::now() >= deadline {
            return Err("Server has not stopped; inspect the process.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
