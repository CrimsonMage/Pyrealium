# Quest actions

Generated from `crates/pyrealium/src/quest_emotes/spec.rs` (`pyrealium-content actions --markdown`); a test keeps it in step. Each `emote_action` row names one action in `action` and its parameters in p1..p5, in this order; an optional parameter (in brackets) may be left empty, and the columns after the last parameter stay empty. `delay_ms` waits that long before the action runs (the rest of the set waits with it).

| action | parameters | what it does |
|---|---|---|
| `Tell` | p1 table: hex (0x...)<br>p2 key: hex (0x...) | Says a DAT string (StringInfo table and key) to the player as the NPC's chat line. |
| `Message` | p1 text: text | Says Pyrealium text to the player, where retail has no DAT line. |
| `AwardXP` | p1 xp: whole number, at least 1 | Gives experience. |
| `AwardLevelProportionalXP` | p1 percent: percent (up to 2 decimals)<br>[p2 min: whole number, at least 0]<br>[p3 max: whole number, at least 0] | Gives a percent of the experience the player's current level spans, at least min and at most max (empty: no limit). |
| `AwardGold` | p1 gold: whole number, at least 1 | Gives coins. |
| `GiveItem` | p1 item: entity DID (0x47...)<br>p2 count: whole number 1-16 | Gives items (one bag slot each). The whole set fails when the bag is full. |
| `TakeItem` | p1 item: entity DID (0x47...)<br>p2 count: whole number 1-16 | Takes items from the bag. The whole set fails when the player has fewer. |
| `RandomGive` | p1 items: 0x47...:weight,... | Gives one item drawn by weight. |
| `AwardKingdomPoints` | p1 points: whole number, at least 1<br>[p2 max: whole number, at least 0] | Adds Kingdom Points, up to max (empty or 0: no cap). |
| `SetKingdom` | p1 kingdom: whole number 0-3 | Sets the player's kingdom: 1 Order, 2 Dominion, 3 Shadow, 0 none. |
| `ApplyEffect` | p1 effect: Effect state DID (0x6f...)<br>p2 seconds: whole number 1-31536000 | Puts an Effect state on the player for a time (a quest's curse or blessing); it shows on the effect strip. |
| `RemoveEffect` | p1 effect: Effect state DID (0x6f...) | Takes an Effect state off the player. |
| `SetStage` | p1 quest: quest name<br>p2 stage: stage (0-255) | Moves the quest to a stage and prints its journal line. Leaving stage 0 (or a completed stage) starts a run: refused below the quest's min_level, before its repeat delay, or past its solve limit. Reaching a completed stage solves it. |
| `EraseQuest` | p1 quest: quest name | Resets the quest to stage 0: its counters and run start go; its solves stay (they count for repeats). |
| `IncrementQuest` | p1 quest: quest name | Counts one more on the quest's kill counter, up to its goal. |
| `IncrementCounter` | p1 quest: quest name<br>p2 counter: counter (0-255)<br>p3 cap: whole number 1-65535 | Counts one more on a counter, up to cap. |
| `DecrementCounter` | p1 quest: quest name<br>p2 counter: counter (0-255) | Counts one less on a counter (not below 0). |
| `SetCounter` | p1 quest: quest name<br>p2 counter: counter (0-255)<br>p3 value: whole number 0-65535 | Sets a counter. |
| `InqStage` | p1 quest: quest name<br>p2 min: stage (0-255)<br>p3 max: stage (0-255)<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether the quest's stage is in min..max. |
| `InqQuestCount` | p1 quest: quest name<br>p2 count: whole number 0-65535<br>[p3 success set: set name]<br>[p4 failure set: set name] | Whether the quest's kill counter reached count. |
| `InqCounter` | p1 quest: quest name<br>p2 counter: counter (0-255)<br>p3 count: whole number 0-65535<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether a counter reached count. |
| `InqItem` | p1 item: entity DID (0x47...)<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player has the item in the bag. |
| `InqOwnsItems` | p1 item: entity DID (0x47...)<br>p2 count: whole number 1-1000<br>[p3 success set: set name]<br>[p4 failure set: set name] | Whether the player has at least count of the item in the bag. |
| `InqLevel` | p1 quest: quest name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player's level reaches the quest's min_level. |
| `InqQuestReady` | p1 quest: quest name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether a new run of the quest may start now (its repeat delay and solve limit). |
| `InqSolved` | p1 quest: quest name<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the quest was ever solved (a prerequisite). |
| `InqQuestSolves` | p1 quest: quest name<br>p2 min: whole number 0-65535<br>[p3 max: whole number 0-65535]<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether the quest's solve count is in min..max (empty max: no upper bound). |
| `InqStat` | p1 stat: stat (level, gold, health, vigor, kingdom, kingdom_points)<br>p2 min: whole number, at least 0<br>[p3 max: whole number, at least 0]<br>[p4 success set: set name]<br>[p5 failure set: set name] | Whether a player stat is in min..max (empty max: no upper bound). |
| `InqKingdom` | p1 kingdom: whole number 0-3<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player is in the kingdom (0: in any). |
| `InqKingdomPoints` | p1 points: whole number, at least 0<br>[p2 success set: set name]<br>[p3 failure set: set name] | Whether the player has at least points Kingdom Points. |
| `Goto` | p1 set: set name | Runs another set's actions, then carries on with this one's. |
| `MoveTo` | p1 cell: hex (0x...)<br>p2 x: number<br>p3 y: number<br>p4 z: number<br>p5 heading: stage (0-255) | Moves the player to a place (heading 0-255, clockwise from north). |
| `StartEscort` | p1 creature: entity DID (0x47...) | The nearest free creature of the entity starts following the player. |
| `EndEscort` | none | The player's escorts stop following. |
