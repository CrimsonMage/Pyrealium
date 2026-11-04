# Quest actions

Generated from `crates/pyrealium/src/quest_emotes/spec.rs` (`pyrealium-content actions --markdown`); a test keeps it in step. Each `emote_action` row names one action in `action` and its parameters in p1..p5, in this order; an optional parameter (in brackets) may be left empty, and the columns after the last parameter stay empty. `delay_ms` waits that long before the action runs (the rest of the set waits with it). Where a parameter takes a DID, an `alias` row's name may stand for it; MoveTo takes a placement alias in p1 alone; a counter may be named by its `quest_counter` row. A `template_use` fills a template's `$parameters` (`content/README.md`).

| action | parameters | what it does |
|---|---|---|
| `Tell` | p1 table: hex (0x...) or alias<br>p2 key: hex (0x...) or alias | Says a DAT string (StringInfo table and key): the NPC's speech, or a chat line when no NPC speaks. |
| `Message` | p1 text: text | Says Pyrealium text to the player, where retail has no DAT line. |
| `Say` | p1 text: text<br>[p2 speaker: text] | The NPC says the text: a speech bubble over it and the dialogue window, titled with the speaker (else the NPC's name). |
| `AwardXP` | p1 xp: whole number, at least 1 | Gives experience. |
| `AwardLevelProportionalXP` | p1 percent: percent (up to 2 decimals)<br>[p2 min: whole number, at least 0]<br>[p3 max: whole number, at least 0] | Gives a percent of the experience the player's current level spans, at least min and at most max (empty: no limit). |
| `AwardGold` | p1 gold: whole number, at least 1 | Gives coins. |
| `AwardXPShare` | p1 percent: percent (up to 2 decimals)<br>p2 quest: quest name<br>p3 part: counter (0-255, or its quest_counter name)<br>p4 whole: counter (0-255, or its quest_counter name) | Gives the percent of the level's experience span times counter part over counter whole of the quest (a share of a reward: fights won of fights). |
| `AwardGoldShare` | p1 gold: whole number, at least 1<br>p2 quest: quest name<br>p3 part: counter (0-255, or its quest_counter name)<br>p4 whole: counter (0-255, or its quest_counter name) | Gives the coins times counter part over counter whole of the quest. |
| `GiveItem` | p1 item: entity DID (0x47...) or entity alias<br>p2 count: whole number 1-16 | Gives items (one bag slot each). The whole set fails when the bag is full. |
| `TakeItem` | p1 item: entity DID (0x47...) or entity alias<br>p2 count: whole number 1-16 | Takes items from the bag. The whole set fails when the player has fewer. |
| `RandomGive` | p1 items: 0x47...:weight,... | Gives one item drawn by weight. |
| `GiveStack` | p1 item: entity DID (0x47...) or entity alias<br>p2 quantity: whole number 1-65535 | Gives the quantity of a stackable item as one stack (merged into one the bag holds when it fits). |
| `TakeQuantity` | p1 item: entity DID (0x47...) or entity alias<br>p2 quantity: whole number 1-65535 | Takes the quantity of a stackable item from the bag's stacks. The whole set fails when the player has less. |
| `GrantScene` | p1 scene: hex (0x...) or alias | Grants a story scene (the Quest panel's Story page). |
| `LearnRecipe` | p1 recipe: hex (0x...) or alias | Teaches a recipe: a recipe any quest teaches can be crafted only once learned. |
| `AwardKingdomPoints` | p1 points: whole number, at least 1<br>[p2 max: whole number, at least 0] | Adds Kingdom Points, up to max (empty or 0: no cap). |
| `AwardQuestPoints` | p1 points: whole number, at least 1 | Adds Quest Points (the 2002-2003 quest reward the AC2 Vault records): kept by the server for later use; no client shows them. InqStat quest_points reads them. |
| `SetKingdom` | p1 kingdom: whole number 0-3 | Sets the player's kingdom: 1 Order, 2 Dominion, 3 Shadow, 0 none. |
| `ApplyEffect` | p1 effect: Effect state DID (0x6f...) or effect alias<br>p2 seconds: whole number 1-31536000 | Puts an Effect state on the player for a time (a quest's curse or blessing); it shows on the effect strip. |
| `RemoveEffect` | p1 effect: Effect state DID (0x6f...) or effect alias | Takes an Effect state off the player. |
| `SetStage` | p1 quest: quest name<br>p2 stage: stage (0-255) | Moves the quest to a stage and prints its journal line. Leaving stage 0 (or a completed stage) starts a run: refused below the quest's min_level, before its repeat delay, or past its solve limit. Reaching a completed stage solves it. |
| `IncrementStage` | p1 quest: quest name | SetStage to the quest's current stage plus one (with SetStage's start and solve rules and journal line). |
| `EraseQuest` | p1 quest: quest name | Resets the quest to stage 0: its counters and run start go; its solves stay (they count for repeats). |
| `IncrementQuest` | p1 quest: quest name | Counts one more on the quest's kill counter, up to its goal. |
| `IncrementCounter` | p1 quest: quest name<br>p2 counter: counter (0-255, or its quest_counter name)<br>p3 cap: whole number 1-65535 | Counts one more on a counter, up to cap. |
| `DecrementCounter` | p1 quest: quest name<br>p2 counter: counter (0-255, or its quest_counter name) | Counts one less on a counter (not below 0). |
| `SetCounter` | p1 quest: quest name<br>p2 counter: counter (0-255, or its quest_counter name)<br>p3 value: whole number 0-65535 | Sets a counter. |
| `InqStage` | p1 quest: quest name<br>p2 min: stage (0-255)<br>p3 max: stage (0-255)<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether the quest's stage is in min..max. |
| `InqQuestCount` | p1 quest: quest name<br>p2 count: whole number 0-65535<br>[p3 success set: set name]<br>[p4 failure set: set name] | Whether the quest's kill counter reached count. |
| `InqCounter` | p1 quest: quest name<br>p2 counter: counter (0-255, or its quest_counter name)<br>p3 count: whole number 0-65535<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether a counter reached count. |
| `InqItem` | p1 item: entity DID (0x47...) or entity alias<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player has the item in the bag. |
| `InqOwnsItems` | p1 item: entity DID (0x47...) or entity alias<br>p2 count: whole number 1-1000<br>[p3 success set: set name]<br>[p4 failure set: set name] | Whether the player has at least count of the item in the bag. |
| `InqLevel` | p1 quest: quest name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player's level reaches the quest's min_level. |
| `InqQuestReady` | p1 quest: quest name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether a new run of the quest may start now (its repeat delay and solve limit). |
| `InqSolved` | p1 quest: quest name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the quest was ever solved (a prerequisite). |
| `InqQuestSolves` | p1 quest: quest name<br>p2 min: whole number 0-65535<br>[p3 max: whole number 0-65535]<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether the quest's solve count is in min..max (empty max: no upper bound). |
| `InqStat` | p1 stat: stat (level, gold, health, vigor, kingdom, kingdom_points, quest_points)<br>p2 min: whole number, at least 0<br>[p3 max: whole number, at least 0]<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether a player stat is in min..max (empty max: no upper bound). |
| `InqKingdom` | p1 kingdom: whole number 0-3<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player is in the kingdom (0: in any). |
| `InqKingdomPoints` | p1 points: whole number, at least 0<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player has at least points Kingdom Points. |
| `Goto` | p1 set: set name | Runs another set's actions, then carries on with this one's. |
| `ClaimInstance` | p1 group: instance group name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Takes a free lane of the instance group for the player (success set), or finds none free (failure set). A player already holding a lane of it succeeds. |
| `ReleaseInstance` | p1 group: instance group name | Frees the player's lane of the group and sends its slots' creatures away. |
| `Generate` | p1 slot: spawn slot name | Summons the slot's creature in the player's lane of its group (an earlier one's corpse goes first); nothing without a lane. |
| `DismissSlots` | p1 group: instance group name | Sends away the creatures, alive or dead, of the group's slots in the player's lane. |
| `MoveToLane` | p1 group: instance group name | Moves the player to the entry of its lane of the group. |
| `GenerateStep` | p1 group: instance group name<br>p2 quest: quest name | Generate for the group's slot whose step (spawn_slot.step) is the quest's current stage: a chain of rooms or waves by stage. |
| `MoveToStep` | p1 group: instance group name<br>p2 quest: quest name | MoveToSlot for the group's slot whose step is the quest's current stage. |
| `MoveToSlot` | p1 slot: spawn slot name | Moves the player to the slot's arrival point in its lane (spawn_slot.arrival: a room of its own per slot). |
| `MoveTo` | p1 cell: hex (0x...) or alias<br>p2 x: number<br>p3 y: number<br>p4 z: number<br>p5 heading: stage (0-255) | Moves the player to a place (heading 0-255, clockwise from north). |
| `StartEscort` | p1 creature: entity DID (0x47...) or entity alias | The nearest free creature of the entity starts following the player. |
| `EndEscort` | none | The player's escorts stop following. |
