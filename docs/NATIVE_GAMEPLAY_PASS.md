# Native gameplay, appearance and content audit

> Historical work log. Some checks recorded here ran the former Unreal client (its `Launch-AC2.ps1`,
> `Unreal/Saved` captures and `*_ui.py` tests), which is no longer in this repository.

## Follow-up UI, signs and grass corrections — 2026-10-30

The shared road-arrow meshes `06000554` and `06000555` now correct the
horizontal lettering UV orientation. Front/back rendered captures both show
readable lettering; arrow geometry, world headings, collision and DAT placement
are unchanged. Selected labels use `SelectionName`, while general labels retain
`NameTextTemplate`; explicit multiline names receive enough vertical space.
The native `InqRadarLook` color policy is still incomplete, so this does not
establish full retail nameplate color parity.

Grass material version 17 converts native partial alpha to spatial coverage
instead of discarding all texels below a fixed cutout threshold. It retains the
existing distance fade, batching, native textures and exact DAT placements.
Rendered checks include the `7CB9` cave approach. This improves cutout edges;
it does not fill authored bare patches or reproduce the native per-instance
scenery shadow-color calculation, which remains missing.

Inventory wheel, arrow buttons and thumb dragging now keep rows and the thumb
in sync immediately. Inventory inspection requests authoritative server rows,
including native item descriptions and supported potion requirements. A short
right-click on the local character also requests self-examination without
turning the player into their own combat target.

The corpse window sizes its title and close button against its final dragged
bounds. Its single-bitmap frame now extends the center while preserving curved
end caps, rather than repeating the entire frame. Native chat background tiles
shared boundary pixels; clipping their overlap removes double-blended dark
lines through the text. Chat scrolling also rebuilds its thumb immediately.

These changes do **not** complete generated loot or active skill effects:

- Brave Crawlspace Vermin (`47000F7E`, loot profile 784) has three captured
  observations: two elixir-only and one elixir-plus-boots bundle. The current
  capture-derived distribution is not the full retail loot generator.
- Item instances still lack generated armor/damage/affix and color-variant
  state. Recovered footwear has distinct native models and race restrictions,
  but color defaults to zero. Missing ratings are reported as unavailable.
- The supported health/vigor potions retain the existing development recovery
  rules. Slayer elixirs and other unsupported effects are not substituted with
  healing effects. Their effect formulas still require recovery/implementation.
- Native skill training, raising, untraining and cancellation pass the rendered
  test. Most active skill effects remain unsupported; catalog visibility does
  not mean those effects are implemented.
- Examination reports known server level/health/vigor. A living creature's
  profile ends with the retail "Chance to hit this target: N%" row, byte for byte
  as the retail servers sent it (28,062 captured creature profiles in the 2017
  captures: `examination.stt` 0x9E78D75C, then 0xD81912CB with the int variable
  0xF6620CE8, then a break). Pyrealium policy: retail's formula is not known, so
  N is this server's own roll for the examiner's basic melee attack
  (`Encounters::hit_percent`: the rating and defense the swing uses, through
  `combat::hit_chance`, rounded to a whole percent).

This pass built the editor, game and server. Checks include the rendered
combat/inventory/mouse flow, self-examination, inventory scroll/drag regression,
native sign front/back and ground-cover captures, recovered equipment and
corpse-loot UDP tests, and examination unit tests. All 52 `AC2.Data` tests passed.
The recovered-population combat render passed with the widened corpse title,
native tooltip and item pickup. The shop render passed sale/purchase/close and
door/chest animation and collision. Render tests temporarily avoid overlapping
saved panels without changing the user's persisted positions. Account-service
state was not changed. Tests use isolated accounts and world stores.

## Recovered equipment, chat and cave furnishings — 2026-10-30

`recover_apparel.py` now generates 31 bounded wearable profiles from matching
Legions EntityDesc, qualities, pile appearance and explicit wearable-family
records. Each profile records its source evidence and fingerprints; equipment
slot, race and male/female appearance must agree. Complex quest-item physics
and ambiguous families are excluded. Armor ratings come only from the item's
own properties, not from another item with the same mesh. Two of these profiles
have a known rating; the others explicitly report that the armor rating is
unavailable. Random retail loot rolls/affixes remain unavailable.

Previously saved mask-zero copies receive a narrowly validated, journaled
equipment-metadata repair when used. Leather Boots `47000FDC` equip on a Human;
Initiate Moccasins `47001009` require a Tumerok. Incompatible races are rejected
with that requirement instead of the generic unsupported-use message. Clothing
appearance and equipment state survive logout and server restart. This is not a
general grant of item-use support to every recovered loot definition.

Basic skill attacks now enter the repeated ordinary-attack queue after their
first skill hit. A new attack cannot replace an in-flight projectile or windup.
The real UDP test verifies that a basic missile skill kills its target without
another click. Shortcut icons resolve from the racial skill repository rather
than three unrelated fixed icon IDs.

Chat text is painted at the native ChatLog node, within its clip and beneath
later windows. Wrapped-row counts determine history range and thumb position.
Dragging the thumb preserves its grab offset, and overlapping panels receive
input first. The rendered UI test drags to both history endpoints and back.
Overhead names now use the native label-template font and outline attributes.

Cell furnishing offsets are already in landblock space. Applying the room frame
a second time displaced dungeon torches/furnishings while their light sources
remained on the walls. The extra transform is removed; a rendered capture of
`7CB90102` confirms its torch mesh and flame at the wall. This does not change
room geometry or source placements. Native ground cover now fades across
120–160 m in planar distance, before component culling at 200 m.

Validation: 106 Rust library tests passed, one explicitly ignored; 17 bridge
tests passed. UE automation reported 47 successes and five successes with
warnings, zero failures. Rendered UI and environment smokes passed. Real UDP
gameplay, service and recovered-equipment tests passed using isolated stores.
`tests/network/test_recovered_equipment.py` verifies old-item repair, appearance,
race rejection, inspection, restart persistence and the captured interior
Drudge Taskmaster in `7CB9013A`. A stationary online 1600×900 performance run
at `7EBA` averaged 10.605 ms/frame (~94 FPS) on this machine, with the extended
foliage distance. This does not establish performance in every biome.

Remaining findings from these screenshots:

- The raised-hand bow idle reproduces using native Human map `03000014`,
  mode `41000010`, idle animator `0500004C`. The firing sequence animates and
  attack repetition is fixed, but the idle-pose discrepancy is not resolved.
- Native grass atlases are loaded at their available high resolution. The
  overhead cross-card/grid appearance is not resolved; no extra plants or
  random placement changes were introduced to conceal it.
- Cave lighting/material fidelity remains incomplete. Programmable and
  generated-coordinate layers still need native equivalents; the torch
  placement correction is not a claim that all cave shading is now correct.
- The compatible pack has two exterior creatures and one captured interior
  Drudge Taskmaster in `7CB9`. Its 128 DAT furnishings are a separate client
  resource. The available captures do not provide a complete room-by-room
  population, spawner schedule, or chest-content definition for this cave.
- Existing supported vendor interaction, buy/sell and inspection tests pass.
  Recovered vendor identities without store properties and complete retail
  vendor stock remain unsupported; the bounded development stock is not retail
  stock. No name-based heuristic promotes arbitrary NPCs to vendors.

## Blood, inspections, skill controls and floating text — 2026-10-30

Ground decals are now decoded from `DecalFX` instead of discarding its
`0x80000000` field. The renderer projects them onto world collision, retains
them independently of the victim, and expires them using the recovered
`DecalEffect::Think` fade calculation. Native material, size, rotation, radius,
colour and lifetime are preserved; red/green and gore preferences select the
authored variants. Invalid ground samples and sharp discontinuities are omitted.
Projection uses a bounded grid, not the original engine's triangle clipping.

The generic Pyrealium damage event reuses the ground decal asset in native FX table
`1E000076`, key `41000620`: red material `310003DD`, green `310006DD`, lifetime
30 seconds. It does **not** invoke the Decapitate ability's animations or other
effects. This asset and fade are evidenced by the DAT and recovered decal code;
an ordinary-hit trigger on every retail attack has **not** been established.
This generic trigger is a Pyrealium policy pending stronger capture evidence.
Only positive authoritative damage emits it; health snapshots do not.

Creature deaths broadcast the native composed `Corpse of <name>` StringInfo,
including its nested original name, as a stamped string-quality update.
Observers entering later receive that update; respawns restore the original
name. Inspections preserve that composition and include runtime player vitals,
level/race, self-only progression, service interaction information, creature
health, item quantity/equipment slots and known armor values. Owned corpse loot
and ground drops can be inspected. Loot inspection still enforces ownership,
visibility, expiry and range. Unknown random affixes/resistances are not invented.

The skill panel's button area now follows its selected skill state; it was
previously forced to the overview state, hiding modification controls. Native
Train/Raise/Untrain/Cancel controls update the repository. Raised skills untrain
as newly earned XP debits invested XP one-for-one, refunding available XP without
increasing total XP; completion returns the skill credits. Dependencies and
pending untraining prevent invalid use/advancement. Cancellation, restart and
journal compaction preserve progress. A malformed old credit-only packet was
removed; credit changes use a complete native repository update.

All 70 decoded panels remain browsable, but **full skill gameplay is not
implemented**: training rules currently support Human/Lugian/Tumerok skills up
to level 50, with hero/faction/crafting and additional race rules still gated.
Most advanced ability effects still require implementation. Functional tree
controls do not establish support for every ability shown in the DAT.

Nearby nonselected objects now receive clickable DAT `NameTextTemplate` labels
within the client's current radar/target range, with overlap and viewport checks.
Authoritative hit events create outlined `DamageTextTemplate` numbers; portal
space hides them and world teardown clears them. Native reticle tuning supplies
placement attempts and float travel. Character options expose persistent
Damage numbers, Third-party damage, its range, and Object names controls.
Personal damage defaults on and third-party damage defaults off. Exact retail
damage-script colour/scale/duration, radar-look name colours and averaged bounds
are still incomplete; the hit presentation currently uses a two-second lifetime.

Validation: server library 104 passed/1 ignored; bridge 17 passed; UE Editor and
game builds passed. `AC2.Data.NativeBloodDecals` verifies native red/green assets,
gore suppression, terrain projection, missing-ground rejection and expiry.
`tests/network/test_corpse_loot.py` verifies two-client and late-observer naming,
inspection access and persistent loot. `tests/network/test_services.py` also checks
self/other-player, equipment, vendor, door and chest inspections.
`tests/network/test_journal_ui.py` routes actual Slate clicks through training, raising,
untraining/cancellation, quest tabs and dragging, and rejects skill decode errors.
The outdoor combat render test checks damage-number visibility toggling, ground
decal creation, corpse naming, native loot pickup and original-name respawn.
Screenshots were reviewed; this is not a side-by-side retail-client acceptance.

## Server time and the native sky — 2026-10-30

The client previously started each world at noon and advanced a float by frame
delta. Pyrealium supplied process uptime only at connection, and the bridge discarded
clock events. Clients joining at different times therefore saw different skies,
and restarts reset server time. The weather seed also used a local day counter.

Pyrealium now saves `data/world/clock-origin.json` under the world store lock. Native
wire time is seconds since that world's origin, including downtime; the running
process advances it with `Instant`. The origin is a Pyrealium world convention, not
a recovered epoch of a historical retail server. A first launch creates it;
subsequent launches preserve it. Gameplay deadlines remain on process monotonic
time. Keep the origin with world backups.

The server sends an initial and five-second periodic `CTimeSyncHeader` with the
native reliable sequence and ISAAC checksum. Plain sequence-zero time headers
are rejected by the native header policy. The bridge forwards connection time
and the adopted native timer; Unreal extrapolates using real monotonic seconds,
accepts forward corrections and ignores stale backward updates. World loading,
idle characters and frame rate no longer determine the calendar speed.
Server-generated creature position timestamps use the same wire clock; AI
scheduling continues to use process monotonic time.

Retail verification uses Legions `GameTime::StreamPack` (`005D52B0`),
`CalcDayBegin` (`005D5420`), `UseTime` (`005D5980`), and
`CSkyDesc::GetCurrentDay` (`0050DD80`), plus the installed DAT and an independent
pinned-reference clock probe. `35000000` has zero-time offset 4893 seconds,
zero-year 498, day length **7347 seconds**, and 360 days/year. The sky follows
that calendar's fraction and absolute day seed; weather selection uses the
high 16 hash bits and strict cumulative-frequency comparison from the decompile.
The pinned reference's full-32-bit weather hash differs, so it is not used as
the weather oracle. Existing native sky keys drive object motion, tint,
sun/ambient lighting, fog and weather as the fraction advances.

Validation: `tests/network/test_world_clock.py` passes real UDP checks for idle time,
periodic synchronization, clients joining at different times, and restart after
three simulated elapsed days. Server library tests pass (93 passed, one ignored).
The UDP test also enters two characters and checks movement replication at the
restored multi-day timestamp.
`AC2.Data.NativeServerCalendar`, `AC2.Data.NativeSkyLightingAndLifetime`, and
`AC2.Render.ServerTimeSkyCycle` pass; the render test samples 06/12/18/23 hours
at different frame deltas. Captures in `Unreal/Saved/Screenshots/SkyClock` were
visually inspected. This verifies native calendar/protocol logic and our rendered
cycle; it is not a live side-by-side retail-client screenshot comparison.

## Swimming mode selection — 2026-10-30

The old `Swim = 40000003` mode constant actually selected **riding** according
to native EnumMapper `23000048`. Swimming uses the Water element with Peace
mode. Local movement/behavior and remote network animation now share that
selection, preserve death mode, and restore the requested stance on land.
This prevents combat-mode packets from selecting riding poses while submerged.

Native human water graphs select idle clip `05000280` and movement clips
`05000323`, `050002DD`, `05000345`, instead of riding graphs `05000EBE/05000EBF`.
`AC2.Data.NativeSwimmingClips` and `AC2.Render.NativeSwimmingObservers` pass.
The render test drives the actual local controller and remote network handler,
compares their multipart mesh poses, and verifies attachment continuity and
return to land. Forward crawl and idle tread-water captures were visually checked
in `Unreal/Saved/Screenshots/Swimming`. This test does not use two live UDP clients.

## Tent interior rendering — 2026-10-30

The circus tents in block `6EAD` use entities `47003669`/`4700366A`, visuals
`1F00192D`/`1F001CCD`, and materials `31000845` through `31000848`. Their
modifiers set property `4003`, bound to material field kind 5, to true. This
maps to native `CULLMODE_NONE` (1); false maps to `CULLMODE_CW` (2). The renderer
previously skipped both this override and the serialized layer culling field.
Opaque cloth therefore lost its back faces even while the masked fringe stayed
visible. The missing interior was not missing geometry or texture data.

Material layers now retain their culling state and modifiers apply the native
boolean mapping. World and UI mesh materials select precompiled one/two-sided
variants independently of alpha blending. Ordinary one-sided templates remain
one-sided; DAT geometry, UVs, textures and placement are unchanged. Material
template version 14 and the launcher regenerate missing variants automatically.

`AC2.Data.NativeMaterialCulling` checks both tent sizes' modifiers and the
unmodified one-sided template in world and preview modes. The rendered
`AC2.Render.NativeTentInterior` test loads the actual tent and captures exterior,
inner-wall and inner-roof views in both modes. Both interior views failed before
the fix and pass afterward; screenshots were visually inspected under
`Unreal/Saved/Screenshots/TentInterior`. Opacity, native preview lighting and
appearance, grass contact, and texture/alpha regressions also pass.

This pass uses the installed Legions DATs, the retail/admin decompiles and the
pinned Rust reference. It preserves exact DAT scenery choices and keeps
incompatible capture placements excluded. It does not establish full retail
physics, rendering, quest or population parity.

## Appearance and effects

Some programmable-material alternatives serialize an empty texture slot but
declare its default sampler in their native property list. Ignoring that default
terminated the fixed-function texture cascade before its palette/lighting stages.
The material reader now resolves those defaults before evaluating the cascade.
Creator skin/clothing palette tests cover six avatars at their choice extremes;
rendered body/clothing screenshots confirm visible palette changes. This fixes a
specific color-processing defect, not every unsupported native shader.

FX table channel conditions now select the configured blood intensity and color
before starting particles, visuals, scripts or sounds. Previously normal, green
and the retail comic-style "Fun" blood variants could run together. Blood effects
and green blood are available in the existing options panel.

Persistent model wounds now use native `BloodyDamaged` FX `41000002`, not hit
particles or an invented decal. `DBFXTable 1E000057` selects appearance tables
`200001E8/2000003A/200001E9/20000056` with key `2`, according to the low/normal
and red/green blood channels. The normal table's five intensity thresholds are
`0.40/0.55/0.70/0.85/1.0`. Pyrealium health snapshots drive the scalar as
`(maximum-current)/maximum`; this maps to 60/45/30/15/0 percent health. The
thresholds and textures are recovered DAT data; that health-to-scalar mapping
is our client fallback, not a recovered retail server script formula. Explicit
native FX scalar and StopFX messages are also supported. The admin decompile's
`FXManager::DoAppearanceFX` at `00505C30` confirms multiplying the appearance
key's scalar modifier by the effect strength and removing it at zero.

Wounds modify materials on existing mesh sections without rebuilding the rig or
interrupting animation. Base skin, dyes, garments and held attachments remain
intact. Local players, remote players, compatible creatures and the inventory
portrait update on health changes. Current health is retained for model/equipment
rebuilds and initial observation; healing, resurrection and logout clear wounds.
Changing blood preferences reapplies the current stage. None/Fun do not acquire
normal wound textures. DAT material-type restrictions are preserved: for example,
Idol Spawn material `3100047D` has type `2`, which its inherited wound table does
not target. It does not get fabricated humanoid wounds. General appearance-FX
mesh/opacity atoms and timed ramps remain outside this wound-material pass.

Portal travel and world entry use the native tunnel textures, particle definition
and enter/loop/exit sounds. The capture animates during loading. Login, server
travel and offline map/recall now remain covered until all required landblocks
and scenery finish uploading, collision is ready, the destination's distant
terrain is installed, and a submitted frame/render fence completes. Closed
dungeons wait only for their connected interior block, not unrelated outdoor
blocks. Sky, lighting and distant terrain continue updating during that hold.
The editor runtime also waits for outstanding first-use shader compilation.

Entry fades the tunnel over an opaque backing, so the new destination cannot
show through during the first second or before the portal actor's first tick.
The player stays held through final placement/camera setup and the initial
network object upload batch. Queued network data, new object/visual revisions
and pending collision invalidate the reveal fence. SceneRenderingComplete is
sent after destination rendering is ready, including subsequent teleports;
Pyrealium's post-acknowledgement inventory/population arrives under portal cover.
A new arrival during the exit flash immediately resumes opaque portal space.
This follows the Legions SmartBox::HandleTeleport / WorldRender sequence in the
local retail reference; the native dome and exact camera/FOV distortion remain
missing. There is no native protocol marker guaranteeing receipt of every
future population update; later live-world updates still stream normally.

`AC2.Render.PortalArrivalReadiness` exercises real DAT dungeon/outdoor streaming,
render-fence readiness, reentry during the exit flash, eventual reveal and the
offline map request. `tests/network/test_map_travel.py --render` verifies login and
server-driven map travel, with captures of both portal cover and the revealed
world. These tests use disposable worlds/accounts.

## Movement and physics

- Native behavior callback thresholds no longer incorrectly hold the last frame
  of ordinary movement actions. Death retains its explicit held end pose.
- Jump animation starts before the physical impulse, using the decoded native
  animation jump phase/delay. Releasing a pending jump cancels it.
- Entering water clears incompatible ground actions and selects the local
  player's native water/swim graph. Incoming ground movement behaviors cannot
  overwrite it while swimming. Remote actors now classify water from replicated feet positions, native body
  height/water thresholds and the local DAT water plane, and select their own
  water/swim graph. They no longer inherit the local player's movement state.
  Swimming does not receive the presentation-only floor correction.
- Falling and landing select their native animation/effect paths.
- Pyrealium now tracks accepted descent and applies damage once on contact
  with DAT-verified terrain, interior BSPs and static-object floors. Drops up to six meters are safe; faster descents over
  six meters scale damage to maximum health, reaching maximum at thirty meters.
  Every verified landing clears the tracked apex and speed, including harmless
  steps and downhill walking. Contact detection must not be gated by the damage
  threshold: doing so accumulated earlier grounded descents into later falls.
  This is an explicit Pyrealium policy, **not a recovered retail formula**. The collision implementation is imported from the pinned Rust reference.
  This remains contact validation, not fully authoritative movement simulation.
  Water friction/lift use native force branches; the Unreal swept-capsule solver
  still supplies local movement. Retail collision transitions, water-entry fall
  cancellation and the exact retail fall-damage formula remain incomplete.

## Native windows and loot

Drag the headers of the inventory/journal side panel, examine side panel, chat,
vitals, toolbar, shortcut bar, shop and loot window to reposition them. Positions
are clamped to the viewport and saved in client settings. The separate radar/map
widgets are not included yet.

The toolbar opens native skills and quest windows. SkillPanel registry
`70001976` supplies 70 panels with their actual node cells, parent lines, icons,
localized labels and specialization/back links. Native tree cell dimensions and
orientation control placement. Training, raising and untraining send the existing
server requests, with credit, prerequisite and XP checks. Displaying a skill does
not implement its gameplay effect: the server still restricts unsupported hero,
faction and advanced class rules. Quest tabs display received active/completed
records; missing story data stays empty instead of being invented.

Double-click a defeated encounter to open the native external-container window.
Double-click an item or use Loot All to collect it. Close/reopen, range and owner
checks, backpack updates and saved claimed items use the server protocol. Native
icons and the loot window are checked through rendered mouse input.

Current ordinary encounter loot is one explicitly authored potion reward. XP and
gold remain automatic, as do tutorial-specific quest rewards. Pending corpse loot
expires after sixty seconds and is not persisted across a server restart; claimed
inventory is persisted. Full retail loot tables, public chest contents and all
native-client container acknowledgements remain incomplete.

## What the placement audit establishes

The read-only DAT scan examined 15,024 entity templates, 3,348 outdoor blocks and
57,013 interiors. It found 20,784 outdoor and 151,868 interior static placements,
but no creature population placements. Encounter descriptor `14000000` has zero
rows in this installation. Five templates could not be parsed completely and are
listed in the report. An AI definition is not evidence for a spawn location.

The selected server pack now contains 1,276 compatible or explicitly authored
objects: 534 encounter candidates, 41 vendors, 48 doors, five chests, seven signs
and 641 other objects. Encounter classification is not proof of hostility. Combat
metadata was regenerated from the installed DATs for 317 entity profiles, using
the upstream candidate list without copying mismatched upstream combat values.

The new pinned capture snapshot has 22,422 observations. Source priority retains
existing IIDs and whole earlier cell snapshots; 6,798 overlapping observations
are withheld instead of stacking different-time captures. The final report also
withholds 14,051 incompatible observations, 1,744 placements missing records and
five duplicate landmarks. Capture names are preserved in spawn and examination
messages. Exact DAT foliage remains unchanged.

Non-service encounter actors can now pursue within their native homesick radius
(where supplied), using native floor and wall checks. Both sides' attacks check
native line of sight. Animation/state and movement updates reach observers and
late arrivals. Three authored tutorial enemies were moved onto verified Shelter
floor positions; captured placements were not relocated. Aggro relationships,
obstacle-routing navigation, roaming, generic attack reach and loot remain authored or
unfinished; no missing retail spawn generator is inferred from an AI template.

Native chest classes open the external-container UI, including an empty result
when no contents are configured; key/quest-restricted chests remain restricted.
The open/closed model state is shared by viewers and sent to late arrivals.
Other visible world objects can be examined through the common interaction path.
This does not execute every native server script or make static art interactive.

### Cell-boundary pursuit and shared chests (2026-10-29)

- Walking steps use native `LandDefs::adjust_to_outside` to rebase block-local
  coordinates, then native LOS traversal and floor queries to resolve the next
  cell. Tests exercise both 20 m cell edges and 160 m block edges against the
  installed Legions DATs. Three body rays and a bounded floor/step check remain;
  this is not the complete retail sphere/capsule movement solver or a pathfinder.
- Moving creatures update the content cell index. Existing observers receive
  position packets, newly interested observers receive CreateObject/current
  stats, and observers losing interest receive DestroyObject. Respawn restores
  the authored home and its index. A two-client UDP test also checks late login.
- Homesickness is checked against the original home before pursuing. On losing
  an eligible target, creatures retrace up to 4,096 verified steps, rechecking
  collision. Returning creatures cannot be attacked. A blocked return waits;
  there is no teleport through obstacles. The 2 m/s chase speed, restoration of
  health on disengagement, and return policy are Pyrealium behavior, not recovered
  retail AI. Aggression/faction policy and obstacle routing remain gaps.
- Unlocked, configured server chest identities publish native open/closed
  modes. Multiple viewers share the state; only the last viewer closes it.
  Logout, death, loss of visibility and leaving the four-metre use radius close
  the viewer's container. Stale close requests do not close another chest.
- The Unreal client resolves native EntityDesc base classes before applying
  door-specific collision changes. Opening a chest preserves its collision;
  reversing open/close state interrupts the previous behavior, including assets
  without a separate close animation. The rendered acceptance test exercises
  the native Order Cache animation/FX alongside door collision and shop input.
- Chest contents and encounter loot tables are unchanged. Empty containers are
  still empty where no supported contents exist; this pass adds no placements,
  invented retail stock, spawn schedules or faction relationships.

Validation: 98 Rust tests passed (four explicitly ignored legacy/converted-DAT
tests), including native geometry and the compatible capture pack. Real UDP
service tests cover shared viewers, logout, late arrivals and range cleanup;
the pursuit test covers two players, block crossing and walking home; corpse
loot tests cover ownership, take/all, replay protection and persisted grants.
The Unreal client builds and the rendered shop/door/chest acceptance passes.

The admin decompile describes object, script and interaction mechanisms. It does
not supply the absent live server population/quest database. A chest, door or sign
rendered as static artwork does not necessarily have a usable server object.
Existing validated server doors and vendors were retested, including replication
to a second player. Unbound scenery doors/signs/chests still require a matching
server identity and behavior/content definition; no visual-name heuristic turns
all matching meshes into interactive objects.

To recover exact spawns and quest bindings, useful inputs are a version-matched
server content database, administrative exports or captures of those locations.
The existing placement/content tools can also create explicitly authored Pyrealium
content, but such entries must not be described as verified retail placements.

Generate the reports without changing the game data:

```powershell
python tools/ac2/population_probe.py "Asheron's Call 2 - Legions" --output Saved/NativeReview/population.json
python tools/ac2/service_audit.py --population Saved/NativeReview/population.json --content data/world/legions-world.tsv --output Saved/NativeReview/services.json
```

`services.json` records each selected object's identity, cell/block, coordinates,
resolved script class and available store/AI properties. Local generated reports
and retail inputs are ignored by Git.

## Supplied SQL archive and upstream snapshot

The user-supplied `5 AC 2 Pyreal SQL Database.7z` was extracted only under ignored
`Saved/PyrealSQL-2017`. The dump SHA256 is
`e70658d4856784ae8a032c81cac2664c014caf009c1cadbbe6387ba5c2e2dd0d`.
The read-only audit found 1,553 telepads, 160 portals, 89 lifestones, 150 player
levels and 1,750 crafting-level rows. Entity, quest, quest-phase, skill and
submission tables are empty. All 160 portals already match `portals.json`, so no
SQL was executed and no existing server data was overwritten. These tables are
useful corroboration and travel/progression references, but not a world spawn DB.

Upstream content is pinned to Pyrealium revision
`468793018b332b87228794b0720ea1ce0400f96d`. See
`content/upstream-content.json` for hashes and import scope. The
native physics dependency remains the existing pinned AC2_Rusted revision; its
sources are now included in the server's corresponding-source archive.

Repeat the SQL check with:

```powershell
python tools/audit_pyreal_sql.py Saved/PyrealSQL-2017/pyreal.sql --output Saved/PyrealSQL-2017/audit.json
```

## Validation

- Wound pass: all 45 AC2 data tests passed. The new tests cover customized
  race/sex profiles, actual bound DAT textures at every stage, native material
  exceptions, blood preferences, health/max ordering, explicit/stale FX,
  appearance rebuilds, healing, death and logout.
- `AC2.Render.ProgressiveDamageAppearance` passed with D3D11. It captures
  human/Drudge clean, five wound stages and healed frames, checking visible pixel
  changes and restoration. Captures are under
  `Unreal/Saved/Screenshots/DamageAppearance/`. The capture test warms first-use
  material shader permutations before measuring pixels.
- `tests/network/test_damage_appearance.py` passed against an isolated real UDP server:
  player and creature current/max health reaches both clients, returning observers
  receive current vitals without another hit, and creature healing reaches both.
- UE 5.8 Editor and game Development targets built successfully.
- All 43 AC2 data automation tests passed, including all 70 skill panels,
  native swim vertex movement, water thresholds, lift and friction.
- This continuation: 85 server library tests passed, one existing ignored test;
  the installed-DAT geometry integration test also passed.
- Bridge tests from the preceding pass: 17 passed.
- Isolated UDP tests passed for falling, corpse loot, gameplay and doors/vendors.
  The fall check verifies a safe small jump, a damaging eighteen-meter descent,
  no repeated charge and saved health after re-entry.
- Corpse tests verify single take, Loot All, close/reopen, foreign-owner and
  distance rejection, no duplicate grant and persistence of claimed inventory.
- The connected rendered smoke passed combat, actual native loot-window use,
  inventory dragging/equipping/drop/pickup, icons and hover interaction checks.
- The journal smoke verified native skill trees/quest pages, routed header dragging
  and a rendered portal-space capture. Creator palette screenshots were inspected.
- Fall accumulation regression: forty separate short drops and a forty-meter
  downhill walk followed by a small drop are harmless. The installed-DAT test
  also walks a native slope from 129.88 to 122.78 meters before a quick contact
  adjustment, verifying no inherited fall damage. Continuous large falls still
  apply damage only once, at verified contact.

Relevant repeatable checks:

```powershell
.\Launch-AC2.ps1 -Mode Test -SkipBuild
python tests/network/test_falling.py
python tests/network/test_corpse_loot.py
python tests/network/test_damage_appearance.py
python tests/network/test_gameplay.py
python tests/network/test_services.py
python tests/network/test_journal_ui.py
python tests/network/test_combat_ui.py --gameplay
```

These integration tests use isolated stores. They do not delete production
accounts or characters. They are functional and rendering checks, not proof that
every race, spell, world object or retail animation is complete.

## Loot, shortcuts, cursor and shoreline continuation (2026-10-30)

- Taking corpse loot from combat temporarily sends peace stance, commits the
  inventory operation, then restores combat. Rejected mutations also restore
  stance; neither path resumes attacks against the corpse. Ownership, range,
  persistence and duplicate-claim checks still apply.
- The corpse panel places its close button inside the right end cap and exposes
  the native Transmute All control alongside Loot All. Transmutation validates
  the native request, journals gold before consuming eligible loot, and leaves
  unsupported/quest items in the container. Prices currently use Pyrealium's bounded
  merchant appraisal policy; retail appraisal tables have not been recovered.
- Learned active skills drag from the book to any of ten shortcut pages. Page
  arrows, numbers 1 through 0, moving bindings and per-character persistence are
  connected. Untrained/untraining skills remain unavailable. Alt and middle
  click no longer toggle persistent mouse capture; held camera gestures remain.
- Six existing placed hostile templates now have combat/AI profiles recovered
  from their inherited DAT properties: Crawlspace Vermin (47001073), Drudge
  Digger (4700158b), Drudge Sentinel (4700158c), Fanatic Juggernaut (47003581),
  Swarm Drone Nymph (47003ad4), and Plains Bull (47003b67). The Volcanic Rift pet
  template was excluded. This fixes classification of existing spawner-managed
  actors; it does not create extra static placements or make town NPCs hostile.

### Explicit shoreline content repair

The user approved extending the adjacent lake into block **74A9**. Its DAT water
map specifies 70.95 m, below all of its terrain (minimum 90.5 m), while adjoining
75A9 and 74AA use 96.7 m. The native lake stops at the block boundary even where
the bank is below that surface. The admin/Legions `CLandBlockData::CacheFarValues`
and admin `LScapeTerrainMesh::UpdateNearWater` read these same per-block values;
they do not interpolate a missing shoreline. This is an inferred content repair,
not a claim that a different retail water algorithm was recovered.

`FAC2WaterSource::At` borrows 75A9's native height for the dry, connected 74A9 bank.
Near/far water rendering and client swimming therefore use one plane. Server
`Geometry::water_height` uses the same adjacent plane so terrestrial spawn
validation rejects submerged ground. The west and south boundaries are above
the lake, and nearby 73A9 remains dry. Existing DAT water wins if a later dataset
restores 74A9. No DAT files or authored scenery placements were edited.

### Verification for this continuation

- Server library: 118 passed, one existing ignored test.
- Installed-DAT geometry: five passed, including repaired water height,
  submerged spawn rejection, and the unrelated western dry block.
- `AC2.Data.RepairedShoreline74A9`: passed, checking both lake seams, native
  material identity, the reported location and dry outside boundaries.
- Offscreen shoreline smoke passed at 9670 cm. Inspected bank and overhead
  captures show the extended water meeting the terrain. Client editor/game,
  network bridge and game-server release builds succeeded; the account service
  was left running on its separate executable.
- Real UDP corpse-loot test: take/Transmute All from combat, stance restoration,
  owner/range/replay rejection and persistent inventory/gold passed.
- Real UDP outdoor-population test: the exact reported Crawlspace Vermin
  placement accepts combat with its native 81 health; pursuit/retaliation and
  protected town NPC checks passed.
- Rendered journal smoke: learned-skill drag, slot move, page arrows/indicator,
  train/raise/untrain and quest tabs passed. Shortcut smoke restores bindings
  and suppresses preference writes while exercising the test page.
- Rendered combat/mouse smoke: combat, corpse pickup, close-button placement,
  both loot controls, visible cursor, inert Alt/middle toggle and held camera
  gestures passed. The wrapper's old empty-corpse assertion was updated to also
  accept its already-supported populated-loot path after variable loot was added.

For a focused offscreen shoreline capture, launch UnrealEditor with the project,
`-game -RenderOffscreen -unattended -nosound -PyrealiumnvironmentSmoke
-AC2ShorelineSmoke -AC2Block=74A9 -AC2Radius=2 -AC2Hour=12` and `-AC2DatDir`.
The smoke checks the streamed swimming surface, then saves bank and overhead
views under `Unreal/Saved/Screenshots/Pyrealiumnvironment-shoreline-*.png`.
