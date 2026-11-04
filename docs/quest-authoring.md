# Writing quests and NPC speech

Every quest and every line an NPC says is a row in the quest database, so you can write them
without touching Rust. The server's code holds only the generic behaviours that rows name:
triggers, actions and checks. If a quest needs something the rows can't express, ask for a new
generic behaviour that any quest could reuse. Don't special-case the quest in code.

The column reference is in [`content/README.md`](../content/README.md). Every action and its
parameters is listed in [`quest-actions.md`](quest-actions.md).

## Where the rows live

- `content/quests.sql` is the checked-in copy: one `INSERT` per row, so a review or a merge
  works line by line.
- Edit them as a database:
  - `cargo run -p pyrealium --bin pyrealium-content -- build` makes `content/quests.sqlite`.
  - Edit it in the world builder's **Quests** tab, or in any SQLite tool.
  - `... -- check` checks every row the way the server does.
  - `... -- export` writes `quests.sql` back.
- In the world builder, a world's own quests are `NAME.quests.sql` beside `NAME.tsv`.
  - **Quests > Open quests** opens them as `NAME.quests.sqlite`.
  - **Save** commits.
  - With **Live server** on, every save is pushed to the running server, which checks the rows
    and runs them at once. Only your edits are kept, as a layer over the shipped quests.

## The shape of a quest

- **`quest`**: its journal entry, plus `min_level`, `time_limit_seconds`, `max_solves` and
  `repeat_seconds`.
- **`quest_stage`**: the journal line for each stage. A stage marked `completed` solves the quest.
- **`emote_set`**: who reacts to what.
  - The owner: an NPC or object (`owner_entity` / `owner_iid`, or an alias name).
  - The trigger, an optional item or `phrase`, and the quest stage range it applies at.
  - Among the sets whose owner, trigger and stages match, the first by `priority` (then name)
    runs.
- **`emote_action`**: what that set does, in order (`ord`). `delay_ms` waits before a step.

### Example: a kill quest

- **`offer`**: Use on the giver, at stage 0.
  - `InqLevel quest offer_ok weak` checks the level.
  - Then `SetStage quest 1` and `Tell <giver's table> 0x2ae6122c` (the offer line).
- **`weak`**: a Goto set that runs `Tell <table> 0x68f492a3` ("come back when you are stronger").
- **`kill`**: Kill on the creature, at stage 1.
  - `IncrementQuest quest` counts the kill.
  - `InqQuestCount quest 10 goal` checks the count.
- **`goal`**: a Goto set that runs `SetStage quest 2`.
- **`busy`**: Use on the giver, at stage 1. It runs `Tell <table> 0xcde80fb2` ("not done yet").
- **`claim`**: Use on the giver, at stage 2.
  - `AwardLevelProportionalXP 2.5 0 52892362` pays the reward.
  - `SetStage quest 3` (completed).
  - `Tell <table> 0x68a07c4d` speaks the reward line.

Shapes that repeat across many quests can be written once as a **template** (`template_*`
tables), then given one `template_use` row per quest.

## Speech: DAT lines or your own text

Choose per row:

- **A DAT line, by reference.** `Tell table key` speaks a line from the NPC's own string table.
  The row stores only the two IDs; the client shows the retail text in the player's language.
  - In the world builder, open the action and press **Pick DAT line…**. It lists the owner NPC's
    lines from the live server and fills in table, key and a quote in `source`.
  - Without the editor: `INDEX/dat_index.sqlite` holds the same lines.
  - The live server answers `GET /admin/strings?entity=0x47...` or `?table=0x25...`.
- **Your own text.**
  - `Say text [speaker]` gives a speech bubble and the dialogue window.
  - `Message text` gives a chat line.
  - Use these where no DAT line fits. A row's text can be edited like any other value.

Many NPC tables share the same keys:

| key | line |
|---|---|
| `0x2ae6122c` / `0x2ae6124e` / `0x2ae61248` / `0x2ae6124a` | offer: the first to fourth task |
| `0x68a07c4d` / `0x68a04d4d` / `0x68a04e4d` / `0x68a04f4d` | reward |
| `0xcde80fb2` / `0xcdeb1fb2` / `0xcdeb2fb2` / `0xcdeb3fb2` | not done yet |
| `0x09200efe` | no task today |
| `0x68f492a3` | come back when stronger |
| `0xa964fc6c` | come back tomorrow (repeat wait) |
| `0x6c459364` | the NPC's description (examination, not speech) |

### What makes an NPC say which line

- **Quest stage (quest stamps):** give the set `quest` with `stage_min` / `stage_max`. Or branch
  inside a set with `InqStage quest min max yes no`, `InqSolved`, `InqQuestReady` or
  `InqQuestSolves`.
- **Level and stats:** `InqLevel quest yes no` checks the quest's `min_level`. `InqStat level 20 -
  yes no` checks any range, and also gold, health, vigor, kingdom and kingdom points.
- **Items held:** `InqOwnsItems item count yes no`.
- **Variety:** several sets with the same owner and trigger, with `probability`. The probability
  is cumulative against one roll. For four lines, give the first three 0.25, 0.5 and 0.75 and
  leave the last empty.
- **Topics:** an `Ask` set with a `phrase` answers `@ask <phrase>`, or `@mentor <phrase>`, from a
  player within four metres. An Ask set with no phrase answers any other topic.
- **Small talk:** a `Talk` set is the NPC's own conversation. It runs when a use finds no quest
  set, merchant or service, and on `@talk`.

## Checking your work

- The database refuses the plainest mistakes as you type: an unknown trigger, a phrase on a
  non-Ask set, a stage past 255, a missing quest.
- `pyrealium-content check` (or a live push) runs the server's whole check. It reports each
  error by table, row and column: unknown actions, wrong parameters, Goto targets that don't
  exist, rewards in sets that may only change quests and talk.
- `cargo test -p pyrealium` plays the shipped hunts, obelisk searches, Arwic lesson, Gambling
  Debt, the Gauntlet and the NPC topics through the engine.
