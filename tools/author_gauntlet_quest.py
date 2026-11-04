#!/usr/bin/env python3
"""Write the Gauntlet (Cragstone Excavation, 32.4N 7.2W) as quest database rows, family `gauntlet`.

    python3 -I tools/author_gauntlet_quest.py content/gauntlet-content-v1.json OUT.sql
    cargo run -p pyrealium --bin pyrealium-content -- import-sql OUT.sql gauntlet

Everything is ordinary engine data (`quest_emotes`): aliases for the NPCs, potion and hub; one
instance group of eight arenas (lanes) with a spawn slot per fight room (`gauntlet-world-v1.tsv`
holds the 128 slot placements, `examples/gauntlet_authoring.rs` placed them); a `gauntlet_fight`
template used once per fight; the Master's start, reward and early-exit rows; and the death,
logout and expiry rows that end a run. Edit the rows in the database afterwards as any quest's,
or change this script and import again (an import replaces only the `gauntlet` family).

Evidence (`gauntlet-content-v1.json` `sources`): the DAT gives the Master (0x4700326A, lines in
table 0x25007E77), the portal (0x4700326B, 0x25007E78), the quest strings (0x25007E61) and the 16
opponents in fight order (their death effects `Eff_AdvanceQuestGauntlet<bracket>Phase<n>`, each
bracket starting further along one chain); the rooms are admin 7148's `dng_gauntlet_<level><lane>`
destinations; AC2Hero and 20oz.net give the rules (level 20, 2 hours, 22 hour reset, 2% XP, 25k
coins, the Frenzy Potion; leaving early pays for the opponents beaten, dying pays nothing).
Pyrealium policy (no record of the retail server): the stage is the run's next fight (1-16; 17
all beaten, 18 rewarded), the bracket by level at the start, a run takes the first free arena at
the portal, the player is carried on 5 seconds after a kill (time to loot) and back to the hub
after the last, an early exit pays 2% XP and 25,000 coins times the share of the bracket's fights
won (two decimals of a percent), a logout frees the arena (the portal takes the player back to
its fight while time remains), and a death or the time running out ends the run unpaid.
"""
import json
import math
import sys

FAMILY = "gauntlet"
QUEST = "gauntlet"
GROUP = "gauntlet_arenas"
SLOT_BASE = 0x8140_6a75_0000_0000
MASTER_LINES, PORTAL_LINES = "0x25007e77", "0x25007e78"
WELCOME, FLED, PATHETIC, SUCCESS = "0x2a6b091e", "0x2968fd1e", "0xa96499a4", "0xb8cd58f5"
UNWORTHY, NOTHING_NOW, NOTHING_HAPPENS = "0xc4f9188e", "0x63691d8f", "0x30b9998d"
CARRY_MS = 5000
REWARD_PERCENT, REWARD_GOLD = 2, 25_000
SRC = "content/gauntlet-content-v1.json"
POLICY = "Pyrealium policy (tools/author_gauntlet_quest.py)"
STAGES = [
    (1, 0, "Enter the portal and defeat your first opponent to move on.", "DAT 7230 quest table 0x25007e61 key 0x2e5c1e52"),
    *[(n, 0, "Defeat your next opponent to move on. You may refuse the challenge and return to the Master of the Gauntlet to "
       "claim a lesser reward.", "DAT 7230 quest table 0x25007e61 key 0x2e231e52") for n in range(2, 16)],
    (16, 0, "You have just one more opponent to defeat!", "DAT 7230 quest table 0x25007e61 key 0x64c560ba"),
    (17, 0, "Return to the Master of the Gauntlet to claim your reward.", "DAT 7230 quest table 0x25007e61 key 0x8e21dd9d"),
    (18, 1, "You have finished the Gauntlet.", "Pyrealium journal text (no retail line is recovered): the run was rewarded"),
]


def q(v):
    return "NULL" if v is None else (str(v) if isinstance(v, int) else "'" + str(v).replace("'", "''") + "'")


def place(room):
    """`cell x y z heading` of a destination (quaternion (w, x, y, z) about z; heading clockwise
    from +Y, 255 a turn)."""
    w, z = room["quaternion"][0], room["quaternion"][3]
    turn = (-2.0 * math.atan2(z, w)) % math.tau
    heading = round(turn / math.tau * 255.0) % 255
    x, y, h = room["origin"]
    return f"{room['cell']} {x:.6f} {y:.6f} {h:.6f} {heading}"


def main():
    data = json.load(open(sys.argv[1], encoding="utf-8"))
    out = []

    def row(table, **cols):
        cols = {**cols, "family": FAMILY}
        out.append(f"INSERT INTO {table} ({', '.join(cols)}) VALUES ({', '.join(q(v) for v in cols.values())});")

    def action(s, order, name, *p, delay=0, source=POLICY):
        p = list(p) + [""] * (5 - len(p))
        row("emote_action", emote_set=s, ord=order, action=name, delay_ms=delay, p1=p[0], p2=p[1], p3=p[2], p4=p[3], p5=p[4], source=source)

    priority = [50_000]

    def eset(name, trigger, owner=None, stages=None, source=POLICY):
        priority[0] += 10
        lo, hi = stages if stages else (None, None)
        row("emote_set", name=name, priority=priority[0], owner_entity=owner, trigger=trigger, quest=QUEST if stages else None,
            stage_min=lo, stage_max=hi, source=source)

    fights = data["opponents"]
    brackets = data["brackets"]
    hub = place(data["hub"])
    row("quest", name=QUEST, journal_id="0x400200a1", title_table="0x25007e61", title_key="0xa8607cec", title="Gauntlet",
        credit="killer", min_level=20, time_limit_seconds=7200, repeat_seconds=79200,
        source="Title: DAT 7230 quest table 0x25007e61 key 0xa8607cec; journal id: Pyrealium 0x4002_0000 | AC2 Vault quests/db id 161 "
               "(no DAT quest record); level 20, 2 h, 22 h reset: AC2Hero quest/osteth_championgauntlet.html and 20oz.net The_Gauntlet "
               "(user choice 2026-11-03 over the Vault's 1 day)")
    for stage, done, text, source in STAGES:
        row("quest_stage", quest=QUEST, stage=stage, completed=done, journal_text=text, source=source)
    row("quest_counter", quest=QUEST, name="bracket", idx=0, source=f"{POLICY}: the run's level bracket, 1 to 6 ({SRC} brackets)")
    for name, kind, value, source in [
        ("gauntlet_master", "entity", "0x" + data["master"], "DAT 7230 entity 0x4700326a, Master of the Gauntlet"),
        ("gauntlet_portal", "entity", "0x" + data["portal"], "DAT 7230 entity 0x4700326b, Gauntlet Portal"),
        ("frenzy_potion", "entity", "0x" + data["frenzy_potion"], "DAT 7230 entity 0x4700326f, Frenzy Potion (the Vault, AC2Hero, 20oz.net)"),
        ("gauntlet_hub", "placement", hub, f"{SRC} hub: admin 7148 destination dng_hum_3a_gauntlet"),
    ]:
        row("alias", name=name, kind=kind, value=value, source=source)

    # Eight arenas; each fight room of each arena holds its opponent's slot.
    row("instance_group", name=GROUP, doc="The Gauntlet's eight arenas (lanes a-h), one run each.", source=f"{SRC} rooms")
    for lane, letter in enumerate("abcdefgh"):
        row("instance_lane", instance_group=GROUP, lane=lane, entry=place(data["rooms"][f"{fights[0]['level']}{letter}"]),
            source=f"{SRC} rooms {fights[0]['level']}{letter}: the arena's first room")
    for n, opponent in enumerate(fights, 1):
        for lane, letter in enumerate("abcdefgh"):
            room = f"{opponent['level']}{letter}"
            row("spawn_slot", name=f"fight_{n}", instance_group=GROUP, lane=lane, iid=f"0x{SLOT_BASE | (opponent['level'] << 8) | lane:016x}",
                arrival=place(data["rooms"][room]), source=f"gauntlet-world-v1.tsv; arrival {SRC} rooms {room} (dng_gauntlet_{room})")

    # One fight: the portal carries a run to its fight room and summons the opponent; the kill
    # moves the run on and, after the loot time, carries the player to the next room.
    t = "gauntlet_fight"
    row("template", name=t, doc="One fight of the Gauntlet: the portal to its room, its opponent's kill.", source=POLICY)
    for order, name in enumerate(["n", "next", "opponent"], 1):
        row("template_param", template=t, name=name, ord=order, source=POLICY)
    def tset(name, prio, trigger, owner=None, stages=(None, None)):
        row("template_set", template=t, name=name, priority=prio, owner_entity=owner, trigger=trigger,
            quest=QUEST if stages[0] else None, stage_min=stages[0], stage_max=stages[1], source=POLICY)
    def tact(s, order, name, *p, delay="0"):
        p = list(p) + [""] * (5 - len(p))
        row("template_action", template=t, emote_set=s, ord=order, action=name, delay_ms=delay, p1=p[0], p2=p[1], p3=p[2], p4=p[3], p5=p[4], source=POLICY)
    tset("portal", 1, "Use", "gauntlet_portal", ("$n", "$n"))
    tset("enter", 2, "Goto")
    tset("full", 3, "Goto")
    tset("kill", 4, "Kill", "$opponent", ("$n", "$n"))
    tact("portal", 1, "ClaimInstance", GROUP, "enter", "full")
    tact("enter", 1, "MoveToSlot", "fight_$n")
    tact("enter", 2, "Generate", "fight_$n")
    tact("full", 1, "Message", "Every arena of the Gauntlet is in use. Try again shortly.")
    tact("kill", 1, "SetStage", QUEST, "$next")
    tact("kill", 2, "MoveToSlot", "fight_$next", delay=str(CARRY_MS))
    tact("kill", 3, "Generate", "fight_$next")
    for n, opponent in enumerate(fights[:-1], 1):
        use = f"gauntlet_fight_{n}"
        row("template_use", name=use, template=t, priority=60_000 + n * 10,
            source=f"fight {n}: 7230 entity 0x{opponent['entity']} ({opponent['name']}, level {opponent['level']}), {SRC} opponents")
        for param, value in [("n", n), ("next", n + 1), ("opponent", "0x" + opponent["entity"])]:
            row("template_arg", use_name=use, param=param, value=str(value), source=POLICY)
    # The last fight: its kill brings the player back to the hub and frees the arena.
    last = len(fights)
    eset("gauntlet_portal_last", "Use", "gauntlet_portal", (last, last))
    action("gauntlet_portal_last", 1, "ClaimInstance", GROUP, "gauntlet_enter_last", "gauntlet_full")
    eset("gauntlet_enter_last", "Goto")
    action("gauntlet_enter_last", 1, "MoveToSlot", f"fight_{last}")
    action("gauntlet_enter_last", 2, "Generate", f"fight_{last}")
    eset("gauntlet_full", "Goto")
    action("gauntlet_full", 1, "Message", "Every arena of the Gauntlet is in use. Try again shortly.")
    eset("gauntlet_kill_last", "Kill", "0x" + fights[-1]["entity"], (last, last),
         source=f"fight {last}: 7230 entity 0x{fights[-1]['entity']} ({fights[-1]['name']}, level {fights[-1]['level']})")
    action("gauntlet_kill_last", 1, "SetStage", QUEST, last + 1)
    action("gauntlet_kill_last", 2, "MoveTo", "gauntlet_hub", delay=CARRY_MS)
    action("gauntlet_kill_last", 3, "ReleaseInstance", GROUP)

    # The Master: a start (by level bracket), the reward, or an early exit.
    tell = lambda s, o, key, src: action(s, o, "Tell", MASTER_LINES, key, source=f"DAT 7230 Master of the Gauntlet table 0x25007e77 key {key}{src}")
    eset("gauntlet_start", "Use", "gauntlet_master", (0, 0))
    action("gauntlet_start", 1, "Goto", "gauntlet_offer")
    eset("gauntlet_again", "Use", "gauntlet_master", (18, 18))
    action("gauntlet_again", 1, "Goto", "gauntlet_offer")
    eset("gauntlet_offer", "Goto")
    action("gauntlet_offer", 1, "InqStat", "level", 20, "", "gauntlet_level_ok", "gauntlet_unworthy", source="level 20: AC2Hero, 20oz.net")
    eset("gauntlet_unworthy", "Goto")
    tell("gauntlet_unworthy", 1, UNWORTHY, "")
    eset("gauntlet_level_ok", "Goto")
    action("gauntlet_level_ok", 1, "InqQuestReady", QUEST, "gauntlet_bracket_6", "gauntlet_not_now")
    eset("gauntlet_not_now", "Goto")
    tell("gauntlet_not_now", 1, NOTHING_NOW, ": before the 22 hour reset")
    for k in range(len(brackets), 0, -1):
        b = brackets[k - 1]
        check = f"gauntlet_bracket_{k}"
        eset(check, "Goto")
        if k > 1:
            action(check, 1, "InqStat", "level", b["min_level"], "", f"gauntlet_begin_{k}", f"gauntlet_bracket_{k - 1}",
                   source=f"bracket {b['name']} ({SRC} brackets)")
        else:
            action(check, 1, "Goto", f"gauntlet_begin_{k}")
        begin = f"gauntlet_begin_{k}"
        eset(begin, "Goto")
        tell(begin, 1, WELCOME, "")
        action(begin, 2, "SetStage", QUEST, b["first_fight"], source=f"bracket {b['name']} starts at fight {b['first_fight']} ({SRC} brackets)")
        # After the stage: starting a run clears its counters.
        action(begin, 3, "SetCounter", QUEST, "bracket", k)
    eset("gauntlet_reward", "Use", "gauntlet_master", (last + 1, last + 1))
    tell("gauntlet_reward", 1, SUCCESS, ": every foe defeated")
    action("gauntlet_reward", 2, "GiveItem", "frenzy_potion", 1, source="Frenzy Potion: the Vault, AC2Hero and 20oz.net")
    action("gauntlet_reward", 3, "AwardLevelProportionalXP", REWARD_PERCENT, source="20oz.net The_Gauntlet: '2% XP'; AC2Hero: '% Based XP'")
    action("gauntlet_reward", 4, "AwardGold", REWARD_GOLD, source="20oz.net The_Gauntlet: '25k Coins'")
    action("gauntlet_reward", 5, "SetStage", QUEST, last + 2)
    action("gauntlet_reward", 6, "ReleaseInstance", GROUP)
    eset("gauntlet_pathetic", "Goto")
    tell("gauntlet_pathetic", 1, PATHETIC, ": no opponent beaten")
    action("gauntlet_pathetic", 2, "ReleaseInstance", GROUP)
    action("gauntlet_pathetic", 3, "EraseQuest", QUEST)
    # An early exit at stage s pays for the bracket's fights won (s - first of 17 - first).
    for s in range(1, last + 1):
        eset(f"gauntlet_exit_{s}", "Use", "gauntlet_master", (s, s))
        action(f"gauntlet_exit_{s}", 1, "Goto", f"gauntlet_exit_{s}_b{len(brackets)}")
        for k in range(len(brackets), 0, -1):
            first = brackets[k - 1]["first_fight"]
            check = f"gauntlet_exit_{s}_b{k}"
            pay = f"gauntlet_exit_{s}_pay{k}"
            eset(check, "Goto")
            if k > 1:
                action(check, 1, "InqCounter", QUEST, "bracket", k, pay, f"gauntlet_exit_{s}_b{k - 1}")
            else:
                action(check, 1, "Goto", pay)
            won, total = s - first, last + 1 - first
            eset(pay, "Goto")
            if won <= 0:
                action(pay, 1, "Goto", "gauntlet_pathetic")
                continue
            percent = max(0.01, math.floor(REWARD_PERCENT * won / total * 100) / 100)
            tell(pay, 1, FLED, f": an early exit, {won} of {total} fights won")
            action(pay, 2, "AwardLevelProportionalXP", f"{percent:g}", source=f"{POLICY}: 2% XP times {won}/{total}")
            action(pay, 3, "AwardGold", REWARD_GOLD * won // total, source=f"{POLICY}: 25,000 coins times {won}/{total}")
            action(pay, 4, "SetStage", QUEST, last + 2)
            action(pay, 5, "ReleaseInstance", GROUP)

    # A run ends by death or time unpaid; a logout frees the arena (the portal takes the player back).
    eset("gauntlet_died", "PlayerDeath", None, (1, last + 1))
    action("gauntlet_died", 1, "Message", "You have fallen in the Gauntlet. Your run is over.")
    action("gauntlet_died", 2, "ReleaseInstance", GROUP)
    action("gauntlet_died", 3, "EraseQuest", QUEST)
    eset("gauntlet_late", "QuestExpired", None, (1, last + 1))
    action("gauntlet_late", 1, "Message", "Your two hours in the Gauntlet are over. Your run is over.", source="2 hours: AC2Hero, 20oz.net")
    action("gauntlet_late", 2, "ReleaseInstance", GROUP)
    action("gauntlet_late", 3, "EraseQuest", QUEST)
    eset("gauntlet_left", "Logout", None, (1, last + 1))
    action("gauntlet_left", 1, "ReleaseInstance", GROUP)
    # The portal outside a run.
    priority[0] = 99_000
    eset("gauntlet_portal_idle", "Use", "gauntlet_portal")
    action("gauntlet_portal_idle", 1, "Tell", PORTAL_LINES, NOTHING_HAPPENS, source="DAT 7230 Gauntlet Portal table 0x25007e78: 'Nothing happens.'")

    with open(sys.argv[2], "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print(f"{len(out)} rows")


if __name__ == "__main__":
    main()
