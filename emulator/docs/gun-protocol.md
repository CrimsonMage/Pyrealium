# GUN networking and adapter protocol

## Compatibility status

**The implemented endpoint is a custom TCP/XML adapter service, not a retail GUN
server.** It is disabled by default. Enable it with `--gun-adapter-bind ADDR`;
`--gun-bind ADDR` remains an alias, and either accepts `off`. For local adapter
development, use an explicit address such as `127.0.0.1:12310`. Bind failures are
reported rather than silently dropping an explicitly requested service.

Stock AC2 clients load `GunDll.dll` and pass `-gunserver` / `-gunport` to its
connection interface. Changing those arguments alone cannot connect the DLL to
our TCP endpoint. The previously reported external `gun-stub` / AC2_Rusted
adapters are not included in this checkout or its pinned submodule. Persisted social-service tests therefore
do not demonstrate client integration or retail compatibility.

## Retail evidence reviewed (2026-10-31)

The local Legions 7160 and admin 7148 decompiles show `TurbineGun::Init` loading
`GunDll.dll`, `TurbineGunUtils::GunCreateInstance` resolving `DllGetClassObject`,
and `TurbineGun::RequestConnect` calling its connection interface. The Legions
addresses are 0x005C6DA0, 0x005C8620 and 0x005C69A0 respectively. These are client
wrappers, **not the DLL implementation**. No GunDll binary/decompile was found in
the available checkout, extracted references or Downloads. The archived 2017
client contains `chatclient.dll`, which is not evidence of GunDll compatibility.

The archived 2005 `.elog` PCAPs independently establish the actual network:

- `launcher/launcher.login.until.char.overview.elog`, SHA-256
  `1ffabcf6401a70b0ee86086aca8f7b3e91a586be6cd29d2c4e473b6613ea372b`:
  UDP port 2310, connection packets beginning `88 01` / `88 02` / `80 02`, system
  32-bit core message types 0x000000C1/0x000000C2/0x000000C3, then the 16-bit
  GUN XML opcode 0xC350. XML negotiation
  proceeds through Connect, ConnectRes (RSAKey), ConInfo (CdKey/Random),
  ConInfoRes, PresentTicketCmd (Ticket), PresentTicketRes. Heartbeats/ACKs and
  sequencing continue around the messages. TCP length framing is absent.
- `adding.to.friendslist.elog`, SHA-256
  `498b2c038ac52800c105ed26c8ee8a97133fce8ad9dcbd92cd4ac02c630d2fd8`:
  native requests use `Cx` and `User`, results use `Cx` and `HR`; the list result
  has `Cnt` and `FriendStatus` children with `User`, `Mode`, and `Location`.
- `friend.add.elog`, SHA-256
  `71a571cd0e1923d364b7dfca12cb6056560484e86cf06d3ce1ff427fefd18887`:
  the same UDP/schema behavior, including packets requesting acknowledgements.
- Across the 501 unique captures in the recovered `Packets` directory, the native
  Rust replay decodes 5,169 of 5,171 UDP datagrams and recovers 303 XML messages.
  Eight messages required fragment reassembly; 89 duplicate frames are suppressed.
  These are offline observations, not a successful native client login.

Pyreal R&D's `Network/GUN/GunSever.cs`, `Packets/Header.cs`, `Packets/Packet.cs`,
`Packets/GunPacketLib/Client.cs`, and `Message/Messages/ConnectRes.cs` corroborate
UDP framing and RSAKey negotiation. That code has hardcoded handshake values and
an explicitly incomplete fragment implementation; it is reference material, not
a complete reliable transport to copy. Pyreal's newer `Network/GunBase` TCP
proxy/account service is a different layer and must not be confused with the
native DLL's network protocol.

The actual transport is DirectPlay 8. Its published
[data frame layout](https://learn.microsoft.com/en-us/openspecs/windows_protocols/mc-dpl8r/97f93510-ca87-4df6-9af1-af5d930e42fa),
[CONNECT layout](https://learn.microsoft.com/en-us/openspecs/windows_protocols/mc-dpl8r/b3c67ec1-f73c-4c47-bd06-95cd4a3b4219),
[CONNECTED layout](https://learn.microsoft.com/en-us/openspecs/windows_protocols/mc-dpl8r/2377d224-85b7-4c1a-8677-bd18a08dc5da),
and [SACK layout](https://learn.microsoft.com/en-us/openspecs/windows_protocols/mc-dpl8r/9ca3f71c-1040-40b0-9866-2e2e40cf65b0)
resolve several misleading names in Pyreal:

- The two bytes called `Magic` are separate `bSeq` and `bNRcv` sequence/ACK values.
  Data frames have separate command and control bytes, not a single 16-bit flag.
- `80 06` is a selective ACK, not an Alive message. ACKs do not consume sequence
  numbers. Actual keepalive data frames carry the session identifier on version
  1.5 and later connections; captures contain version 1.5 and 1.6 negotiation.
- Core session message types are 32-bit. The `06 00 01 00` and `05 00 01 00` in
  connection frames are protocol versions, not GUN application opcodes.

Pyreal R&D's `MsgMgr.cs` unconditionally accepts ConInfo and PresentTicketCmd,
while `Utility/Crypto/RSA.cs` explicitly describes itself as unused test code.
Its hardcoded key/challenge and unconditional success do not establish working
RSA authentication and have not been copied into a live service.

## Implemented native codec and capture replay

`src/gun/native/` now provides bounded unsigned connection/data/ACK decoders,
connection and ACK encoders, message fragmentation, and a receive window for
reliable sequential messages. The receive window handles out-of-order delivery,
8-bit sequence rollover, retries, message boundaries, session-checked keepalives,
and the end-stream flag. Coalesced payload decoding honors header and payload
alignment. Optional ACK/send masks are decoded but their sender-side semantics
are not yet implemented. Unordered/unreliable delivery is explicitly unsupported.
The 4 KiB datagram, 64 KiB message and 64-frame receive limits are local bounds.

GUN XML is NUL-terminated after its 0xC350 opcode. Forty-five decoded messages
have eight extra bytes after that terminator. Reading those bytes as XML made
valid ConnectRes, FStart and other responses appear malformed in the earlier
single-datagram audit. The new codec retains the eight-byte trailer separately;
its purpose and any validation requirements remain unresolved. Unknown trailer
lengths are rejected. The recovered XML retains `Cx`, `HR`, `User`, `Cnt`,
`FriendStatus`, `Word` and child `Val` fields without adapter field translation.

The full replay reports 2,465 ACK frames, 2,640 data frames, 14 CONNECT frames,
50 CONNECTED frames, 77 keepalive frames and 303 complete XML messages, including
339 FriendStatus entries. It does not export entry values. Two four-byte
end-stream frames in `logout.completely.elog` remain unresolved: their keepalive/
correlate bit needs the negotiated version, which that capture does not include.
They are reported as truncated under the replay's explicit default version 1.6;
the decoder does not silently reinterpret them as a legacy connection.

Build and run against local captures (nothing is sent over the network):

```powershell
cargo build --manifest-path emulator/Cargo.toml --target-dir target/game --example gun_replay
python emulator/tools/replay_gun_captures.py PATH_TO_PACKETS_DIRECTORY --decoder target/game/debug/examples/gun_replay.exe --output Saved/gun-native-replay.json
```

The Python reader passes packets through stdin to the Rust codec; no raw capture
fixtures, tickets or identities are written to the repository. Reports contain
only hashes, counts, allowlisted schema names and static error categories. Each
capture and direction has separate receive state. Missing fragments and conflicting
retries cannot be combined into an apparently complete message.

The earlier lightweight Python audit remains available for individual datagrams;
it omits fragments and does not handle the newly identified XML trailer:

```powershell
python emulator/tools/audit_gun_captures.py PATH_TO_CAPTURE.elog --output Saved/gun-audit.json
```

## Work required for a retail connection

Wire the codec into a UDP session lifecycle with sender ACK processing, selective
retransmission, timers, cancellation masks, negotiated versions, and complete
disconnect handling. Implement the DirectPlay core session/name table exchange.
Then reproduce the Connect/ConInfo RSA challenge exchange and validate native
tickets without treating them as our web account tokens. Native XML serialization
must preserve `Cx`/`HR`, `User`, `Cnt`, `FriendStatus`, `Word` (Flags request), and
child `Val` fields. Finally wire the client callbacks and test a real DLL login,
friends/ignore changes, presence, tells, packet loss, reconnect and shutdown.
The captures support this work, but do not by themselves establish the full
crypto, COM ABI, or reliable-session implementation. The native codec is currently
used by the offline replay; it does not replace the custom adapter or open a UDP
listener. Signed connections and authentication remain unimplemented.

## Custom adapter framing

TCP. A frame is a big-endian `u32` payload length (1 to 65536), then that many bytes of UTF-8:
one XML element. Attributes use double quotes and `&amp; &lt; &gt; &quot;` plus `&#N;` for control
characters. No declaration, comments or text; children are flat (one level). Requests carry
`seq="N"` and the answer echoes it. `hr` is `0x%08X`, the `ZT_*` HRESULT (0 is success).

Synthetic adapter frame (`FriendAdd`, seq 3; not a retail capture):

```
00 00 00 24  <FriendAdd Name="Bob@Pyrealium" seq="3"/>   (length 0x24 = 36 bytes of text)
```

## Custom adapter messages

Some element names resemble retail, but fields, correlation and payloads below
belong to the custom adapter. The formerly cited `decompiled-pdb/gundll/gun_messages.csv`
is not available in this checkout and is not evidence for this implementation.

| Client sends | Server answers | Notes |
|---|---|---|
| `<Connect ProtoVer="1"/>` | `<ConnectRes hr Random="32 hex"/>` | no `seq`; the client then raises `ConnectEv` itself |
| `<PresentTicketCmd seq Ticket="hex"/>` | `<PresentTicketRes seq hr Status="0" Account/>` | then `<FStart>` with `<Friend/>` children when the account has friends |
| `<ReleaseTicketCmd seq/>` | `<ReleaseTicketRes seq hr/>` | |
| `<GetFriendsList seq/>` | `<GetFriendsListRes seq hr><Friend Name Mode Location/>...` | |
| `<FriendAdd seq Name/>` / `<FriendDelete seq Name/>` | `<FriendAddRes seq hr/>` / `<FriendDeleteRes seq hr/>` | add also pushes `<FriendStat/>` for the new friend |
| `<GetIgnoredList seq/>` | `<GetIgnoredListRes seq hr><Ignored Name/>...` | |
| `<IgnoredAdd seq Name/>` / `<IgnoredDelete seq Name/>` | `<IgnoredAddRes/>` / `<IgnoredDeleteRes/>` | |
| `<Flags seq Op Which Flags/>` | `<FlagsRes seq hr WhichFlag Flags/>` | Op 0 get, 1 set, 2 clear; Which 0 control, 1 state |
| `<SetLoc seq Flags Location/>` | `<SetLocRes seq hr/>` | the presence mode bits and a place |
| `<SendToUserEx seq Name Text Flags/>` | `<SendToUserExRes seq hr/>` | the target gets `<UTellEv From Text Flags/>` |

Server events (no `seq`): `<FStart>` (friends snapshot), `<FriendStat Name Mode Location/>` (a
friend's mode or location changed), `<UTellEv From Text Flags/>`.

Any request before a successful ticket is answered `<NameRes hr="0xC112080B"/>` (`ZT_E_NOT_TICKETED`).
Mode bits: 1 logged in, 2 away, 4 do not disturb, 8 in game, 16 in chat. A session's mode is its
`SetLoc` flags plus 1; a character in the world adds 1 and 8.
Error codes used: `ZT_E_ALREADY_IN_LIST` 0xC1121009, `ZT_E_NOT_IN_LIST` 0xC1121401,
`ZT_E_AUTH_INVALID_TICKET` 0xC1120806, `ZT_E_ALREADY_TICKETED` 0xC112080C, `ZT_E_INVALID_USER`
0xC1120012 (tell to someone not here), `ZT_E_INVALID_NAME_LENGTH` 0xC1120019, `ZT_E_TOO_MANY`
0xC112001A, `ZT_E_INVALID_ARG` 0xC1120013.

## Identity and presence

A ticket is the opaque bytes the client presents. Developer mode (no account store, loopback bind
only): the account name. With `--auth-data` or `--local-accounts`: `account:token`, a token from the
account store (`tickets/`), spent once and bound to the connecting IP.

The adapter carries these values in plaintext TCP (hex is encoding, not
encryption). Use loopback or a protected tunnel for remote development. The
game login and adapter require separately issued tokens because each is spent
once; the launcher does not currently obtain an adapter ticket or connect to it.

Friends are keyed by the part of `Name@World` before the `@`, case-insensitive; names come back as
stored. This adapter has one local world's identity space: it does not route across
worlds, and changing `@World` does not select another server. A name is online when a GUN session of that account exists, or a character of that name is
in the world on this server (then its owner's sessions supply away/do-not-disturb). Lists persist in
`gun-users.json` in the data directory.

ReleaseTicket clears the session identity; further protected requests require a new ticket.
An ignored character name also blocks tells from its owning account. List changes are
acknowledged only after persistence succeeds (`0x80004005` on failure). Socket output uses
bounded queues; slow readers are disconnected without blocking the game loop. Incomplete
frames expire after ten seconds, and unauthenticated sessions after thirty seconds.

## Not implemented

Native UDP/session/crypto/ticket compatibility, client adapters, cross-world
routing, chat rooms (`JoinRoom`, `SendToRoomEx`, ...), accounts, news, personas,
support and matchmaking messages. The captured game chat path must be audited
separately; a working adapter tell does not prove retail chat routing.
