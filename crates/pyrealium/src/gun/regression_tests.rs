use super::*;

fn session(account: &str) -> (Session, mpsc::Receiver<Vec<u8>>, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    let (out, receiver) = mpsc::sync_channel(1);
    (Session { account: Some(account.into()), out, socket, flags: 0,
        location: String::new(), words: [0; 2] }, receiver, peer)
}
fn response(receiver: &mpsc::Receiver<Vec<u8>>) -> Elem {
    let bytes = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    Elem::parse(std::str::from_utf8(&bytes[4..]).unwrap()).unwrap()
}
fn hub(state: State) -> Hub {
    Hub { state: Mutex::new(state), db: Mutex::new(None), auth: GunAuth::Developer }
}

#[test]
fn full_output_queue_disconnects_instead_of_blocking_shared_state() {
    let (client, _receiver, _peer) = session("alice");
    let mut state = State::default();
    state.sessions.insert(1, client);
    state.deliver(1, &Elem::new("First"));
    let start = Instant::now();
    state.deliver(1, &Elem::new("Overflow"));
    assert!(start.elapsed() < Duration::from_millis(250));
}

#[test]
fn ignoring_a_character_blocks_its_account_tells() {
    let (alice, alice_out, _alice_peer) = session("alice");
    let (bob, bob_out, _bob_peer) = session("bob");
    let mut state = State::default();
    state.sessions.extend([(1, alice), (2, bob)]);
    state.characters.insert("alice hero".into(), "alice".into());
    state.users.insert("bob".into(), User { friends: vec![], ignored: vec!["Alice Hero@Pyrealium".into()] });
    let hub = hub(State::default());
    request(&hub, &mut state, 1, "alice", "1", &Elem::new("SendToUserEx").attr("Name", "bob").attr("Text", "hidden"));
    assert_eq!(response(&alice_out).get("hr"), Some("0x00000000"));
    assert!(bob_out.try_recv().is_err());
}

#[test]
fn release_revokes_identity_and_location_only_updates_are_announced() {
    let (alice, alice_out, _alice_peer) = session("alice");
    let (bob, bob_out, _bob_peer) = session("bob");
    let mut state = State::default();
    state.sessions.extend([(1, alice), (2, bob)]);
    state.users.insert("alice".into(), User { friends: vec!["bob".into()], ignored: vec![] });
    state.announce(&BTreeSet::from(["bob".into()]));
    assert_eq!(response(&alice_out).get("Mode"), Some("1"));
    state.sessions.get_mut(&2).unwrap().location = "Cragstone".into();
    state.announce(&BTreeSet::from(["bob".into()]));
    assert_eq!(response(&alice_out).get("Location"), Some("Cragstone"));
    let hub = hub(state);
    handle(&hub, 2, "127.0.0.1".parse().unwrap(), &Elem::new("ReleaseTicketCmd"));
    assert_eq!(response(&bob_out).name, "ReleaseTicketRes");
    assert_eq!(response(&alice_out).get("Mode"), Some("0"));
    handle(&hub, 2, "127.0.0.1".parse().unwrap(), &Elem::new("GetFriendsList"));
    assert_eq!(response(&bob_out).get("hr"), Some("0xC112080B"));
}

#[test]
fn failed_list_save_rolls_back_and_reports_failure() {
    let (alice, outgoing, _peer) = session("alice");
    let mut state = State::default();
    state.sessions.insert(1, alice);
    let hub = hub(State::default());
    request(&hub, &mut state, 1, "alice", "1", &Elem::new("FriendAdd").attr("Name", "bob"));
    assert_eq!(response(&outgoing).get("hr"), Some("0x80004005"));
    assert!(state.users["alice"].friends.is_empty());
}

#[test]
fn partial_frame_timeout_is_fatal_not_an_idle_timeout() {
    let (client, _outgoing, mut peer) = session("alice");
    let mut socket = client.socket;
    socket.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    peer.write_all(&[0, 0, 0, 10, b'<']).unwrap();
    let error = read_frame_with_deadline(&mut socket, &mut Vec::new(), &AtomicBool::new(false),
        Duration::from_millis(10)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn authenticated_ticket_is_ip_bound_and_single_use() {
    let database = crate::db::testdb::fresh();
    let mut accounts = crate::auth::Accounts::open(&database).unwrap();
    accounts.create("alice", "long-test-password").unwrap();
    let ip = "127.0.0.1".parse().unwrap();
    let token = accounts.issue("alice", ip).unwrap();
    let hub = Hub { state: Mutex::new(State::default()), db: Mutex::new(Some(database.connect().unwrap())), auth: GunAuth::Tickets };
    assert_eq!(account_for_ticket(&hub, &format!("alice:{token}"), ip), Ok("alice".into()));
    assert_eq!(account_for_ticket(&hub, &format!("alice:{token}"), ip), Err(ZT_E_AUTH_INVALID_TICKET));
    let token = accounts.issue("alice", ip).unwrap();
    assert_eq!(account_for_ticket(&hub, &format!("alice:{token}"), "127.0.0.2".parse().unwrap()), Err(ZT_E_AUTH_INVALID_TICKET));
    drop(accounts);
}
