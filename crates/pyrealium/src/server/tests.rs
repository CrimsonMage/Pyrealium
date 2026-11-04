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
