# Pyrealium network tests and the native protocol bridge

The server speaks native AC2 UDP. `Bridge/` is a small Rust program, built on
AC2_Rusted's checksummed, reliable native transport and codecs, that logs on as an
AC2 client and is driven over private anonymous pipes (JSON on stdin/stdout; no IPC
network port is opened). The Python integration tests here start an isolated Pyrealium
server, drive one or more bridges as players, and check what the server sends.
Object IDs remain decimal strings across JSON to preserve all 64 bits.

`prepare_content.py` (with `content_profile.py`) is the server's content step: it
builds the prepared world from Pyrealium's content packs, keeping only placements whose
records the local DATs hold. See the root README for running the server.

## Server rules the tests exercise

The Pyrealium initial ruleset tests **automatic loot grants** on supported mob kills. It does not yet supply a complete retail corpse-looting window, every combat animation, every merchant/quest/social panel, terrain-authoritative server collision or anti-cheat parity. Static recovered placements and authored encounter behavior are distinct; seeing an NPC does not mean its full original script has been implemented.

The Legions profile grants DAT-validated racial starter clothes and three weapons once per character. Clothing changes rebuild the native avatar appearance while preserving face/body traits and colours; current observers, later arrivals, selection and re-entry receive the saved result. Seven additional armor definitions support Human/Lugian torso, legs and feet and a Tumerok torso, including sex-specific appearance tables and server armor values. Unequip the current piece before wearing its replacement. Additional training Drudges drop these profiles after the relevant quest item has been collected; this is an authored loot policy.

Starter attack IDs and animation modes come from DAT tables; damage, vigor cost, range and timing remain development rules. Pyrealium sends native arrow/boulder/javelin and racial auto-magic entities through CreateObject/InitializeMissile. Release is currently an authored 350ms delay and flight uses an authored 22m/s speed; damage is revalidated and applied after flight. Starter magic skills use direct cast/impact scripts. Objects are removed from every original recipient at impact, including after a disconnect or area change. Native ballistic parameters, sound/light/streak effects and full spellbook parity remain incomplete.

The supported human dungeon circuit is: equip a weapon, defeat three Idol Spawn, use the mentor for the reward, bind the lifestone, use the training gateway to the outdoor hub, then recall. This verifies the **authored Pyrealium starter circuit**, not the recovered retail tutorial's full quest chain. Starter Idol pursuit is server-owned, limited to its current cell, with an 18m leash; this is not cross-cell pathfinding or line-of-sight collision. Both current observers and later arrivals receive its authoritative pose; respawn returns it to the original placement.

Separately, the Shelter Guard/Lieutenant/Sentry introduction and training-stone
text use native DAT StringInfo references, with per-character durable progress.
`@quest shelter` reports its status. The continuation covers Digger/tool/Scout,
Sentinel/glyph/portal, Advance Scout and a reconstructed Menhir completion step.
Item consumption, rewards and glyph travel use durable atomic transactions.
See the Pyrealium guide for reconstructed placements and remaining retail gaps.

## Build and verify

The bridge carries native `WM_Craft` registry, ingredient-validation, execution
and rank-increase messages, including full-width profession XP and item IDs.
The craft registry lists 356 default recipes across 26 disciplines, plus the Arwic
sword recipe after the Overseer teaches it. The server
rechecks ingredients, recipe level, workshop distance, ownership, life/combat
state, costs and cooldown before committing inventory and progress together.
Manufacturing, refining, dyeing, surveying, five mining disciplines and butchery
use the same durable journal. Recipe grants and missing server formulas are
explicit Pyrealium reconstruction policies; spellbinding extraction is not enabled.
The authored mentor's Field Supplies/Potion Practice continuation is enabled in
the Legions profile as well as the development-kit profile. These lessons and
recipe grants are Pyrealium policy, not recovered retail entitlement rules.

`Network/test_crafting.py` checks the entire mentor prerequisite chain, both
potion recipes, rejection of wrong/foreign ingredients, unknown recipes, combat
and stale requests, then restarts the server to verify persistence. These tests
use isolated ports 19528/19529 and save evidence in `Saved/AC2NetworkTests`.
`Network/test_professions.py` verifies multi-stack manufacture, XP/rank increases,
item stats, refining, pickaxe manufacture, survey cooldowns, native-ground
deposit discovery, mining, depletion and restart persistence over real UDP.
It also uses port 19528; run the two acceptance scripts sequentially.

`Network/test_arwic_mines.py` uses isolated port 19525 to verify the Overseer
conversation, quest-only hammer loot, targeted hammer/sword hand-ins, learned
recipe validation, multi-stage rewards, readable book pages and three restarts.
It rejects combat/distant NPC use, an unlearned recipe and stale ingredients.
Its combat fixture lowers Outcast health; a separate native-DAT unit test checks
all six production mine actors, their native combat/AI values and corpse-aware
spawner refill. Native `MessageDialog` StringInfo is decoded into readable chat
while the original script event remains available for protocol diagnostics.

`Network/test_vigor_loot.py` uses two clients and isolated port 19526 to check
Lightning Bolt's learned-skill/drum requirements, periodic vigor drain, observer
updates and live examination. It also checks Baron Colier's native 3–6 generic
loot slots, ownership, stable reopening and rolled equipment stats preserved on
pickup. Locations, training XP and weakened boss health are disposable fixtures;
target vigor and loot limits come from the installed DAT.

`Network/test_weapon_loot.py` uses isolated ports 19532/19533 to check four
rolled weapon drops, pickup/inspection, native minimum-level refusal, equipped
Flame Bolt damage, unchanged burn ticks, remote spell FX and a full server
restart retaining equipment and stats. The corpse seed, positions, XP and
weakened boss health are test fixtures; generated damage is documented Pyrealium
reconstruction. Weapon identities and minimum use levels come from the DAT.

`Network/test_wilderness_population.py` uses ports 19534/19535 and two disposable
characters to verify seven regions in the full prepared world: all five Knorr
areas, Arramora and Drudge Citadel. It checks visibility, death replication and
loot pickup. Sample targets have reduced health; native-DAT tests separately
validate every expansion actor's ground, stats, loot and managed spawner refill.

`Network/test_hunting_quests.py` uses ports 19536/19537 to complete all 35 NPC
hunts from both immutable catalogs, including Ikeras, Arramora and Foundry
chains, a partial-progress restart, fixed/capped return rewards, level gates,
duplicate-turn-in protection and completed-journal replay. NPC positions are
production content; ten low-health copies per hunt cycle through its placed
qualifying variants at native ground positions. Production saves are never used.

`Build-AC2Network.ps1 -Test` builds the bridge and Pyrealium and runs their Rust tests. Install Rust with rustup so `cargo` is on PATH; an optional local installation in `Tools/Rust` is also supported. Python defaults to the bundled runtime if present, otherwise `python`; use `-Python` for another installation.

The bridge dependency is pinned at AC2_Rusted commit `bcf67b4b9ac3ee9eb429d68ba6766cfce47c5a25`, in `References/AC2_Rusted`; `Cargo.lock` pins registry dependencies. Initialize it with `git submodule update --init --recursive`, or clone this repository with `--recurse-submodules`. Do not silently update that checkout when building.

`Network/test_network.py` uses two native UDP clients and a unique temporary store to verify creation, visibility, movement, chat, mob death, loot, logout, re-entry and persistence across a full server restart. `Network/test_auth.py` verifies account login, wrong-password rejection, ticket replay rejection, anonymous rejection and the source archive. They use loopback ports 19270/19271 and 19280/19282/19283 respectively. Results are in `Saved/AC2NetworkTests`.

All integration scripts generate their compatible content in the temporary test
directory. Set `AC2_DAT_DIRECTORY` to use a retail installation outside this
checkout; no pre-existing `Saved` files are needed.

`Network/test_gameplay.py` uses two real UDP clients on 19320/19321 to exercise weapon equip/unequip, combat modes, rejected wrong-weapon casting, melee/magic/missile attacks, creature pursuit and shared locations, incoming damage, kills, the starter reward, portal visibility removal and recall. It also tests player death/resurrection and journal replay of equipment and quest completion. It uses disposable stores under `Saved/AC2NetworkTests`.

`Network/test_combat_recovery.py` uses two authenticated accounts on isolated ports
19490/19492/19493. It checks that a rejected combat-button or attack request returns
both local and remote avatars to peace, allows immediate equipment changes, and
does no damage. Synthetic placements in DAT-visible neighboring rooms verify
cross-cell melee, retaliation and death; overlapping hidden rooms stay excluded.
It also verifies bow/magic kills at 12-14m across a visible room boundary without
health resets or long-range melee hits, replicated missiles and their cleanup,
and cancellation of airborne damage when returning to peace.
Range uses shared landblock coordinates (including outdoor block offsets) plus
native visibility. This does not implement server wall collision or line of sight.
Out-of-range feedback reports distance and weapon reach. Z or Stop Combat returns
to peace before changing equipment; an out-of-range combat-button attack now
returns to peace automatically. Set `AC2_TEST_SERVER` to test a separately built
server binary.

The two-client gameplay test also checks that a rejected attack restores peace
on both the player's model and the observing client.

The Legions server profile sends compact native inventory profiles because the pinned bridge decoder does not yet support the optional embedded native VisualDesc in full inventory graphs. Authoritative object visuals still travel in CreateObject. The original-client server profile retains full graphs; servers sending those full inventory graphs to this bridge still need decoder support.

`Network/test_server_launcher.py` runs the public batch launcher from another
working directory, connects a native client, and stops the server gracefully.
It verifies that normal startup reuses the built server without invoking Rust,
repeated startup reports the existing process without rewriting live content,
rebuilding a running server is rejected, and unverified locks are preserved.
It also checks recovery of abandoned world/account markers from an exited process.

`test_armor.py` exercises every starter clothing slot for all six supported bodies
through two UDP clients. `test_gameplay.py` additionally checks appearance for a
late observer and Shelter introduction order/restart persistence.

`test_shelter.py` uses two UDP clients on 19360/19361 to complete the tool/glyph/
Vault path, reject remote or incorrect targets, verify all three Human armor
updates on both peers, and replay completion and travel after restart. Rust tests
also reject a full backpack without consuming the tool or appending a journal
entry, and validate all seven armor profiles for both sexes.

Compatibility details: Pyrealium accepts unencrypted character creation on queue 4, and retains the incoming ordered counter across character logout. The bridge adapts these two behaviors without altering AC2_Rusted or the upstream reference. It decodes Pyrealium's current admin7148 selection layout even when using the experimental 7230 login profile.

The bridge preserves instance stamps and 64-bit IDs, including snapshot animation state.
Movement/acceleration/jump scales, global modifiers and motion-descriptor changes apply
to the local player. Item use preserves native unsigned object-ID tags (0x84); lifestone
use has no target and the stone as the item. Recall sends skill 0x343 through the native
active-skill request; the server decides whether and where it succeeds.

## Doors and vendors

Using a nearby door or vendor opens it. Normal doors use native open/closed modes
and opening/closing animations. Pyrealium replicates their state to current and newly
arriving observers. Door state resets when the server restarts.
Key/quest-locked doors report their restriction; unlocking scripts are unfinished.
Use requires a living character within four metres of a visible object.

Vendors are identified by installed EntityDesc StoreTemplate/StoreGroup properties.
The server answers the native shop's purchase and sale requests. The server rechecks ownership, price, funds, bag capacity,
range, combat state and the active offer identity before committing inventory and
gold together. Purchases use server-issued offer IDs, never client prices.

This is an authored Pyrealium stock profile: health potion (5 gold), training weapon
(12), Mucor mushroom (2), and a 100-unit tallow bundle (2). Supported starter gear
and recovered armor sell for 3 gold. Quest items are excluded. Retail vendor-specific
inventories, dynamic prices, consignment, and partial-stack sales remain unfinished.
Only verified placements are spawned; this does not restore missing town population.

`Network/test_services.py` uses two native UDP clients and a disposable store on
19386/19387 to check door replication, late observers, locked doors, purchases,
gear sales, insufficient funds, stale offers, ownership and combat restrictions.

## NPC conversations

Wiki NPC conversations use the existing native use completion, named speech and
MessageDialog messages. Using an NPC without a dedicated action reads
its next topic. Select any catalog NPC and enter `@talk` to browse information
without invoking its working quest or vendor action. Range, life/combat state,
native noncombat identity and a shared one-second dialogue limit are enforced by
the server. The catalog does not grant quests or rewards. See
[`npc-dialogue-v1.audit.json`](../emulator/content/npc-dialogue-v1.audit.json)
for coverage and [`test_npc_dialogue.py`](test_npc_dialogue.py) for the isolated
UDP regression on ports 19540/19541.

## Sources and licenses

This server continues Thwargle's AC2E, derived from Vanquish's Pyrealium at commit `7c2e856840b04302d1c78f057dcf8094326472a4`. The separate bridge and both upstream projects are AGPL-3.0-only. Their original notices and licenses are preserved. Pyrealium's account backend serves the corresponding server source at `/source.zip`; rebuild it after source changes. The original game DATs are read-only local inputs and are excluded from the source archive. See `GUIDE.md` and `Network/Bridge/NOTICE.md`.
