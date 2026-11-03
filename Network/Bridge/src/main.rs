// SPDX-License-Identifier: AGPL-3.0-only
// Separate process adapter for AC2_Rusted's native protocol. Stdio is private IPC, never a listener.
#![forbid(unsafe_code)]
mod events;
mod examination;
mod shop;
mod journal;
mod crafting;
use ac2_net::{protocol::*, Credential, Logon, NetClient, NetClientConfig, NetEvent};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    net::ToSocketAddrs,
    sync::mpsc,
    time::{Duration, Instant},
};

const MAX_LINE: usize = 65536;
#[cfg(test)]
mod use_tests {
    use super::*;
    #[test]
    fn bag_sort_uses_native_reorganize_and_unsigned_heap_fields() {
        let ClientEvent::Interp{function,arguments,..}=reorganize_item(InvMoveDesc::new(0x813000000000002a,7,3,7,8,12)) else {panic!("reorganize event")};
        assert_eq!(function,0x820e0049);assert_eq!(arguments.len(),284);
        assert_eq!(arguments[232+2],0x84);assert_eq!(arguments[232+44],0x84);
        assert_eq!(arguments[232+25],4);assert_eq!(arguments[232+26],4);
        let ScriptRequest::ReorganizeContents(desc)=ScriptRequest::decode(function,&arguments).unwrap().unwrap() else {panic!("sort descriptor")};
        assert_eq!(desc.item(),0x813000000000002a);assert_eq!(desc.from_slot(),3);assert_eq!(desc.target_slot(),8);assert_eq!(desc.quantity(),12);
    }
    #[test]
    fn equip_and_unequip_keep_native_unsigned_ids_and_slot_types() {
        for equip in [true,false] {
            let d=if equip {InvEquipDesc::equip(0x8140000000000001,7,0x40,7,3)}
                else {InvEquipDesc::unequip(0x8140000000000001,7,0x40,3)};
            let ClientEvent::Interp {function,arguments,..}=equip_item(d,equip) else {panic!("equip event")};
            assert_eq!(arguments.len(),180);
            assert_eq!(arguments[111],0x84);assert_eq!(arguments[115],4);
            let decoded=ScriptRequest::decode(function,&arguments).unwrap().unwrap();
            let d=match decoded {ScriptRequest::EquipItem(d)|ScriptRequest::UnequipItem(d)=>d,_=>panic!("equip descriptor")};
            assert_eq!(d.item(),0x8140000000000001);assert_eq!(d.container(),7);assert_eq!(d.location(),0x40);assert_eq!(d.slot(),3);
        }
    }
    #[test]
    fn native_use_keeps_unsigned_object_ids_and_default_target() {
        let owner=0x5000000000000001;let item=0x8140000000030003;
        let ClientEvent::Interp {function,arguments,..}=use_item(owner,item,0) else {panic!("use event")};
        assert_eq!(function,0x82160016);assert_eq!(arguments.len(),160);
        assert_eq!(&arguments[112..132],&[1,1,0x84,0xcd,1,2,0x84,0xcd,3,0x82,0xcd,2,0x82,0xcd,2,1,2,2,2,2]);
        let ScriptRequest::Use(desc)=ScriptRequest::decode(function,&arguments).unwrap().unwrap() else {panic!("use desc")};
        assert_eq!(desc.user,owner);assert_eq!(desc.item,item);assert_eq!(desc.target,0);
    }
}
fn emit(value: Value) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key].as_str().ok_or_else(|| format!("Missing {key}"))
}
fn id(v: &Value, key: &str) -> Result<u64, String> {
    text(v, key)?.parse().map_err(|_| format!("Invalid {key}"))
}
fn word(v: &Value, key: &str) -> Result<u32, String> {
    v[key]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("Invalid {key}"))
}
fn vector(v: &Value, key: &str) -> Result<[f32; 3], String> {
    let a = v[key]
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or("Invalid vector")?;
    let mut result = [0.; 3];
    for i in 0..3 {
        result[i] = a[i]
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() < 1e8)
            .ok_or("Invalid coordinate")? as f32;
    }
    Ok(result)
}
fn send(client: &mut NetClient, event: ClientEvent) -> Result<(), String> {
    let (queue, data) = event.encode();
    // Pyrealium accepts clear character creation on its control queue (4),
    // whereas the original client uses encrypted queue 8 with a session key.
    let queue = if queue == 8 { 4 } else { queue };
    client.send(queue, data).ok_or("Not connected")?;
    Ok(())
}
fn use_item(user: u64, item: u64, target: u64) -> ClientEvent {
    let mut reference = UsageDesc::new(user, item, target).to_reference();
    // Native UsageDesc::37 stores the item/user as unsigned object IDs (0x84).
    // The reference crate uses generic signed long tags (0x82) for both;
    // preserve the captured native profile accepted by Pyrealium.
    if let PackedEntry::Heap { object, .. } = &mut reference.entries[0] {
        object.tags[2] = 0x84;
        object.tags[6] = 0x84;
        if target!=0 {object.tags[9]=0x84;}
    }
    ClientEvent::Interp { function: 0x82160016,
        arguments: pack_values(&[PackValue::Reference(reference)]), encrypted: false }
}
fn equip_item(desc: InvEquipDesc, equip: bool) -> ClientEvent {
    // gmCWInterface::EquipItem / UnEquipItem: typed unsigned IDs and slot enums.
    // Generic reference setters keep signed Int/Long tags; the native script
    // constructors preserve these narrower tags in their packed heap graph.
    let mut reference=desc.to_reference();
    if let PackedEntry::Heap {object,..}=&mut reference.entries[0] {
        object.tags=vec![2,1,2,0x84,0xcd,2,if equip {2}else{4},4,0x84,0xcd,0x84,0xcd,2,2,2,4,0x82,0xcd,4];
    }
    ClientEvent::Interp {function:if equip {0x820e004f}else{0x820e0015},
        arguments:pack_values(&[PackValue::Reference(reference)]),encrypted:false}
}
fn reorganize_item(desc: InvMoveDesc) -> ClientEvent {
    let mut reference=desc.to_reference();
    // Native same-container drag (820e0049), including typed DID/unsigned IID
    // fields retained in the sorting captures. Generic MoveItem is a different
    // operation: ownership transfer, splitting, or dropping into the world.
    if let PackedEntry::Heap{object,..}=&mut reference.entries[0] {
        object.tags=vec![2,2,0x84,0xcd,2,2,4,2,0x84,0xcd,2,4,2,2,2,2,0x84,0xcd,0x82,0xcd,2,2,4,2,4,2,4,2,0x84,0xcd,1,2,0x84,0xcd,2,2,0x84,0xcd,4,4,2,2,2,2,0x84,0xcd,2,2,2,2,4];
        if desc.from_slot()!=0 {object.tags[1]=4;}
        if desc.quantity()>1 {object.tags[25]=4;}
    }
    ClientEvent::Interp{function:0x820e0049,arguments:pack_values(&[PackValue::Reference(reference)]),encrypted:false}
}
struct Session {
    client: NetClient,
    account: String,
    player: u64,
    instance: u16,
    teleport: u16,
    force: u16,
    moveto: u16,
    ready: bool,
    logged_on: bool,
    since: Instant,
    entered: bool,
    skill_request: u32,
}
impl Session {
    fn connect(v: &Value) -> Result<Self, String> {
        let host = text(v, "host")?;
        let port = word(v, "port")?;
        if host.len() > 253 || port == 0 || port >= 65535 {
            return Err("Invalid server address/port".into());
        }
        let server = (host, port as u16)
            .to_socket_addrs()
            .map_err(|_| "Cannot resolve server")?
            .find(|a| a.is_ipv4())
            .ok_or("Server has no IPv4 address")?;
        let account = text(v, "account")?.to_ascii_lowercase();
        if account.is_empty()
            || account.len() > 32
            || !account
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err("Account must contain 1-32 letters, digits or underscores".into());
        }
        let ticket = v["ticket"].as_str().unwrap_or("");
        let credential = if ticket.is_empty() {
            if !server.ip().is_loopback() {
                return Err("Remote servers require an account-service login ticket".into());
            }
            Credential::Account
        } else {
            if ticket.len() != 64
                || !ticket
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err("Invalid login ticket".into());
            }
            // Pyrealium's GLS ticket uses auth 0x40000002, packed as a narrow string.
            Credential::Password(ticket.into())
        };
        let version = v["version"].as_str().unwrap_or("7148");
        if !matches!(version, "7148" | "7230") {
            return Err("Pyrealium wire version must be 7148 or 7230".into());
        }
        let client = NetClient::connect(NetClientConfig {
            server,
            logon: Logon {
                version: version.into(),
                account: account.clone(),
                credential,
                connection_sequence: ac2_net::real_time() as u32,
                crypto: None,
            },
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            account,
            player: 0,
            instance: 0,
            teleport: 0,
            force: 0,
            moveto: 0,
            ready: false,
            logged_on: false,
            since: Instant::now(),
            skill_request: 0,
            entered: false,
        })
    }
    fn command(&mut self, v: &Value) -> Result<(), String> {
        match text(v, "command")? {
            "delete" if !self.ready && self.player == 0 => {
                send(&mut self.client, ClientEvent::DeleteCharacter {
                    account: self.account.clone(), character: id(v, "id")?,
                })?;
            }
            "enter" => {
                self.ready = false;
                self.player = 0;
                // Pyrealium retains its incoming ordered counter across character logout.
                if !self.entered {
                    self.client.net_mut().enter_world();
                    self.entered = true;
                }
                send(
                    &mut self.client,
                    ClientEvent::EnterWorld {
                        character: id(v, "id")?,
                        account: self.account.clone(),
                    },
                )?;
                emit(
                    json!({"event":"status","phase":"entering","message":"Entering server world..."}),
                );
            }
            "create" => {
                let name = text(v, "name")?;
                if name.len() > 64 || name.is_empty() {
                    return Err("Invalid character name".into());
                }
                let mut physical = Vec::new();
                for pair in v["physical"].as_array().ok_or("Missing physical values")? {
                    let key = pair[0]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or("Invalid physique")?;
                    let value = pair[1]
                        .as_f64()
                        .filter(|x| x.is_finite() && x.abs() < 100.)
                        .ok_or("Invalid physique")?;
                    physical.push((key, value as f32));
                    if physical.len() > 32 {
                        return Err("Too many physique values".into());
                    }
                }
                send(
                    &mut self.client,
                    ClientEvent::CharGenResult {
                        account: self.account.clone(),
                        result: CharGenResult {
                            name: name.into(),
                            entity_did: 0x4700110f,
                            start_area: word(v, "start_area")?,
                            race: word(v, "race")?,
                            sex: word(v, "sex")?,
                            physical,
                            ..Default::default()
                        },
                    },
                )?;
            }
            "ready" if self.player != 0 => {
                send(&mut self.client, ClientEvent::SceneRenderingComplete)?;
                self.ready = true;
                emit(
                    json!({"event":"status","phase":"world","message":"Connected to server world"}),
                );
            }
            "move" if self.ready => {
                let heading = v["heading"]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or("Invalid heading")? as f32;
                let p = ClientPosition {
                    time: self.client.net().timer().cur_time(),
                    position: CellPosition {
                        cell: word(v, "cell")?,
                        origin: vector(v, "position")?,
                    },
                    heading_degrees: heading.rem_euclid(360.),
                    motion: vector(v, "motion")?,
                    cancel_moveto: true,
                    instance_stamp: self.instance,
                    teleport_stamp: self.teleport,
                    force_position_stamp: self.force,
                    moveto_stamp: self.moveto,
                    jump_velocity: None,
                };
                send(&mut self.client, ClientEvent::Position(p))?;
            }
            "chat" if self.ready => {
                let message = text(v, "text")?;
                if message.len() > 2048 {
                    return Err("Chat message too long".into());
                }
                let literal = StringInfo {
                    literal: Some(message.into()),
                    ..Default::default()
                };
                let request = match v["channel"].as_str().unwrap_or("say") {
                    "emote" => ScriptRequest::EmoteText(message.into()),
                    "tell" => ScriptRequest::Tell {
                        mode: 0,
                        text: literal,
                        recipient: StringInfo {
                            literal: Some(text(v, "recipient")?.into()),
                            ..Default::default()
                        },
                    },
                    "say" => ScriptRequest::Say {
                        mode: 0,
                        text: literal,
                    },
                    _ => return Err("Unsupported server chat channel".into()),
                };
                send(&mut self.client, request.to_event())?;
            }
            "target" if self.ready => {
                send(
                    &mut self.client,
                    ScriptRequest::SetTarget(id(v, "id")?).to_event(),
                )?;
            }
            "examine" if self.ready => {
                send(&mut self.client, examination::request(id(v,"id")?))?;
            }
            "attack" if self.ready => {
                let target = id(v, "id")?;
                send(
                    &mut self.client,
                    ClientEvent::Interp {
                        function: 0x82040008,
                        arguments: pack_values(&[PackValue::LongInt(target)]),
                        encrypted: false,
                    },
                )?;
            }
            "stop_attack" if self.ready => {
                send(
                    &mut self.client,
                    ClientEvent::Interp {
                        function: 0x82040010,
                        arguments: vec![],
                        encrypted: false,
                    },
                )?;
            }
            "combat" if self.ready => {
                let enabled = v["enabled"].as_bool().ok_or("Expected combat enabled flag")?;
                send(&mut self.client, ClientEvent::Interp {
                    function: if enabled { 0x82040008 } else { 0x82040010 },
                    arguments: vec![], encrypted: false,
                })?;
            }
            "skill_cancel_untrain" if self.ready => {
                send(&mut self.client,ClientEvent::Interp{function:0x82140001,arguments:vec![],encrypted:false})?;
            }
            "skill_train" | "skill_raise" | "skill_untrain" if self.ready => {
                let skill=word(v,"skill")?;
                if skill==0 || skill>65535{return Err("Invalid skill ID".into());}
                let function=match v["command"].as_str().unwrap_or("") {"skill_train"=>0x82140002,"skill_raise"=>0x8214000c,_=>0x82140007};
                send(&mut self.client,ClientEvent::Interp{function,arguments:pack_values(&[PackValue::Int(skill)]),encrypted:false})?;
            }
            "skill" if self.ready => {
                let skill = word(v, "skill")?;
                if skill == 0 || skill > 65535 { return Err("Invalid skill ID".into()); }
                self.skill_request = self.skill_request.wrapping_add(1);
                send(&mut self.client, ClientEvent::Interp {
                    function: 0x8204000b,
                    arguments: pack_values(&[PackValue::Int(self.skill_request), PackValue::Int(skill), PackValue::LongInt(id(v, "target")?)]),
                    encrypted: false,
                })?;
            }
            "resurrect" if self.ready => {
                send(
                    &mut self.client,
                    ClientEvent::Interp {
                        function: 0x8207000e,
                        arguments: vec![],
                        encrypted: false,
                    },
                )?;
            }
            "recall" if self.ready => {
                self.skill_request=self.skill_request.wrapping_add(1);
                send(&mut self.client, ClientEvent::Interp {
                    function: 0x8204000b,
                    arguments: pack_values(&[PackValue::Int(self.skill_request), PackValue::Int(0x343), PackValue::LongInt(self.player)]),
                    encrypted: false,
                })?;
            }
            "loot" if self.ready => {
                send(
                    &mut self.client,
                    ScriptRequest::LootAll(InvTakeAllDesc::new(id(v, "id")?, self.player))
                        .to_event(),
                )?;
            }
            "open_container" if self.ready => {
                send(
                    &mut self.client,
                    ScriptRequest::OpenContainer(id(v, "id")?).to_event(),
                )?;
            }
            "transmute_loot" if self.ready => {
                send(&mut self.client,ScriptRequest::TransmuteAll(InvTransmuteAllDesc::new(id(v,"id")?,self.player)).to_event())?;
            }
            "close_container" if self.ready => {
                send(&mut self.client, ScriptRequest::CloseContainer(id(v,"id")?).to_event())?;
            }
            "take_item" if self.ready => {
                send(&mut self.client, ScriptRequest::MoveItem(InvMoveDesc::new(
                    id(v,"id")?,id(v,"container")?,word(v,"from")? as i32,self.player,-1,1)).to_event())?;
            }
            "move_item" if self.ready => {
                send(
                    &mut self.client,
                    reorganize_item(InvMoveDesc::new(
                        id(v, "id")?,
                        self.player,
                        word(v, "from")? as i32,
                        self.player,
                        word(v, "to")? as i32,
                        word(v, "quantity")? as i32,
                    )),
                )?;
            }
            "drop_item" if self.ready => {
                let mut event=reorganize_item(InvMoveDesc::new(id(v,"id")?,self.player,
                    word(v,"from")? as i32,u64::from(word(v,"landblock")?),-1,word(v,"quantity")? as i32));
                if let ClientEvent::Interp {function,..}=&mut event {*function=0x820e0036;}
                send(&mut self.client,event)?;
            }
            "equip" if self.ready => {
                send(
                    &mut self.client,
                    equip_item(InvEquipDesc::equip(
                        id(v, "id")?,
                        self.player,
                        word(v, "location")?,
                        self.player,
                        word(v, "slot")? as i32,
                    ),true),
                )?;
            }
            "unequip" if self.ready => {
                send(
                    &mut self.client,
                    equip_item(InvEquipDesc::unequip(
                        id(v, "id")?,
                        self.player,
                        word(v, "location")?,
                        word(v, "slot")? as i32,
                    ),false),
                )?;
            }
            "craft_validate" | "craft_execute" | "craft_raise" if self.ready => send(&mut self.client,crafting::command(v)?)?,
            "shop_buy" if self.ready => send(&mut self.client,shop::buy(self.player,id(v,"vendor")?,id(v,"offer")?))?,
            "shop_sell" if self.ready => send(&mut self.client,shop::sell(self.player,id(v,"item")?,word(v,"slot")?,word(v,"quantity")?))?,
            "shop_close" if self.ready => send(&mut self.client,shop::close(id(v,"vendor")?))?,
            "shop_refresh" if self.ready => send(&mut self.client,ClientEvent::Interp{function:0xb83a0066,arguments:pack_values(&[PackValue::LongInt(id(v,"vendor")?)]),encrypted:false})?,
            "use" if self.ready => {
                let event = use_item(self.player, id(v, "item")?, id(v, "target")?);
                send(&mut self.client, event)?;
            }
            "logout" => {
                self.ready = false;
                send(
                    &mut self.client,
                    ClientEvent::LogOffCharacter {
                        character: id(v, "id")?,
                    },
                )?;
            }
            _ => return Err("Command is not available in this session state".into()),
        }
        Ok(())
    }
    fn poll(&mut self) -> bool {
        if !self.logged_on && self.since.elapsed() > Duration::from_secs(45) {
            emit(json!({"event":"disconnected","message":"Server login timed out"}));
            return false;
        }
        for event in self.client.poll() {
            match event {
                NetEvent::Connected { server_time, .. } => {
                    emit(json!({"event":"clock","server_time":server_time}));
                }
                NetEvent::TimeSync(_) => {
                    // HandleTimeSynch has already applied native monotonic clock adoption.
                    // Forward that clock, not the process's local elapsed time.
                    emit(json!({"event":"clock","server_time":self.client.net().timer().cur_time()}));
                }
                NetEvent::LoggedOn => {
                    self.logged_on = true;
                    emit(
                        json!({"event":"status","phase":"selection","message":"Loading server characters..."}),
                    );
                }
                NetEvent::Disconnected(_) => {
                    emit(
                        json!({"event":"disconnected","message":"Server connection closed or timed out"}),
                    );
                    return false;
                }
                NetEvent::ConnectionError { .. } => {
                    emit(
                        json!({"event":"disconnected","message":"Server rejected the connection"}),
                    );
                    return false;
                }
                NetEvent::Blob(blob) => {
                    // Pyrealium currently emits the admin CharacterSet layout even for its 7230 profile.
                    match ServerEvent::decode(&blob.data, Build::Admin7148) {
                        Ok(event) => {
                            match &event {
                                ServerEvent::CreatePlayer { id, .. } => self.player = *id,
                                ServerEvent::CreateObject(o) if o.id == self.player => {
                                    self.instance = o.physics.instance_stamp;
                                    // PhysicsDesc stamps: position, teleport, force-position, move-to.
                                    self.teleport = o.physics.timestamps[1];
                                    self.force = o.physics.timestamps[2];
                                    self.moveto = o.physics.timestamps[3];
                                }
                                ServerEvent::UpdatePosition(p) | ServerEvent::PositionCell(p)
                                    if p.sender.id == self.player =>
                                {
                                    self.instance = p.sender.instance;
                                    self.teleport = p.teleport_stamp;
                                    self.force = p.force_position_stamp;
                                }
                                ServerEvent::CharExitGame => {
                                    self.ready = false;
                                    self.player = 0;
                                }
                                _ => {}
                            }
                            for value in events::convert(event) {
                                emit(value);
                            }
                        }
                        Err(_) => {
                            let opcode = blob
                                .data
                                .get(..4)
                                .map(|a| u32::from_le_bytes(a.try_into().unwrap()))
                                .unwrap_or(0);
                            emit(
                                json!({"event":"unsupported","opcode":opcode,"bytes":blob.data.len()}),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        true
    }
}
fn main() {
    let (tx, rx) = mpsc::sync_channel::<Value>(64);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let mut line = Vec::new();
            // Bound each command before allocating; closing stdin closes the native session.
            let mut limited = std::io::Read::take(&mut input, (MAX_LINE + 1) as u64);
            let n = match limited.read_until(b'\n', &mut line) {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 || n > MAX_LINE {
                break;
            }
            if let Ok(v) = serde_json::from_slice(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let mut session: Option<Session> = None;
    emit(json!({"event":"bridge","version":1}));
    loop {
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(v) => {
                    let command = v["command"].as_str().unwrap_or("");
                    if command == "disconnect" {
                        session = None;
                        emit(json!({"event":"disconnected","message":"Disconnected"}));
                        continue;
                    }
                    if command == "quit" {
                        return;
                    }
                    let result = if command == "connect" {
                        session = None;
                        Session::connect(&v).map(|s|{session=Some(s);emit(json!({"event":"status","phase":"connecting","message":"Connecting to server..."}));})
                    } else if let Some(s) = session.as_mut() {
                        s.command(&v)
                    } else {
                        Err("Not connected".into())
                    };
                    if let Err(message) = result {
                        emit(
                            json!({"event":if command=="connect" {"disconnected"} else {"error"},"message":message}),
                        );
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        if let Some(s) = session.as_mut() {
            if !s.poll() {
                session = None;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
