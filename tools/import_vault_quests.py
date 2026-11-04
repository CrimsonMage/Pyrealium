#!/usr/bin/env python3
"""Import the AC2 Vault's quest library as drafts, resolve them against the DATs and Pyrealium's world,
and write the quest/emote table rows for the quests today's engine can run.

  python3 -I tools/import_vault_quests.py \
      --vault  ../../../third_party/ac2vault-data/ac2vault.sqlite \
      --dat-index ../../../INDEX/dat_index.sqlite \
      --world  ~/.local/state/pyrealium-test/world-2516529122-play.tsv \
      --content content --drafts reports/vault-quests.drafts.json \
      --report reports/vault-quests-coverage.md --ship content \
      [--overrides content/vault-quest-overrides.tsv] [--gaps vault-quest-gaps.tsv]
  (from emulator of a checkout under the analysis workspace's third_party/pyrealium-src/)

Inputs (all read only; the Vault database is untrusted third-party data, so run with -I from
outside its directory):
  --vault      AC2 Vault quests (title, requirements, timers, reward, walkthrough prose), with its
               creature/trophy/location links (`ac2vault-docker/README.md`).
  --dat-index  the workspace DAT index (7230): entity and string-table names, every string.
  --world      the prepared Pyrealium world table the server runs (iid, entity, visual, cell, x, y, z).
  --content    the emulator's content/: definitions.tsv (entity display names),
               native-quests.json (the DAT's quest journal ids, names, phases and the creatures
               whose death effects advance a quest), the older hand-coded catalogs (overlaps).

Outputs:
  --drafts  one structured draft per Vault quest: parsed fields, every resolved field with its
            confidence and source, the engine gaps that block it and its catalog overlaps.
  --report  the coverage report (markdown).
  --ship    the content directory whose quest database (`quests.sql`) receives the rows, as its
            `vault` family: they are written as the four `<table>-vault.tsv` files into a
            temporary directory and imported with `pyrealium-content import-tsv DIR vault
            CONTENT`, which replaces only the vault family's rows (the tool is
            `$PYREALIUM_CONTENT`, else `cargo run -p pyrealium --bin pyrealium-content`).
            Rows are generated, never hand
            edited.

Evidence ranking (workspace rule): client/DAT, then AC2 Vault, then AC2Hero, then Pyrealium policy.
Nothing here invents behaviour: a field no source gives stays unresolved, and a quest ships only
when every field it needs resolves and today's engine has every feature it needs.
"""
import argparse
import difflib
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict

NAME_KEY = 0xA8607CEC        # StringInfo key of an object's name in its own table (DAT convention)
DESC_KEY = 0x6C459364        # ... and of its description
LAND_CELLS = 1020            # LandDefs: map coordinate = (global land cell - 1020) / 1020 * 100

# ---------------------------------------------------------------------------------------------
# small helpers


def key(name):
    """A name's comparison key: lower case, markers ([E], [mn], (Group)) and articles dropped."""
    n = (name or "").lower().replace("&", " and ")
    n = re.sub(r"\[[^\]]*\]", "", n)
    n = re.sub(r"\((group|boss|elite|named|unique|rare|quest|solo)[^)]*\)", "", n)
    n = re.sub(r"^(the|a|an)\s+", "", n.strip())
    return re.sub(r"[^a-z0-9]+", "", n)


PACKAGE_NOISE = {"quest", "the", "a", "an", "of", "s"}


def package_key(name):
    """A quest package (QuestDilloRustlers) or title (Dillo Rustlers) as comparable words."""
    n = re.sub(r"([a-z])([A-Z])", r"\1 \2", name or "")
    n = n.lower().replace("'", "")
    return "".join(w for w in re.split(r"[^a-z0-9]+", n) if w and w not in PACKAGE_NOISE)


VERBS = {"kill", "slay", "fracture", "destroy", "exterminate", "dispatch", "perforate", "puncture", "deflate",
         "eliminate", "drive", "out", "crush", "cull", "eradicate", "hunt", "defeat", "boss", "the", "quest", "of", "a"}


def package_words(name):
    n = re.sub(r"([a-z])([A-Z])", r"\1 \2", name or "").lower().replace("'", "")
    out = set()
    for w in re.split(r"[^a-z0-9]+", n):
        if not w or w in VERBS:
            continue
        for suffix, repl in (("ies", "y"), ("s", "")):
            if w.endswith(suffix) and len(w) > 4:
                w = w[: -len(suffix)] + repl
                break
        out.add(w)
    return frozenset(out)


def norm_text(t):
    t = (t or "").lower().replace("’", "'").replace("`", "'")
    t = re.sub(r"%[a-z0-9_]+%?|\$\w+|<[^>]*>", " ", t)
    return re.sub(r"[^a-z0-9]+", " ", t).strip()


def plural_keys(name):
    """The keys a (possibly plural) creature or item name may stand for."""
    k = key(name)
    out = [k]
    for suffix, repl in (("ies", "y"), ("es", ""), ("s", ""), ("men", "man")):
        if k.endswith(suffix) and len(k) > len(suffix) + 2:
            out.append(k[: -len(suffix)] + repl)
    return list(dict.fromkeys(out))


COORD = re.compile(r"(\d{1,2}(?:\.\d+)?)\s*([NnSs])\s*(?:[,x/]|and)?\s*(\d{1,3}(?:\.\d+)?)\s*([EeWw])\b")


def coords(text):
    out = []
    for m in COORD.finditer(text or ""):
        n = float(m.group(1)) * (1 if m.group(2).lower() == "n" else -1)
        e = float(m.group(3)) * (1 if m.group(4).lower() == "e" else -1)
        if abs(n) <= 102 and abs(e) <= 102:
            out.append({"north": n, "east": e, "text": m.group(0), "at": m.start()})
    return out


def map_of_cell(cell, x, y):
    """Map (north, east) of a placement; interior cells give their landblock's centre."""
    bx, by = (cell >> 24) & 0xFF, (cell >> 16) & 0xFF
    if (cell & 0xFFFF) >= 0x100:
        gx, gy, exact = bx * 8 + 4, by * 8 + 4, False
    else:
        gx, gy, exact = bx * 8 + x / 20.0, by * 8 + y / 20.0, True
    return round((gy - LAND_CELLS) / LAND_CELLS * 100, 2), round((gx - LAND_CELLS) / LAND_CELLS * 100, 2), exact


def map_to_cell(north, east):
    """The outdoor cell and in-block offset of a map coordinate (inverse of `map_of_cell`)."""
    gy = north / 100 * LAND_CELLS + LAND_CELLS
    gx = east / 100 * LAND_CELLS + LAND_CELLS
    bx, by = int(gx // 8), int(gy // 8)
    cx, cy = int(gx - bx * 8), int(gy - by * 8)
    return {"cell": f"0x{bx:02x}{by:02x}{cx * 8 + cy + 1:04x}", "x": round((gx - bx * 8) * 20, 1),
            "y": round((gy - by * 8) * 20, 1)}


def dist(a, b):
    return ((a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2) ** 0.5


# ---------------------------------------------------------------------------------------------
# time and reward parsing


def seconds(text):
    """Seconds of a Vault duration, None when unknown, and a note when the text is ambiguous."""
    t = (text or "").strip()
    if not t or t.lower().startswith("unknown") or t.lower() == "none":
        return None, "unknown"
    if "in-game" in t.lower():
        return None, "in-game hours (no real-time length given)"
    days = re.search(r"([\d.]+)\s*day", t, re.I)
    hours = re.search(r"([\d.]+)\s*hour", t, re.I)
    if "days hours" in t.lower():
        hours = None
    if not days and not hours:
        return None, f"unparsed: {t}"
    d = float(days.group(1)) if days else 0.0
    h = float(hours.group(1)) if hours else 0.0
    note = None
    if days and d != int(d) and hours:
        note = "ambiguous: fractional days plus hours"
    return int(round(d * 86400 + h * 3600)), note


def number(s):
    return int(s.replace(",", ""))


def parse_reward(text):
    r = {"xp": None, "xp_percent": None, "xp_cap": None, "qp": None, "kp": None, "gold": None,
         "items": [], "other": [], "raw": text or ""}
    lines = [l.strip() for l in (text or "").splitlines() if l.strip()]
    for line in lines:
        low = line.lower()
        m = re.match(r"^([\d,]+)\s*(xp|experience)\b", low)
        if m:
            r["xp"] = number(m.group(1))
            continue
        if re.search(r"\bxp\b|experience|percentage", low) and re.search(r"%|percent|capped|maximum|minimum|@", low):
            # level-proportional experience: '3% XP', '.5% of XP needed to level. Minimum of 200,000 XP',
            # 'Percentage XP - 775,334,738 @ 73' (an observation, not a rule)
            m = re.search(r"(\d*\.?\d+)\s*%", low)
            if m:
                r["xp_percent"] = float(m.group(1))
            c = re.search(r"(?:capped(?: @| at)?|maximum of)\s*([\d,]{4,})", low)
            if c:
                r["xp_cap"] = number(c.group(1))
            mn = re.search(r"minimum of\s*([\d,]{4,})", low)
            if mn:
                r["xp_min"] = number(mn.group(1))
            o = re.search(r"([\d,]{5,}|[\d.]+\s*(?:million|mil|billion))\s*(?:xp\s*)?@\s*(?:level\s*)?(\d+)", low)
            if o:
                r.setdefault("xp_observed", []).append(o.group(0))
            if not m:
                r["xp_percent_unknown"] = True
            continue
        m = re.match(r"^([\d,]+)\s*qp\b", low)
        if m:
            r["qp"] = number(m.group(1))
            continue
        m = re.search(r"([\d,]+)\s*(kingdom points|kp)\b", low)
        if m:
            r["kp"] = number(m.group(1))
            continue
        m = re.match(r"^([\d,]+)\s*(g|gold|pyreals?)\b", low)
        if m:
            r["gold"] = number(m.group(1))
            continue
        r["other"].append(line)
    return r


# ---------------------------------------------------------------------------------------------
# body sections


SECTIONS = ["Quest index", "Last Updated:", "Requirements:", "Time Limit:", "Reset Timer:", "Reward:",
            "Misc Notes:", "Description:", "Starting the Quest:", "Quest Walkthrough:",
            "Encountered Monsters:", "Associated Items:", "Associated Information/Links:"]


def sections(body):
    out = {}
    marks = []
    for s in SECTIONS:
        i = (body or "").find(s)
        if i >= 0:
            marks.append((i, s))
    marks.sort()
    for n, (i, s) in enumerate(marks):
        end = marks[n + 1][0] if n + 1 < len(marks) else len(body)
        out[s.rstrip(":")] = body[i + len(s):end].strip()
    return out


QUOTE = re.compile(r"[\"“]([^\"”]{12,})[\"”]")

TALK = re.compile(r"\b(?i:speak(?:ing)?|talk(?:ing)?|return(?:ing)?|go back|report)\s+(?:back\s+)?(?:to|with)\s+"
                  r"(?:the\s+|an?\s+)?([A-Z][\w'\-]*(?:[ \-](?:the\s+|of\s+|de\s+)?[A-Z][\w'\-]*){0,4})")
KILL = re.compile(r"\b(?i:kill|slay|defeat|destroy|hunt)\s+(?:(\d+|a|an|one|two|three|four|five|six|seven|eight|nine|ten|twenty|the)\s+)?"
                  r"(?:of\s+the\s+)?([A-Z][\w'\-]*(?:[ \t]+[A-Z][\w'\-]*){0,4})")
LOOT = re.compile(r"\b(?i:loot|collect|retrieve|obtain|find|gather|bring(?:\s+back)?)\s+(?:(\d+|a|an|one|two|three|four|five|the|some|any)\s+)?"
                  r"([A-Z][\w'\-]*(?:[ \t]+(?:of[ \t]+)?[A-Z][\w'\-]*){0,4})")
GIVE = re.compile(r"\b(?i:give|hand|return|bring|turn in|deliver)\s+(?:the\s+|your\s+|it\s+|them\s+|this\s+)?"
                  r"([A-Z][\w'\-]*(?:[ \t]+(?:of[ \t]+)?[A-Z][\w'\-]*){0,4})?\s*(?:back\s+)?to\s+(?:the\s+)?"
                  r"([A-Z][\w'\-]*(?:[ \t]+[A-Z][\w'\-]*){0,3})")
USE = re.compile(r"\b(?i:use|double-click|place|insert)\s+(?:the\s+|your\s+)?([A-Z][\w'\-]*(?:[ \t]+[A-Z][\w'\-]*){0,3})"
                 r"(?:\s+(?:on|at|in|into)\s+(?:the\s+)?([A-Z][\w'\-]*(?:[ \t]+[A-Z][\w'\-]*){0,3}))?")
WORDS = {"a": 1, "an": 1, "one": 1, "the": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7,
         "eight": 8, "nine": 9, "ten": 10, "twenty": 20}

FEATURE_WORDS = [
    ("fellowship", re.compile(r"\bfellow(ship)?\b", re.I)),
    ("kingdom", re.compile(r"\bkingdom\b|\bfaction\b|\ballegiance\b|\bhero of dereth\b|must be a hero", re.I)),
    ("random_reward", re.compile(r"\brandom(ly)?\b|one of (a few|several|the following)|varies", re.I)),
    ("timed_order", re.compile(r"in the order", re.I)),
    ("crafting", re.compile(r"\brecipe\b|\bcraft(ed|ing)?\b|\btradeskill|\bspellbinding\b", re.I)),
    ("chest_or_door", re.compile(r"\bchest\b|\bdoor\b|\blever\b|\bswitch\b", re.I)),
    # an NPC or creature that walks with the player (the audit: "protect" alone is mostly a kill
    # or a timed hold, below)
    ("escort", re.compile(r"\bescort(s|ed|ing)?\b|\bfollows? (you|him|her)\b", re.I)),
    # something the player spawns or plants and must keep alive for a time
    ("defend", re.compile(r"\b(spawn|plant)\w*\b.{0,120}?\bprotect (it|them|the [\w ]{1,30}?) (from|for|until)\b", re.I | re.S)),
    ("pet", re.compile(r"\bsummon(s|ed)?\b|\bpet\b", re.I)),
]


def steps_of(text):
    """Ordered step hints from walkthrough prose: talk, kill, loot, give, use, travel."""
    out = []
    t = QUOTE.sub(lambda m: " " * len(m.group(0)), text or "")  # quotes are dialogue, not steps
    for m in TALK.finditer(t):
        out.append({"kind": "talk", "npc": m.group(1).strip(), "at": m.start()})
    for m in KILL.finditer(t):
        n = m.group(1)
        count = int(n) if n and n.isdigit() else WORDS.get((n or "a").lower(), 1)
        out.append({"kind": "kill", "count": count, "target": m.group(2).strip(), "at": m.start()})
    for m in LOOT.finditer(t):
        n = m.group(1)
        count = int(n) if n and n.isdigit() else WORDS.get((n or "a").lower(), 1)
        out.append({"kind": "loot", "count": count, "item": m.group(2).strip(), "at": m.start()})
    for m in GIVE.finditer(t):
        out.append({"kind": "give", "item": (m.group(1) or "").strip() or None, "npc": m.group(2).strip(), "at": m.start()})
    for m in USE.finditer(t):
        out.append({"kind": "use", "item": m.group(1).strip(), "target": (m.group(2) or "").strip() or None, "at": m.start()})
    for c in coords(t):
        out.append({"kind": "travel", "north": c["north"], "east": c["east"], "text": c["text"], "at": c["at"]})
    out.sort(key=lambda s: s["at"])
    return out


def list_lines(text):
    lines = [l.strip() for l in (text or "").splitlines() if l.strip()]
    return [l for l in lines if l.lower() not in ("item description",)]


# ---------------------------------------------------------------------------------------------
# reference data


class Data:
    def __init__(self, a):
        self.content = a.content
        dat = sqlite3.connect(f"file:{a.dat_index}?mode=ro", uri=True)
        self.dat = dat
        # entity display names: Pyrealium's definitions.tsv, then the DAT's own name strings
        self.entity_name = {}
        self.entity_marker = {}   # the name's markers ([mn] a named NPC, [E] ...)
        self.creature_entities = set()
        for fn in ("creature-combat.tsv",):
            for line in open(os.path.join(a.content, fn), encoding="utf-8"):
                f = line.split("\t")
                if re.fullmatch(r"[0-9a-f]{8}", f[0]):
                    self.creature_entities.add(int(f[0], 16))
        self.entity_file = {}
        self.by_name = defaultdict(set)
        self.table_of_entity = {}
        for line in open(os.path.join(a.content, "definitions.tsv"), encoding="utf-8"):
            f = line.rstrip("\n").split("\t")
            if len(f) >= 3 and re.fullmatch(r"[0-9a-f]{8}", f[0]):
                e = int(f[0], 16)
                self.entity_name[e] = re.sub(r"\[[^\]]*\]", "", f[2]).strip()
                self.entity_marker[e] = "".join(re.findall(r"\[([^\]]*)\]", f[2])).lower()
        tables = {name: did for did, name in dat.execute("SELECT did, name FROM names WHERE kind='STRING_TABLE'")}
        self.table_name = {did: name for name, did in tables.items()}
        names7230 = dict(((t, k), s) for t, k, s in dat.execute(
            "SELECT table_did, string_id, text FROM strings WHERE release='7230' AND string_id=?", (NAME_KEY,)))
        for did, name in dat.execute("SELECT did, name FROM names WHERE kind='ENTITYDESC'"):
            self.entity_file[did] = name
            stem = re.sub(r"\.entity$", "", name)
            stem = re.sub(r"^(wsl|npc)_", "", stem)
            t = tables.get(stem + ".stt")
            if t is not None:
                self.table_of_entity[did] = t
                n = names7230.get((t, NAME_KEY))
                if n and did not in self.entity_name:
                    self.entity_name[did] = re.sub(r"\[[^\]]*\]", "", n).strip()
                if n and did not in self.entity_marker:
                    self.entity_marker[did] = "".join(re.findall(r"\[([^\]]*)\]", n)).lower()
        for e, n in self.entity_name.items():
            self.by_name[key(n)].add(e)
        # effects: eff_<x>.stt's name string, and the WSTATE eff_<x>.wc it belongs to
        wstate = {name: did for did, name in dat.execute("SELECT did, name FROM names WHERE kind='WSTATE'")}
        self.effects = defaultdict(list)
        for (t, _), n in names7230.items():
            tn = self.table_name.get(t, "")
            if tn.startswith("eff_") and tn.endswith(".stt"):
                stem = tn[:-4]
                w = wstate.get(stem + ".wc")
                clean = re.sub(r"\[[^\]]*\]", "", n).strip()
                if clean and len(clean) >= 5:
                    self.effects[key(clean)].append({"name": clean, "table": t, "wstate": w, "file": stem})
        # world placements
        self.placements = defaultdict(list)
        self.placements_by_iid = {}
        self.world_path = a.world
        for line in open(os.path.expanduser(a.world), encoding="utf-8"):
            f = line.rstrip("\n").split("\t")
            if len(f) < 7 or not re.fullmatch(r"[0-9a-f]{16}", f[0]):
                continue
            ent, cell = int(f[1], 16), int(f[3], 16)
            n, e, exact = map_of_cell(cell, float(f[4]), float(f[5]))
            self.placements_by_iid[f"0x{f[0]}"] = ent
            self.placements[ent].append({"iid": f"0x{f[0]}", "entity": ent, "cell": f"0x{cell:08x}", "north": n,
                                         "east": e, "exact": exact})
        # creatures the other Pyrealium world tables and camps place (packs a prepared world may leave out)
        self.pack_entities = defaultdict(set)
        for fn in sorted(os.listdir(a.content)):
            path = os.path.join(a.content, fn)
            if re.search(r"world.*\.tsv$", fn):
                for line in open(path, encoding="utf-8"):
                    f = line.split("\t")
                    if len(f) > 3 and re.fullmatch(r"[0-9a-fA-F]{16}", f[0]) and re.fullmatch(r"[0-9a-fA-F]{8}", f[1]):
                        self.pack_entities[int(f[1], 16)].add(fn)
            elif re.search(r"-content-v\d+\.json$", fn):
                d = json.load(open(path, encoding="utf-8"))
                for camp in (d.get("camps") or []) if isinstance(d, dict) else []:
                    for e in set(camp.get("roster") or []):
                        self.pack_entities[e].add(fn + " camps")
        # the DAT's quests: journal id, package, name, phases; creatures whose deaths advance them
        nq = json.load(open(os.path.join(a.content, "native-quests.json"), encoding="utf-8"))
        self.native = {}
        self.native_by_key = defaultdict(list)
        self.native_by_package = {}
        for q in nq["quests"]:
            nm = q.get("name") or {}
            text = nm.get("text")
            if not text and nm.get("table"):
                row = dat.execute("SELECT text FROM strings WHERE release='7230' AND table_did=? AND string_id=?",
                                  (nm["table"], nm["key"])).fetchone()
                text = row[0] if row else None
            text = re.sub(r"\[[^\]]*\]|<hidden>", "", text or "").strip()
            q["_name"] = text
            self.native[q["id"]] = q
            if text:
                self.native_by_key[key(text)].append(q["id"])
            self.native_by_package[q.get("package") or ""] = q["id"]
        self.death = defaultdict(list)   # quest id -> [(entity, effect)]
        for b in nq.get("entity_bindings") or []:
            for d in b.get("death_effects") or []:
                if d.get("quest"):
                    self.death[d["quest"]].append((b["entity"], d.get("effect")))
        # the older hand-coded catalogs: quest ids, titles, NPC entities
        self.catalog_quest = defaultdict(list)     # journal id -> [catalog]
        self.catalog_title = defaultdict(list)     # title key -> [catalog]
        self.catalog_quest_rows = defaultdict(list)  # title key -> [(catalog, quest)]
        self.catalog_hunts = {}                      # hunting catalog -> its quests
        self.catalog_npc = defaultdict(list)       # NPC entity -> [catalog]
        for fn in sorted(os.listdir(a.content)):
            if not re.match(r"(hunting|exploration|inferred)-content-v\d+\.json$|npc-dialogue-v\d+\.json$", fn):
                continue
            d = json.load(open(os.path.join(a.content, fn), encoding="utf-8"))
            src = fn[:-5]
            for q in d.get("quests") or []:
                if isinstance(q, dict):
                    if "id" in q:
                        self.catalog_quest[q["id"]].append(src)
                    if q.get("title"):
                        self.catalog_title[key(q["title"])].append(src)
                        self.catalog_quest_rows[key(q["title"])].append((src, q))
                        if src.startswith("hunting"):
                            self.catalog_hunts.setdefault(src, []).append(q)
                    if q.get("npc"):
                        self.catalog_npc[q["npc"]].append(src + " (quest NPC)")
            for n in (d.get("npcs") or []) + (d.get("profiles") or []):
                if isinstance(n, dict) and n.get("entity"):
                    self.catalog_npc[n["entity"]].append(src + (" (dialogue)" if "profiles" in d else " (NPC)"))
        src_dir = os.path.join(os.path.dirname(os.path.abspath(a.content)), "src")
        for rs in ("arwic_mines.rs", "training_kit.rs", "recovered_quests.rs", "exploration_quests.rs", "hunting_quests.rs"):
            p = os.path.join(src_dir, rs)
            if os.path.exists(p):
                for m in re.finditer(r"0x(40[0-9a-fA-F]{6})\b", open(p, encoding="utf-8").read()):
                    i = int(m.group(1), 16)
                    if i in self.native and rs[:-3] not in self.catalog_quest[i]:
                        self.catalog_quest[i].append(rs[:-3])
        # the shipped base tables: quests and owners already data
        self.base_owners = set()
        p = os.path.join(a.content, "quests.sql")
        if os.path.exists(p):
            db = sqlite3.connect(":memory:")
            db.executescript(open(p, encoding="utf-8").read())
            for (owner,) in db.execute("SELECT owner_entity FROM emote_set WHERE family <> 'vault' AND owner_entity IS NOT NULL"):
                self.base_owners.add(int(owner, 16))
            db.close()

    # --- lookups -----------------------------------------------------------------------------
    def entities(self, name):
        out = set()
        for k in plural_keys(name):
            out |= self.by_name.get(k, set())
        return out

    def string(self, table, k):
        row = self.dat.execute("SELECT text FROM strings WHERE release='7230' AND table_did=? AND string_id=?",
                               (table, k)).fetchone()
        return row[0] if row else None

    def find_quote(self, quote, prefer_tables=()):
        """The 7230 string (table, key) whose text is this quote, by FTS then similarity."""
        words = norm_text(quote).split()
        if len(words) < 3:
            return None
        best = None
        tried = set()
        for start in (0, max(0, len(words) // 2 - 3), max(0, len(words) - 7)):
            phrase = " ".join(words[start:start + 7])
            if phrase in tried:
                continue
            tried.add(phrase)
            q = '"' + phrase.replace('"', "") + '"'
            try:
                rows = self.dat.execute(
                    "SELECT s.table_did, s.string_id, s.text FROM strings_fts f JOIN strings s ON s.rowid=f.rowid "
                    "WHERE strings_fts MATCH ? AND s.release='7230' LIMIT 40", (q,)).fetchall()
            except sqlite3.OperationalError:
                continue
            for t, k, text in rows:
                tn = self.table_name.get(t, "")
                if re.search(r"_(deutsch|francais|japanese|korean|english)\.stt", tn):
                    continue  # localisation copies; the client reads the base table
                a, b = norm_text(quote), norm_text(text)
                ratio = difflib.SequenceMatcher(None, a, b, autojunk=False).ratio()
                # the Vault often quotes only part of a line
                contained = a in b or b in a
                score = max(ratio, 0.9 if contained and min(len(a), len(b)) >= 25 else 0)
                bonus = 0.05 if t in prefer_tables else 0
                cand = (score + bonus, score, t, k, text)
                if best is None or cand[0] > best[0]:
                    best = cand
        if not best or best[1] < 0.75:
            return None
        _, score, t, k, text = best
        return {"table": f"0x{t:08x}", "key": f"0x{k:08x}", "table_name": self.table_name.get(t),
                "dat_text": text, "similarity": round(score, 3),
                "confidence": "high" if score >= 0.9 else "medium",
                "source": f"DAT 7230 string table 0x{t:08x} key 0x{k:08x} by text match ({score:.2f})"}

    def find_title(self, title):
        """A 7230 string whose whole text is the quest's title (for a Pyrealium journal entry)."""
        words = norm_text(title).split()
        if not words:
            return None
        try:
            rows = self.dat.execute(
                "SELECT s.table_did, s.string_id, s.text FROM strings_fts f JOIN strings s ON s.rowid=f.rowid "
                "WHERE strings_fts MATCH ? AND s.release='7230' LIMIT 200", ('"' + " ".join(words) + '"',)).fetchall()
        except sqlite3.OperationalError:
            return None
        hits = [(t, k, x) for t, k, x in rows if norm_text(re.sub(r"\[[^\]]*\]", "", x)) == " ".join(words)
                and not re.search(r"_(deutsch|francais|japanese|korean|english)\.stt", self.table_name.get(t, ""))]
        if not hits:
            return None
        hits.sort(key=lambda h: (not self.table_name.get(h[0], "").startswith("quest"), h[1] != NAME_KEY, h[0]))
        t, k, x = hits[0]
        return {"table": f"0x{t:08x}", "key": f"0x{k:08x}", "table_name": self.table_name.get(t), "text": x,
                "confidence": "high" if self.table_name.get(t, "").startswith("quest") else
                              ("medium" if self.table_name.get(t) is None or k == NAME_KEY else "low"),
                "source": f"DAT 7230 string 0x{t:08x}/0x{k:08x} equal to the Vault title"}

    def place(self, ents, near=None, radius=1.0):
        """The placements of `ents`, nearest `near` (north, east) first."""
        ps = [p for e in ents for p in self.placements.get(e, [])]
        if near:
            for p in ps:
                p["_d"] = round(dist((p["north"], p["east"]), near), 3)
            ps.sort(key=lambda p: p["_d"])
            ps = [p for p in ps if p["_d"] <= (radius if p["exact"] else radius + 0.8)]
        return ps


# ---------------------------------------------------------------------------------------------
# drafting and resolving one quest


NPC_START = [
    re.compile(r"\b(?i:speak|talk|talking|speaking)\s+(?i:to|with)\s+(?:the\s+)?([A-Z][\w'\-]*(?:[ \-](?:of\s+the\s+|the\s+|of\s+)?[A-Z][\w'\-]*){0,4})"),
    re.compile(r"\b(?i:locate|find|visit)\s+(?:the\s+)?([A-Z][\w'\-]*(?:[ \t]+[A-Z][\w'\-]*){0,3})\s+(?:at|in|near)\b"),
    re.compile(r"\b(?i:bring)\s+.+?\s+to\s+(?:the\s+)?([A-Z][\w'\-]*(?:[ \t]+[A-Z][\w'\-]*){0,3})"),
]
KILL_LINE = re.compile(r"(?i:kill|slay|defeat|death)[^.]{0,100}(?i:message|you see|you receive the|you will see|text)", re.S)
TELLS = re.compile(r"([A-Z][\w'\-]*(?:[ ,\-]+[A-Z][\w'\-]*){0,5})\s+tells you[,:]?\s*[\"“]?", re.M)


# ---------------------------------------------------------------------------------------------
# user overrides (content/vault-quest-overrides.tsv)

OVERRIDE_HEADER = "vault_id\tfield\tkey\tvalue\tsource"
OVERRIDE_FIELDS = {
    "start_npc": "key `entity` (an EntityDesc DID; its one placement, or the one nearest the Vault's start "
                 "coordinates) or `iid` (a placement in the prepared world)",
    "kill_target": "key: the target's name as the walkthrough has it (or `*` for the quest's only one); value: "
                   "EntityDesc DIDs, comma-separated",
    "reward_item": "key: the reward's name; value: `0x47...` or `0x47...,count`",
    "xp": "key `fixed` (value: experience) or `percent` (value: percent of the level's span, optionally "
          "`,cap`)",
    "quote": "key: the quote's number (1 = first in the draft); value: `table:key`, both hex",
    "prerequisite": "value: the Vault id or title of the quest that must be solved first",
    "step": "key: its order (1, 2, ...); value: `talk NPC`, `kill N X`, `loot ITEM`, `give ITEM to NPC`, "
            "`use ITEM on X`, `open OBJECT`, `craft ITEM`, `effect 0x6f... SECONDS`, `lift 0x6f...` or "
            "`travel N.N[NS] E.E[EW]`; any step row replaces the parsed steps",
    "item": "key: an item's name in the steps or the reward; value: its EntityDesc DID",
    "npc": "key: an NPC's name in the steps; value: `0x47...` (its placement) or a 16-digit placement iid",
    "object": "key: a world object's name in the steps (a chest, a door, a target of `use`); value: a 16-digit "
              "placement iid, or `0x47...` when it is placed once",
    "chest": "key: the object `open` names; value: what it gives, `0x47...[,count]` separated by `;`",
    "random_reward": "value: `0x47...:weight` separated by `,` (one item drawn by weight)",
    "credit": "value `fellowship` (the killer's fellows are credited too) or `killer`",
    "drop_reward": "key `qp` or `kp`; value `yes`: the Vault's quest/kingdom points are not awarded",
    "ignore": "key: a flagged feature the walkthrough only mentions (chest, crafting, random, fellowship, pet, "
              "kingdom, escort, defend); value `yes`",
}
OVERRIDES = {}
IGNORABLE = {"chest": "chest/door/lever interaction", "crafting": "crafting step", "random": "random reward choice",
             "fellowship": "fellowship credit", "pet": "summoned pet reward", "kingdom": "kingdom/faction/hero status",
             "escort": "escort/protect", "defend": "defend a spawned object"}


def hexval(text, kinds):
    v = int(text, 16) if re.fullmatch(r"0x[0-9a-fA-F]{1,16}", text or "") else None
    return v if v is not None and (v >> 24 if v < 1 << 32 else None) in kinds else None


STEP_GRAMMAR = [
    ("talk", re.compile(r"^talk\s+(?P<npc>.+)$", re.I)),
    ("kill", re.compile(r"^kill\s+(?P<count>\d+)\s+(?P<target>.+)$", re.I)),
    ("loot", re.compile(r"^loot\s+(?P<item>.+)$", re.I)),
    ("give", re.compile(r"^give\s+(?P<item>.+?)\s+to\s+(?P<npc>.+)$|^give\s+(?P<item2>\S+(?:\s\S+)*?)\s+(?P<npc2>[A-Z][^ ]*(?:\s[A-Z][^ ]*)*)$", re.I)),
    ("use", re.compile(r"^use\s+(?P<item>.+?)\s+on\s+(?P<target>.+)$", re.I)),
    ("travel", re.compile(r"^travel\s+(?P<where>.+)$", re.I)),
    ("open", re.compile(r"^open\s+(?P<target>.+)$", re.I)),
    ("craft", re.compile(r"^craft\s+(?P<item>.+)$", re.I)),
    ("effect", re.compile(r"^effect\s+(?P<effect>0x6f[0-9a-fA-F]{6})\s+(?P<seconds>\d+)$", re.I)),
    ("lift", re.compile(r"^lift\s+(?P<effect>0x6f[0-9a-fA-F]{6})$", re.I)),
    ("escort", re.compile(r"^escort\s+(?P<npc>.+)$", re.I)),
    ("arrive", re.compile(r"^arrive\s+(?P<target>.+?)(?:\s+(?P<range>\d+(?:\.\d+)?)\s*m)?$", re.I)),
]


# The radius of an `arrive` step's Enter set when the step names none (Pyrealium policy: no source
# gives the retail radius).
ARRIVE_RANGE = 10.0
ESCORT_ABANDONED = ("You left your escort behind too long. The quest has failed; you may start it again.",
                    "Pyrealium text and policy: an escort out of its player's radar for three minutes gives up "
                    "and the quest fails as for its death (the server's escort rule, encounters/escort.rs)")
ESCORT_LOST = ("Your escort has fallen. The quest has failed; you may start it again.",
               "Pyrealium text and policy: the escort's death fails the quest and it may start over at once "
               "(as the AC2 Vault's Mosswart Rescue walkthrough, id 87, says of its escort)")


def parse_step(text):
    """One hand-written step as the parsed steps are: None when the grammar does not take it."""
    for kind, rx in STEP_GRAMMAR:
        m = rx.match(text.strip())
        if not m:
            continue
        g = {k: v for k, v in m.groupdict().items() if v}
        if kind == "talk":
            return {"kind": "talk", "npc": g["npc"]}
        if kind == "kill":
            return {"kind": "kill", "count": int(g["count"]), "target": g["target"]}
        if kind == "loot":
            return {"kind": "loot", "count": 1, "item": g["item"]}
        if kind == "give":
            return {"kind": "give", "item": g.get("item") or g.get("item2"), "npc": g.get("npc") or g.get("npc2")}
        if kind == "use":
            return {"kind": "use", "item": g["item"], "target": g["target"]}
        if kind in ("open", "craft"):
            return {"kind": kind, "target" if kind == "open" else "item": g.get("target") or g.get("item")}
        if kind == "effect":
            return {"kind": "effect", "effect": g["effect"].lower(), "seconds": int(g["seconds"])}
        if kind == "lift":
            return {"kind": "lift", "effect": g["effect"].lower()}
        if kind == "escort":
            return {"kind": "escort", "npc": g["npc"]}
        if kind == "arrive":
            return {"kind": "arrive", "target": g["target"], "range": float(g.get("range") or ARRIVE_RANGE)}
        c = coords(g["where"])
        if len(c) == 1:
            return {"kind": "travel", "north": c[0]["north"], "east": c[0]["east"], "text": c[0]["text"]}
    return None


def load_overrides(path, quest_ids):
    """Reads the hand-edited overrides; a bad row stops the run naming its file and line."""
    OVERRIDES.clear()
    if not path or not os.path.exists(path):
        return 0
    lines = open(path, encoding="utf-8").read().split("\n")
    if (lines[0].lstrip("\ufeff").rstrip("\r") if lines else "") != OVERRIDE_HEADER:
        sys.exit(f"{path}:1: the header must be `{OVERRIDE_HEADER.replace(chr(9), ' TAB ')}`")
    count = 0
    seen = set()
    for n, line in enumerate(lines[1:], 2):
        line = line.rstrip("\r")
        if not line.strip() or line.startswith("#"):
            continue
        f = [x.strip() for x in line.split("\t")]
        bad = lambda why: sys.exit(f"{path}:{n}: {why}")
        if len(f) != 5:
            bad(f"{len(f)} columns, 5 expected")
        vid, field, k, value, source = f
        if not vid.isdigit() or int(vid) not in quest_ids:
            bad(f"vault_id {vid!r} is no AC2 Vault quest")
        if field not in OVERRIDE_FIELDS:
            bad(f"field must be one of {', '.join(OVERRIDE_FIELDS)}")
        if not source:
            bad("source must not be empty (where the value comes from)")
        if field == "start_npc":
            if k == "entity" and hexval(value, {0x47}) is None or k == "iid" and not re.fullmatch(r"0x[0-9a-fA-F]{16}", value) \
                    or k not in ("entity", "iid"):
                bad("start_npc takes key `entity` with an EntityDesc DID or key `iid` with a 16-digit placement id")
        elif field == "kill_target":
            if not k or not value or any(hexval(v.strip(), {0x47}) is None for v in value.split(",")):
                bad("kill_target takes the target's name and EntityDesc DIDs")
        elif field == "reward_item":
            parts = value.split(",")
            if not k or hexval(parts[0].strip(), {0x47}) is None or len(parts) > 2 or \
                    len(parts) == 2 and not (parts[1].strip().isdigit() and 1 <= int(parts[1]) <= 16):
                bad("reward_item takes the reward's name and `0x47...` or `0x47...,count` (1 to 16)")
        elif field == "xp":
            if k == "fixed" and not (value.isdigit() and int(value) > 0):
                bad("xp fixed takes a positive whole number")
            if k == "percent" and not re.fullmatch(r"\d+(\.\d{1,2})?(,\d+)?", value):
                bad("xp percent takes `P` or `P,cap` (P with at most two decimals)")
            if k not in ("fixed", "percent"):
                bad("xp takes key `fixed` or `percent`")
        elif field == "quote":
            if not k.isdigit() or int(k) < 1 or not re.fullmatch(r"0x25[0-9a-fA-F]{6}:0x[0-9a-fA-F]{1,8}", value):
                bad("quote takes the quote's number and `0x25......:0x........` (table:key)")
        elif field == "prerequisite":
            if not value:
                bad("prerequisite takes the Vault id or title of the earlier quest")
        elif field == "step":
            if not k.isdigit() or int(k) < 1:
                bad("step takes its order, 1 or more")
            if parse_step(value) is None:
                bad("step is not `talk NPC`, `kill N X`, `loot ITEM`, `give ITEM to NPC`, `use ITEM on X`, `open OBJECT`, "
                    "`craft ITEM`, `effect 0x6f... SECONDS`, `lift 0x6f...`, `escort NAME`, `arrive OBJECT [N m]` or "
                    "`travel N.N[NS] E.E[EW]`")
        elif field == "item":
            if not k or hexval(value, {0x47}) is None:
                bad("item takes the item's name and its EntityDesc DID")
        elif field in ("npc", "object"):
            if not k or not (hexval(value, {0x47}) is not None or re.fullmatch(r"0x[0-9a-fA-F]{16}", value)):
                bad(f"{field} takes a name and `0x47...` or a 16-digit placement iid")
        elif field == "chest":
            for part in value.split(";"):
                e, _, c = part.strip().partition(",")
                if hexval(e.strip(), {0x47}) is None or c and not (c.strip().isdigit() and 1 <= int(c) <= 16):
                    bad("chest takes `0x47...[,count]` separated by `;`")
        elif field == "random_reward":
            for part in value.split(","):
                e, _, w = part.strip().partition(":")
                if hexval(e.strip(), {0x47}) is None or not (w.strip().isdigit() and int(w) > 0):
                    bad("random_reward takes `0x47...:weight` separated by `,`")
        elif field == "credit":
            if value not in ("fellowship", "killer"):
                bad("credit takes `fellowship` or `killer`")
        elif field == "drop_reward":
            if k not in ("qp", "kp") or value != "yes":
                bad("drop_reward takes key `qp` or `kp` and value `yes`")
        elif field == "ignore":
            if k not in IGNORABLE or value != "yes":
                bad(f"ignore takes a key of {', '.join(IGNORABLE)} and value `yes`")
        if (vid, field, k) in seen and field not in ("prerequisite",):
            bad(f"a second {field} {k!r} override for quest {vid}")
        seen.add((vid, field, k))
        OVERRIDES.setdefault(int(vid), []).append({"field": field, "key": k, "value": value, "source": source, "line": n})
        count += 1
    return count


def overrides(qid, field):
    return [o for o in OVERRIDES.get(qid, []) if o["field"] == field]


def draft(row, data, links):
    qid, title = row["id"], row["title"].strip().rstrip(":").strip()
    sec = sections(row["body"] or "")
    d = {"vault_id": qid, "title": title, "file": row["file"], "fields": {}, "gaps": [], "missing": [],
         "soft_gaps": [], "overlaps": [], "soft_overlaps": [], "notes": []}
    src_v = f"AC2 Vault quests/db id {qid}"
    F = d["fields"]

    # --- level and other requirements
    req = (row["requirements"] or "").strip()
    m = re.search(r"Level:\s*(\d+)\+?", req)
    F["level"] = {"value": int(m.group(1)) if m else None, "source": src_v + " requirements", "confidence": "high" if m else "none"}
    other_req = [l.strip() for l in req.splitlines() if l.strip() and not l.strip().lower().startswith("level")]
    F["requirements"] = {"value": other_req, "source": src_v + " requirements"}
    prereqs = []
    for l in other_req:
        for pm in re.finditer(r"(?:completed|complete)\s+(?:the\s+)?(.+?)(?:\s+quest)?(?:,|\.|$| and have)", l, re.I):
            prereqs.append(pm.group(1).strip())
    F["prerequisites"] = {"value": prereqs, "source": src_v + " requirements"}
    if overrides(qid, "prerequisite"):
        vals = [data.vault_titles.get(int(o["value"]), o["value"]) if o["value"].isdigit() else o["value"] for o in overrides(qid, "prerequisite")]
        prereqs[:] = vals
        F["prerequisites"] = {"value": vals, "source": "user override: " + "; ".join(o["source"] for o in overrides(qid, "prerequisite"))}

    # --- timers
    tl, tl_note = seconds(row["time_limit"])
    rs, rs_note = seconds(row["reset_timer"])
    F["time_limit_seconds"] = {"value": tl, "raw": row["time_limit"], "note": tl_note, "source": src_v}
    F["repeat_seconds"] = {"value": rs, "raw": row["reset_timer"], "note": rs_note, "source": src_v}

    # --- rewards
    rw = parse_reward(row["reward"])
    for o in overrides(qid, "xp"):
        rw["xp_observed"] = rw.get("xp_observed", []) + ([f"{rw['xp']} XP"] if rw.get("xp") else [])
        rw.update(xp=None, xp_percent=None, xp_cap=None, xp_min=None, xp_percent_unknown=False)
        if o["key"] == "fixed":
            rw["xp"] = int(o["value"])
        else:
            pct, _, cap = o["value"].partition(",")
            rw["xp_percent"], rw["xp_cap"] = float(pct), int(cap) if cap else None
        rw["other"] = [x for x in rw["other"] if not re.search(r"\bxp\b|experience|percentage", x, re.I)]
        rw["xp_source"] = "user override: " + o["source"]
    F["reward"] = {"value": {k: v for k, v in rw.items() if k != "raw"}, "raw": rw["raw"], "source": src_v + " reward"}

    # --- start NPC and coordinates
    start_text = sec.get("Starting the Quest", "")
    walk = sec.get("Quest Walkthrough", "")
    desc = (row["description"] or "").strip()
    start_coords = coords(start_text)
    npc_name = None
    for pat in NPC_START:
        m = pat.search(start_text)
        if m:
            npc_name = m.group(1).strip()
            break
    if not npc_name:
        m = TELLS.search(desc) or TELLS.search(walk)
        if m:
            npc_name = m.group(1).strip()
    if npc_name:
        npc_name = re.sub(r"\s+(in|at|near|located|outside|inside|from|who|on)$", "", npc_name)
        npc_name = re.split(r",", npc_name)[0].strip()
    F["start_npc"] = {"name": npc_name, "coords": start_coords[0] if start_coords else None,
                      "text": start_text[:300], "source": src_v + " 'Starting the Quest'"}

    # --- steps, quotes, monsters, items
    d["_walk"], d["_desc"] = walk, desc
    F["steps"] = steps_of(walk)
    if overrides(qid, "step"):
        F["steps"] = [dict(parse_step(o["value"]), at=int(o["key"]), source="user override: " + o["source"])
                      for o in sorted(overrides(qid, "step"), key=lambda o: int(o["key"]))]
    quotes = []
    for where, text in (("description", desc), ("walkthrough", walk), ("start", start_text)):
        for m in QUOTE.finditer(text):
            q = m.group(1).strip()
            if len(q.split()) >= 4:
                quotes.append({"where": where, "text": q})
    F["quotes"] = quotes
    F["monsters"] = sorted(set(links["creatures"].get(title, [])) | set(
        l for l in list_lines(sec.get("Encountered Monsters", "")) if len(l) < 60))
    F["items"] = sorted(set(links["trophies"].get(title, [])))
    feats = sorted({name for name, rx in FEATURE_WORDS if rx.search(" ".join([req, walk, start_text, row["misc_notes"] or "", rw["raw"]]))})
    F["features"] = feats

    # ------------------------------------------------------------------ resolution
    R = d["resolved"] = {}
    # journal id
    jid = None
    cands = data.native_by_key.get(key(title), [])
    if not cands:
        k = key(title)
        for kk, ids in data.native_by_key.items():
            if len(k) > 6 and (kk.startswith(k) or k.startswith(kk)) and abs(len(kk) - len(k)) <= 4:
                cands = ids
                break
    how = "DAT quest list (content/native-quests.json, gmQuestInfo) by name"
    if not cands:
        # 7230 keeps most quest names in private tables; the quest's package (its WState name,
        # e.g. QuestDarkstone) still carries the title
        k = package_key(title)
        best = None
        for pkg, i in data.native_by_package.items():
            pk = package_key(pkg)
            if not pk:
                continue
            r = 1.0 if pk == k else difflib.SequenceMatcher(None, pk, k).ratio()
            if r >= 0.9 and (best is None or r > best[0]):
                best = (r, i, pkg)
        if not best:
            # hunt titles name the verb differently (Fracture the Rime Golems / QuestKillGolemRime):
            # compare the remaining words as sets
            ws = package_words(title)
            hits = [(i, pkg) for pkg, i in data.native_by_package.items() if ws and package_words(pkg) == ws]
            if len(hits) == 1 and (len(ws) >= 2 or len(next(iter(ws))) >= 7):
                best = (0.9, hits[0][0], hits[0][1])
        if best:
            cands = [best[1]]
            how = f"DAT quest list (content/native-quests.json) by package name {data.native[best[1]]['package']} ({best[0]:.2f})"
    if cands:
        jid = cands[0]
        q = data.native[jid]
        R["journal"] = {"id": f"0x{jid:08x}", "package": q.get("package"), "name": q["_name"],
                        "phases": len(q.get("phases") or []),
                        "title_table": f"0x{q['name']['table']:08x}" if q["name"].get("table") else None,
                        "title_key": f"0x{q['name']['key']:08x}" if q["name"].get("key") else None,
                        "confidence": "high" if (q["_name"] and key(q["_name"]) == key(title)) or package_key(q.get("package")) == package_key(title) else "medium",
                        "source": how}
    else:
        R["journal"] = None
        # the Shelter's precedent: a Pyrealium journal id with the title from a DAT string
        t = data.find_title(title)
        R["title"] = t
        if t and t["confidence"] != "low":
            d["soft_gaps"].append("Pyrealium journal id (no DAT quest of this name; the title is a DAT string)")
        elif t:
            d["missing"].append(f"journal id (the only DAT string of the title is in {t['table_name']}, not a quest table)")
        else:
            d["missing"].append("journal id and title string (no DAT quest or string of this name)")

    # start NPC
    npc = None
    near = (start_coords[0]["north"], start_coords[0]["east"]) if start_coords else None
    ov = overrides(qid, "start_npc")
    if ov:
        o = ov[-1]
        if o["key"] == "iid":
            ps = [p for ps_ in data.placements.values() for p in ps_ if p["iid"] == o["value"].lower()]
        else:
            ps = data.place({int(o["value"], 16)}, near, 99.0) if near else data.place({int(o["value"], 16)})
        src = f"user override: {o['source']}"
        if ps:
            p = ps[0]
            npc = {"name": data.entity_name.get(p["entity"]), "entity": f"0x{p['entity']:08x}", "iid": p["iid"], "placed": True,
                   "cell": p["cell"], "map": [p["north"], p["east"]], "distance": p.get("_d"),
                   "table": (f"0x{data.table_of_entity[p['entity']]:08x}" if p["entity"] in data.table_of_entity else None),
                   "confidence": "high", "source": f"{src}; placement {p['iid']} in the prepared world"}
        else:
            d["missing"].append(f"start NPC override ({o['key']} {o['value']}) has no placement in the prepared world")
            npc = {"name": npc_name, "entities": [o["value"]] if o["key"] == "entity" else [], "confidence": "none", "source": src}
    elif npc_name:
        ents = data.entities(npc_name)
        if not ents:
            # 'Akkilea, Zu Factotum' or 'Tyke Geddon, Apprentice Armorsmith': try the first words
            parts = npc_name.split()
            for n in range(len(parts) - 1, 0, -1):
                ents = data.entities(" ".join(parts[:n]))
                if ents:
                    break
        placed = data.place(ents, near, 1.0) if ents else []
        if not placed and ents and not near:
            placed = data.place(ents)
        if placed:
            p = placed[0]
            exact_name = key(data.entity_name.get(p["entity"], "")) in plural_keys(npc_name)
            conf = "high" if exact_name and (near is None and len(placed) == 1 or near and p.get("_d", 9) <= 0.4) else "medium"
            iid = p["iid"]
            note = ""
            if near is None and len(placed) > 1:
                conf = "low"
                # One NPC captured at several nearby spots (Shiela Ondaran in two Arwic blocks):
                # every placement of the entity answers, so the sets name the entity, not an iid.
                if exact_name and len({x["entity"] for x in placed}) == 1 and max(
                        dist((x["north"], x["east"]), (y["north"], y["east"])) for x in placed for y in placed) <= 1.5:
                    conf, iid = "medium", None
                    note = (f"; {len(placed)} placements of the one entity within 1.5 map units ("
                            + ", ".join(x["iid"] for x in placed) + "), so the sets name the entity")
            npc = {"name": data.entity_name.get(p["entity"]), "entity": f"0x{p['entity']:08x}", "iid": iid,
                   "placed": bool(placed),
                   "cell": p["cell"], "map": [p["north"], p["east"]], "distance": p.get("_d"),
                   "table": (f"0x{data.table_of_entity[p['entity']]:08x}" if p["entity"] in data.table_of_entity else None),
                   "confidence": conf,
                   "source": f"Pyrealium world {os.path.basename(data.world_path)} placement of an entity named "
                             f"'{data.entity_name.get(p['entity'])}' (definitions.tsv / DAT name)"
                             + (f", {p.get('_d')} map units from the Vault's {start_coords[0]['text']}" if near else "") + note}
        elif ents:
            d["missing"].append(f"start NPC '{npc_name}' placement (entity known: "
                                + ", ".join(f"0x{e:08x}" for e in sorted(ents)[:4]) + "; none placed in Pyrealium's world near the Vault coordinates)")
            npc = {"name": npc_name, "entities": [f"0x{e:08x}" for e in sorted(ents)], "confidence": "none"}
        else:
            d["missing"].append(f"start NPC '{npc_name}' entity (no DAT/definitions name match)")
    else:
        d["missing"].append("start NPC (the Vault names none, or the start is not an NPC)")
    if near:
        R["start_cell"] = dict(map_to_cell(*near), source=src_v + " start coordinates (LandDefs map conversion)")
    R["start_npc"] = npc

    # quotes
    prefer = set()
    if npc and npc.get("table"):
        prefer.add(int(npc["table"], 16))
    rq = []
    for q in quotes:
        hit = data.find_quote(q["text"], prefer)
        rq.append({"where": q["where"], "vault": q["text"][:200], "dat": hit})
    for o in overrides(qid, "quote"):
        i = int(o["key"]) - 1
        if i >= len(rq):
            sys.exit(f"vault-quest-overrides.tsv:{o['line']}: quest {qid} has {len(rq)} quotes")
        t, k = (int(x, 16) for x in o["value"].split(":"))
        rq[i]["dat"] = {"table": f"0x{t:08x}", "key": f"0x{k:08x}", "table_name": data.table_name.get(t),
                        "dat_text": data.string(t, k), "similarity": None, "confidence": "high",
                        "source": f"user override: {o['source']} (DAT string table 0x{t:08x} key 0x{k:08x})"}
    R["quotes"] = rq
    unresolved_quotes = [q for q in rq if not q["dat"]]

    # creatures (kill targets): DAT death effects for this journal id win
    targets = {}
    death = data.death.get(jid, []) if jid else []
    kill_names = [s["target"] for s in F["steps"] if s["kind"] == "kill"]
    for s in F["steps"]:
        if s["kind"] == "kill":
            # 'kill a Burun Immortal ... once you kill the Immortal': the short form is the same target
            if any(o != s["target"] and key(o).endswith(key(s["target"])) for o in kill_names):
                s["alias"] = True
                continue
            ents = data.entities(s["target"])
            kov = [o for o in overrides(qid, "kill_target") if o["key"] == "*" or key(o["key"]) == key(s["target"])]
            if kov:
                ents = {int(v.strip(), 16) for v in kov[-1]["value"].split(",")}
                placed = sorted(e for e in ents if data.placements.get(e))
                packed = sorted(e for e in ents if data.pack_entities.get(e))
                where = ("placed in the prepared world" if placed else "only in Pyrealium content packs the prepared world leaves out"
                         if packed else "not placed by any Pyrealium world table or camp")
                targets[s["target"]] = {"count": s["count"], "entities": [f"0x{e:08x}" for e in sorted(ents)],
                                        "placed": len(placed), "in_packs": len(packed), "where": where,
                                        "spawned": bool(placed or packed), "confidence": "high",
                                        "source": f"user override: {kov[-1]['source']}; {where}"}
                continue
            if not ents:
                continue
            dat_ents = sorted({e for e, _ in death if e in ents})
            placed = sorted({e for e in ents if data.placements.get(e)})
            packed = sorted({e for e in ents if data.pack_entities.get(e)})
            chosen = dat_ents or placed or packed or sorted(ents)
            conf = "high" if dat_ents else ("medium" if placed or packed else "low")
            where = ("placed in the prepared world" if any(e in placed for e in chosen) else
                     "only in Pyrealium content packs the prepared world leaves out: "
                     + ", ".join(sorted({p for e in chosen for p in data.pack_entities.get(e, ())}))
                     if any(e in packed for e in chosen) else "not placed by any Pyrealium world table or camp")
            targets[s["target"]] = {"count": s["count"], "entities": [f"0x{e:08x}" for e in chosen],
                                    "placed": len(placed), "in_packs": len(packed), "where": where,
                                    "spawned": any(e in placed or e in packed for e in chosen), "confidence": conf,
                                    "source": ("DAT death effects naming this quest (native-quests.json entity_bindings)"
                                               if dat_ents else "definitions.tsv / DAT name") + "; " + where}
    R["kill_targets"] = targets
    R["death_effects"] = [{"entity": f"0x{e:08x}", "name": data.entity_name.get(e), "effect": eff} for e, eff in death]

    # items (trophies, loot, reward items)
    items = {}
    item_names = set(F["items"])
    for s in F["steps"]:
        if s["kind"] in ("loot", "give", "use") and s.get("item"):
            item_names.add(s["item"])
    for n in rw["other"]:
        if len(n) < 50 and ":" not in n and re.match(r"^[A-Z]", n):
            item_names.add(n.strip())
    for n in sorted(item_names):
        ents = data.entities(n)
        if ents:
            items[n] = {"entities": [f"0x{e:08x}" for e in sorted(ents)][:6], "confidence": "medium" if len(ents) == 1 else "low",
                        "source": "definitions.tsv / DAT entity name"}
    counts = {}
    for o in overrides(qid, "reward_item"):
        ent, _, cnt = o["value"].partition(",")
        items[o["key"]] = {"entities": [f"0x{int(ent.strip(), 16):08x}"], "confidence": "high",
                           "source": "user override: " + o["source"]}
        counts[o["key"]] = int(cnt) if cnt else 1
        if o["key"] not in rw["other"]:
            rw["other"].append(o["key"])
    R["reward_counts"] = counts
    R["items"] = items
    reward_items = [n for n in rw["other"] if n in items]
    unresolved_reward = [n for n in rw["other"] if len(n) < 50 and ":" not in n and re.match(r"^[A-Z]", n) and n not in items]

    # effects (curses, buffs) named anywhere in the prose
    effects = {}
    text_all = " ".join([walk, rw["raw"], row["misc_notes"] or "", desc])
    low = text_all.lower()
    for k, lst in data.effects.items():
        name = lst[0]["name"]
        # single words ('Portal', 'Spirit') are ordinary prose; a named effect is two words or more,
        # written with its capitals
        if len(name.split()) < 2 or name.lower() not in low:
            continue
        if not re.search(r"\b" + re.escape(name) + r"\b", text_all):
            continue
        if data.by_name.get(k):
            continue  # an object's name (item/creature), not an effect
        effects[name] = [{"wstate": f"0x{e['wstate']:08x}" if e["wstate"] else None, "table": f"0x{e['table']:08x}",
                          "file": e["file"]} for e in lst]
    R["effects"] = effects

    # ------------------------------------------------------------------ engine gaps
    g = d["gaps"]
    if rs is not None:
        g.append("repeat timer")
    elif rs_note and rs_note != "unknown":
        g.append("repeat timer")
    if tl is not None:
        g.append("time limit")
    if F["level"]["value"] and F["level"]["value"] > 1:
        g.append("level requirement")
    if prereqs:
        g.append("prerequisite quests")
    # Kingdom, faction and hero: the audit of the Vault's 44 quests that name them. A Hero (of
    # Dereth) is a character of level 45 or more: Focus and hero credits begin at 45 in the
    # player level table (Pyreal_DB_Insert_PlayerLevelData.sql) and the client's vitals show the
    # Focus meter only for a hero (gmVitalsUI::RecvNotice_UpdateHeroStatus 00439e00), so the
    # requirement is a level of 45. A kingdom requirement or reward is in the requirements or the
    # reward (aligned to / belong to / in a kingdom, a named kingdom's hero, Kingdom Points), or
    # the start addresses kingdom members; a kingdom that is only a place or an item ('Kingdom
    # NPC', 'kingdom HQ library', 'Kingdom armor', 'Allegiance hall') is not one.
    hero = bool(re.search(r"\bhero\b", req, re.I)) and not re.search(r"hero (of|in) the \w+ kingdom", req, re.I)
    if hero:
        F["hero"] = {"value": True, "source": src_v + " requirements ('" + req.replace(chr(10), " / ") + "'): a Hero is level 45+ "
                     "(Focus and hero credits begin at level 45 in Pyreal_DB_Insert_PlayerLevelData.sql; the client shows "
                     "the Focus meter only for a hero, gmVitalsUI::RecvNotice_UpdateHeroStatus 00439e00)"}
        if (F["level"]["value"] or 0) < 45:
            F["level"] = {"value": 45, "source": F["hero"]["source"], "confidence": "medium"}
    kingdom_req = re.search(r"kingdom|faction", req, re.I) or rw.get("kp") or re.search(r"kingdom points", rw["raw"], re.I) \
        or re.search(r"kingdom members|your kingdom", start_text, re.I)
    if kingdom_req:
        F["kingdom"] = {"value": (re.search(r"(Order|Dominion|Shadow)", req) or re.search(r"(Order|Dominion|Shadow)", start_text) or [None, None])[1]
                        if kingdom_req else None, "source": src_v + " requirements/reward/start"}
        g.append("kingdom/faction/hero status")
    elif "kingdom" in feats:
        d["notes"].append("names a kingdom, a faction or an allegiance only as a place or an item: not a requirement (audit)")
    if rw["qp"]:
        g.append("quest points")
    if rw["kp"]:
        g.append("kingdom points")
    if rw["gold"]:
        g.append("gold reward")
    received = re.search(r"\b(?:receive|given|hands? you|gives? you)\s+(?:a|an|the|your)\s+(?!quest\b|message\b|reward\b|tell\b|curse\b|buff\b|\d)", walk, re.I)
    if reward_items or received or items and any(s["kind"] == "loot" for s in F["steps"]):
        g.append("item grants beyond the Shelter's item profiles")
    if effects or re.search(r"\bcurse[sd]?\b|\bbuff\b|\beffect\b", rw["raw"] + " " + walk, re.I):
        g.append("ApplyEffect")
    if any(s["kind"] == "give" for s in F["steps"]):
        # the Shelter's precedent: 'give the tool to the Scout' is a Use of the Scout with InqItem
        # and TakeItem; the Give hook would be more faithful but does not block
        d["soft_gaps"].append("Give hook (Use + InqItem + TakeItem stands in, as in the Shelter)")
    if any(s["kind"] == "travel" for s in F["steps"]) and not any(s["kind"] in ("talk", "kill") for s in F["steps"]):
        g.append("Enter hook (arrive at a place)")
    if sum(1 for t in targets.values() if t["count"] > 1) > 1:
        g.append("several kill counters in one quest")
    if targets and KILL_LINE.search(walk):
        g.append("Kill-set Tell (a line on a kill)")
    if "fellowship" in feats:
        g.append("fellowship credit")
    if "random_reward" in feats:
        g.append("random reward choice")
    if "crafting" in feats:
        g.append("crafting step")
    if "chest_or_door" in feats:
        g.append("chest/door/lever interaction")
    if "escort" in feats:
        g.append("escort/protect")
    if "defend" in feats:
        g.append("defend a spawned object")
    if "pet" in feats:
        g.append("summoned pet reward")
    if rw["xp_percent"]:
        pass  # AwardLevelProportionalXP covers it
    if rw.get("xp_percent_unknown") and not rw.get("xp_percent"):
        d["missing"].append("experience percentage (the Vault records only an observed amount)")
    if rw.get("xp_percent") and rw.get("xp"):
        d["notes"].append("absolute XP beside a percentage: read as an observation at one level")
    if not F["steps"] and not quotes:
        d["missing"].append("walkthrough has no parseable steps")
    if unresolved_quotes:
        d["missing"].append(f"{len(unresolved_quotes)} of {len(quotes)} quoted lines have no DAT string match")
    for n in unresolved_reward:
        d["missing"].append(f"reward item '{n}' entity")
    for s in F["steps"]:
        if s["kind"] == "kill" and s["target"] not in targets and not s.get("alias"):
            d["missing"].append(f"kill target '{s['target']}' entity")
    for n, t in targets.items():
        if not t["spawned"]:
            d["missing"].append(f"kill target '{n}' placement (its entities are not placed by any Pyrealium world table or camp)")
    if npc and npc.get("placed") and npc["confidence"] == "low":
        d["missing"].append("start NPC placement (several same-named placements and no Vault coordinates)")

    # ------------------------------------------------------------------ overlaps
    if jid and data.catalog_quest.get(jid):
        d["overlaps"].append({"by": "journal id", "catalogs": data.catalog_quest[jid], "winner": "catalog",
                              "why": "an operated quest of this journal id; rows here would double-run it"})
    elif data.catalog_title.get(key(title)):
        d["overlaps"].append({"by": "title", "catalogs": data.catalog_title[key(title)], "winner": "catalog",
                              "why": "an operated quest of this title; rows here would double-run it"})
    if npc and npc.get("entity"):
        e = int(npc["entity"], 16)
        cats = sorted(set(data.catalog_npc.get(e, [])))
        # hunting_quests::supported answers each quest's `npc`; exploration_quests::supported its
        # NPCs; an `npcs` placement of the other catalogs has no Use handler of its own
        handler = [c for c in cats if c.endswith("(quest NPC)") or (c.startswith("exploration") and c.endswith("(NPC)"))]
        placed_by = [c for c in cats if not c.endswith("(dialogue)") and c not in handler]
        if placed_by:
            d["soft_overlaps"].append({"by": "start NPC entity", "catalogs": placed_by, "winner": "vault rows",
                                       "why": "the catalog only places this NPC; it has no Use handler"})
        if handler:
            d["overlaps"].append({"by": "start NPC entity", "catalogs": handler, "winner": "catalog",
                                  "why": "server.rs runs the hunting/exploration NPC handlers before quest emotes, "
                                         "so a Use set on this NPC never runs"})
        if any(c.endswith("(dialogue)") for c in cats):
            d["soft_overlaps"].append({"by": "start NPC entity", "catalogs": [c for c in cats if c.endswith("(dialogue)")],
                                       "winner": "vault rows",
                                       "why": "npc-dialogue topics are information only and server.rs runs them after quest emotes"})
        if e in data.base_owners:
            d["overlaps"].append({"by": "start NPC entity", "catalogs": ["emote-sets.tsv"], "winner": "first set in file order",
                                  "why": "another quest's sets already answer this NPC"})

    # ------------------------------------------------------------------ classification
    d["gaps"] = sorted(set(g))
    d["remaining_gaps"] = [x for x in d["gaps"] if x not in ENGINE_HAS]
    if prereqs:
        d["missing"].append("prerequisite quests: the generator does not link them to their own rows yet")
    if d["remaining_gaps"]:
        d["status"] = "blocked"
    elif d["missing"]:
        d["status"] = "partial"
    else:
        d["status"] = "resolved"
    return d


# ---------------------------------------------------------------------------------------------
# rows for the quests that resolve cleanly

# Engine features `quest_emotes` has (Phase 2): the registry's min_level, time_limit_seconds and
# repeat_seconds, GiveItem of any item entity, AwardGold, InqSolved.
ENGINE_HAS = {"level requirement", "time limit", "repeat timer", "item grants beyond the Shelter's item profiles",
              "several kill counters in one quest", "Kill-set Tell (a line on a kill)",
              "gold reward", "prerequisite quests", "Enter hook (arrive at a place)", "ApplyEffect"}

# NPC StringInfo keys whose role is the same in every NPC table (the key is a hash of the line's
# name; 7230 has 60 to 140 tables with each). The roles are Pyrealium's reading of the lines' text
# across those tables: a quest's offer, its reward line and its 'not done yet' line come in
# matching variants (one per quest an NPC gives), and the 'no task today' and 'come back when
# stronger' lines are one per NPC.
NPC_GROUPS = [
    {"offer": 0x2AE6122C, "reward": 0x68A07C4D, "progress": 0xCDE80FB2},
    {"offer": 0x2AE6124E, "reward": 0x68A04D4D, "progress": 0xCDEB1FB2},
    {"offer": 0x2AE61248, "reward": 0x68A04E4D, "progress": 0xCDEB2FB2},
    {"offer": 0x2AE6124A, "reward": 0x68A04F4D, "progress": 0xCDEB3FB2},
]
NPC_NO_TASK = 0x09200EFE
NPC_TOO_WEAK = 0x68F492A3
SUMMARY_KEY = DESC_KEY


def table_strings(data, table):
    return {k: t for k, t in data.dat.execute(
        "SELECT string_id, text FROM strings WHERE release='7230' AND table_did=?", (table,))}


def journal_id(d):
    """The DAT quest's journal id, or Pyrealium's (0x4002_0000 | the Vault id) for a quest the DAT
    quest list does not hold (the Shelter's precedent: 0x4000ff05)."""
    j = d["resolved"].get("journal")
    return (int(j["id"], 16), j["source"]) if j else (0x40020000 | d["vault_id"], "Pyrealium journal id 0x4002_0000 | Vault id (no DAT quest id is recovered for this quest)")


def hunt_parts(d, data, catalog=None):
    """A hunt's own rows (registry, stages, kill sets) and what its NPC's sets need: the NPC
    offers it, N kills of one creature advance the journal one phase each (the DAT quest table's
    phase lines: N, N-1, ... 1 left, then 'Report to X.'), and the NPC pays on return. `catalog`
    is the hunting catalog quest it replaces (its id is kept, so the journal entry and the
    progress carry over; its targets are kept where they come from the DAT's NPC table). None,
    with the reason in the draft, when any piece is missing."""
    why = d.setdefault("generator", {})
    R, F = d["resolved"], d["fields"]
    targets = list(R["kill_targets"].items())
    if len(targets) != 1 or len([s for s in F["steps"] if s["kind"] == "kill" and not s.get("alias")]) != 1:
        why["hunt"] = "not one kill target"
        return None
    target, kt = targets[0]
    n = kt["count"]
    title = R.get("title") or {}
    if not title or not (title.get("table_name") or "").startswith("quest"):
        why["hunt"] = "no DAT quest table (quest*.stt) holds the title"
        return None
    qt = int(title["table"], 16)
    qs = table_strings(data, qt)
    phase = {}
    report = None
    for k, t in qs.items():
        m = re.match(r"^(\d+) (.+)$", t.strip())
        if m:
            phase.setdefault(int(m.group(1)), []).append((k, t.strip()))
        elif re.match(r"^(Report|Return) to .+\.$", t.strip()):
            report = (k, t.strip())
    if n == 1 and not phase:
        # a single kill's task line ('Find and destroy Cunning Tom.') before the report line
        task = [(k, t.strip()) for k, t in qs.items() if re.match(r"^(Find and destroy|Hunt down and slay|Slay|Destroy|Kill) .+\.$", t.strip())]
        if len(task) == 1:
            phase[1] = task
    if sorted(phase) != list(range(1, n + 1)) or any(len(v) != 1 for v in phase.values()) or not report:
        why["hunt"] = f"the quest table's phase lines are not 1..{n} and a report line"
        return None
    npc = R.get("start_npc") or {}
    if not npc.get("placed") or not npc.get("table") or npc.get("confidence") not in ("high", "medium"):
        why["hunt"] = "no placed start NPC with a string table"
        return None
    nt = int(npc["table"], 16)
    ns = table_strings(data, nt)
    # The quest table's summary quotes the offer ('Oriona said: \q...\q'), sometimes reworded
    summary = norm_text(re.sub(r"^.*?\bsaid:?\s*", "", qs.get(SUMMARY_KEY, "")).replace("\\q", " "))
    def like(text):
        t = norm_text(text)
        return bool(t) and (t in summary or difflib.SequenceMatcher(None, t, summary, autojunk=False).ratio() >= 0.85)
    group = [gp for gp in NPC_GROUPS if gp["offer"] in ns and gp["reward"] in ns and like(ns[gp["offer"]])]
    if len(group) != 1:
        why["hunt"] = "the NPC table has no single offer/reward pair whose offer is the quest's summary"
        return None
    gp = group[0]
    rw = F["reward"]["value"]
    if rw.get("xp"):
        pay = ("AwardXP", [str(rw["xp"])], xp_source(rw, None, f"{rw['xp']} XP"))
    elif rw.get("xp_percent"):
        cap = rw.get("xp_cap") or ""
        pay = ("AwardLevelProportionalXP", [f"{rw['xp_percent']:g}", str(rw.get("xp_min") or ""), str(cap)],
               xp_source(rw, None, f"{rw['xp_percent']:g}% XP" + (f", capped at {cap}" if cap else " (the Vault states no cap)")))
    else:
        why["hunt"] = "no experience reward the Vault states"
        return None
    gives = reward_gives(d)
    if any(rw.get(x) for x in ("qp", "kp", "gold")) or gives is None:
        why["hunt"] = "a reward beyond experience and identified items"
        return None
    q = f"v{d['vault_id']}"
    vsrc = f"AC2 Vault quests/db id {d['vault_id']}"
    if catalog:
        src, cq = catalog
        jid, jsrc = cq["id"], (f"the id of the {src} hunt this quest replaces (kept, so its journal entries and progress "
                               "carry over)")
    else:
        jid, jsrc = journal_id(d)
    ents, esrc = kt["entities"], kt["source"]
    if catalog and (cq.get("evidence") or {}).keys() & {"native_death_effects", "native_death_effect"}:
        # the catalog's targets come from the DAT's NPC tables (death effects): the DATs outrank the Vault
        ents = [f"0x{e:08x}" for e in cq["targets"]]
        esrc = f"the DAT NPC tables' death effects for this hunt (as {src} cites them; DAT over Vault)"
    tsrc = lambda k: f"DAT string table 0x{qt:08x} ({data.table_name.get(qt)}) key 0x{k:08x}"
    level = F["level"]["value"] or 0
    tl = F["time_limit_seconds"]["value"]
    rs = F["repeat_seconds"]["value"]
    reg = [[q, f"0x{jid:08x}", f"0x{qt:08x}", title["key"], title["text"].replace("[E]", "").strip(), "", "killer",
            level or "", tl or "", "", rs or "",
            f"Title: {tsrc(int(title['key'], 16))}; journal id: {jsrc}; level, time limit and repeat: {vsrc} "
            f"('{F['level']['value']}+', '{F['time_limit_seconds']['raw']}', '{F['repeat_seconds']['raw']}')"]]
    stages = []
    for k in range(1, n + 1):
        key_, text = phase[n - k + 1][0]
        stages.append([q, k, 0, text, f"{tsrc(key_)}: " + ("the task, before the report line" if n == 1 and not text[0].isdigit() else
                       f"phase {k} shows the kills left (the DAT's numbered phase lines count down, as Legions' "
                       "QuestGGWNightmare1-6 phases do)")])
    stages.append([q, n + 1, 0, report[1], tsrc(report[0]) + f": phase {n + 1}, after the last kill"])
    done_text = re.sub(r"\\q", '"', qs.get(SUMMARY_KEY, "")).strip()
    stages.append([q, n + 2, 1, done_text, tsrc(SUMMARY_KEY) + " (the quest's summary; Pyrealium shows it for the completed quest)"])
    sets, acts = [], []
    for ent in ents:
        for k in range(1, n + 1):
            sets.append([f"{q}_kill_{ent[2:]}_{k}", ent, "", "Kill", "", q, k, k, "", "",
                         f"{esrc}; each kill advances one phase (the DAT has eff_advancequest...phase2..{n + 1})"])
            acts.append([f"{q}_kill_{ent[2:]}_{k}", 1, "SetStage", 0, q, k + 1, "", "", "", f"{vsrc}: kill {n} {target}"])
    why["hunt"] = "generated"
    return {"q": q, "d": d, "n": n, "reg": reg, "stages": stages, "sets": sets, "acts": acts, "npc": npc, "nt": nt,
            "ns": ns, "gp": gp, "pay": pay, "gives": gives, "level": level, "vsrc": vsrc, "jid": jid, "targets": ents,
            "catalog": catalog}


STOP_WORDS = {"the", "of", "to", "a", "an", "and", "for", "is", "he", "she", "in", "at", "on", "out", "with", "his", "her",
              "seek", "speak", "talk", "find", "go", "who", "that", "this", "from", "by"}


def words(text):
    return {w for w in re.split(r"[^a-z0-9]+", (text or "").lower()) if len(w) > 2 and w not in STOP_WORDS}


def reward_gives(d):
    """The reward's items as (entity, count, source): each `other` reward line must be an item
    with one entity (resolved, or a user override); None when one is not."""
    R, rw = d["resolved"], d["fields"]["reward"]["value"]
    out = []
    for name in [o for o in rw.get("other", []) if o.strip()]:
        it = R["items"].get(name)
        if not it or len(it["entities"]) != 1:
            return None
        out.append((it["entities"][0], R.get("reward_counts", {}).get(name, 1), f"{it['source']}: reward '{name}'"))
    return out


def xp_source(rw, vsrc, text):
    return rw.get("xp_source") or f"AC2 Vault reward: {text}"


DATA = None


def parity(d, h, data):
    """How the generated hunt compares with the catalog quest of the same title: the fields that
    differ (vault rows, catalog), empty at parity."""
    out = {}
    reg = h["reg"][0]
    npc = int(h["npc"]["entity"], 16)
    targets = sorted(int(t, 16) for t in h["targets"])
    n = h["n"]
    pay = h["pay"]
    for src, q in data.catalog_quest_rows.get(key(d["title"]), []):
        diff = {}
        cmp = [("npc", npc, q.get("npc")), ("targets", targets, sorted(q.get("targets") or [])),
               ("goal", n, q.get("goal")), ("level", reg[7] or 0, q.get("level")),
               ("time limit", reg[8] or 0, q.get("limit_seconds")), ("repeat", reg[10] or 0, q.get("repeat_seconds"))]
        if pay[0] == "AwardXP":
            cmp.append(("experience", f"{pay[1][0]} XP", f"{q.get('xp_cap')} XP" if not q.get("xp_basis_points") else f"{q['xp_basis_points'] / 100:g}%"))
        else:
            cmp.append(("experience", f"{pay[1][0]}% cap {pay[1][2] or '-'}", f"{(q.get('xp_basis_points') or 0) / 100:g}% cap {q.get('xp_cap')}"))
        for name, mine, theirs in cmp:
            if mine != theirs:
                diff[name] = [mine if not isinstance(mine, list) else [f"0x{x:08x}" for x in mine],
                              theirs if not isinstance(theirs, list) else [f"0x{x:08x}" for x in theirs]]
        out[src] = diff
    return out


PREREQ_MISSING = "prerequisite quests: the generator does not link them to their own rows yet"


def chain_order(hunts):
    """The hunts of one NPC in prerequisite order (Vault requirements by title, then the catalog's
    prerequisite field); None when they do not form one chain."""
    by_title = {key(h["d"]["title"]): h for h in hunts}
    pre = {}
    for h in hunts:
        names = [key(p) for p in h["d"]["fields"]["prerequisites"]["value"]]
        found = [by_title[n] for n in names if n in by_title]
        if not found and h["catalog"] and h["catalog"][1].get("prerequisite"):
            found = [x for x in hunts if x["catalog"] and x["catalog"][1]["id"] == h["catalog"][1]["prerequisite"]]
        if len(found) > 1:
            return None
        pre[h["q"]] = found[0]["q"] if found else None
    roots = [h for h in hunts if pre[h["q"]] is None]
    if len(roots) != 1:
        return None
    order = [roots[0]]
    while len(order) < len(hunts):
        nxt = [h for h in hunts if pre[h["q"]] == order[-1]["q"]]
        if len(nxt) != 1:
            return None
        order.append(nxt[0])
    return order


KINGDOM_TRACE = ("kingdom membership: Pyrealium stores a character's kingdom and Kingdom Points (quest actions "
                 "SetKingdom, InqKingdom, AwardKingdomPoints, InqKingdomPoints), but no source traced shows how a player "
                 "joins a kingdom (which NPC or quest sets it): decide the join flow, then the quest's rows can gate on it")
DEFEND_TRACE = ("a timed hold: the player's item use spawns a beacon, portal bore or crop that must survive "
                "attackers for a time; Pyrealium's quest tables have no action that spawns a creature or object at the "
                "player and no timed stage (delay_ms is not scheduled), so this needs engine work beyond escort "
                "(or `ignore defend`)")
KINGDOM_TRACE_OLD = ("kingdom/faction/hero standing: the client knows a faction (gmWeenieDesc data mask "
                 "FactionType 0x2000) and shows 'Kingdom Points:' (UI string table 0x25000011 key 0x5c9a09da), "
                 "but Pyrealium stores neither (allegiance.rs answers every player unaffiliated); the requirement and "
                 "award need that state first. Or `ignore kingdom` where the walkthrough only mentions one")


def classify(d, data):
    """What stops a draft, and who can close it: (gap, who, detail). `you` closes a gap with an
    override or a placement, `decision` with a policy choice (also an override), `us` with engine
    work. Quoted lines with no DAT string are optional (an override can name them)."""
    qid, R, F = d["vault_id"], d["resolved"], d["fields"]
    d["blocks"], d["optional"] = [], []
    if not R.get("journal") and not (R.get("title") and R["title"].get("confidence") != "low"):
        R["title"] = {"table": None, "key": None, "table_name": None, "text": d["title"], "confidence": "high",
                      "source": "Pyrealium title: the AC2 Vault's (no DAT quest or string of this name; the Shelter's precedent)"}
    for m in d["missing"]:
        if m.startswith("journal id") or m == PREREQ_MISSING:
            continue
        if "quoted lines have no DAT" in m:
            d["optional"].append(("NPC line not found in DAT (optional: a `quote` override)", "you", m))
            continue
        kind = next(((g, w) for needle, g, w in GAP_KINDS if needle in m), ("Other data", "you"))
        d["blocks"].append((kind[0], kind[1], m))
    ign = {o["key"] for o in overrides(qid, "ignore")}
    kinds = {parse_step(o["value"])["kind"] for o in overrides(qid, "step")}
    for g in d["gaps"]:
        if g in ENGINE_HAS:
            continue
        word = next((k for k, v in IGNORABLE.items() if v == g), None)
        if word and word in ign:
            continue
        if g == "chest/door/lever interaction":
            if "open" not in kinds:
                d["blocks"].append(("Supply values", "you", "the walkthrough has a chest, door or lever: write the steps with "
                                    "`open OBJECT`, an `object` override for its placement and a `chest` override with what it "
                                    "gives (or `ignore chest`)"))
        elif g == "crafting step":
            if "craft" not in kinds:
                d["blocks"].append(("Write steps", "you", "the walkthrough has a crafting step: write the steps with `craft ITEM` "
                                    "(or `ignore crafting`)"))
        elif g == "random reward choice":
            if not overrides(qid, "random_reward"):
                d["blocks"].append(("Supply values", "you", "the reward is drawn at random: a `random_reward` override with the "
                                    "items and their weights (or `ignore random`)"))
        elif g == "fellowship credit":
            if not overrides(qid, "credit"):
                d["blocks"].append(("Decide", "decision", "fellowship credit: a `credit` override, `fellowship` (the killer's "
                                    "fellows count too) or `killer` (or `ignore fellowship`)"))
        elif g == "summoned pet reward":
            if reward_gives(d) is None or not F["reward"]["value"].get("other"):
                d["blocks"].append(("Identify entity or item", "you", "the reward summons a pet: a `reward_item` override naming "
                                    "the pet's summoning item (or `ignore pet`)"))
        elif g == "kingdom points":
            pass  # AwardKingdomPoints carries it; the quest waits on the kingdom decision row
        elif g in ("quest points", "kingdom points"):
            k = "qp" if g == "quest points" else "kp"
            if not any(o["key"] == k for o in overrides(qid, "drop_reward")):
                d["blocks"].append(("Decide", "decision", f"{g}: neither the client nor the DATs define a stat Pyrealium could award; "
                                    f"drop it (`drop_reward {k} yes`) or ask for a Pyrealium counter"))
        elif g == "kingdom/faction/hero status":
            d["blocks"].append(("Decide", "decision", KINGDOM_TRACE))
        elif g == "escort/protect":
            if "escort" not in kinds:
                d["blocks"].append(("Write steps", "you", "an escorted NPC or creature: write the steps with `escort NAME` (a "
                                    "creature Pyrealium spawns near where the escort starts; an `npc` override may name its "
                                    "`0x47...`), then `arrive OBJECT [N m]` at the destination or a turn-in step (or `ignore "
                                    "escort` where the walkthrough only mentions one)"))
        elif g == "defend a spawned object":
            d["blocks"].append(("Engine feature", "us", DEFEND_TRACE))
        else:
            d["blocks"].append(("Engine feature", "us", g))


def placement_of(data, value, source):
    """(entity, iid or None, table, source) of an override's `0x47...` or placement iid."""
    if re.fullmatch(r"0x[0-9a-fA-F]{16}", value):
        ent = data.placements_by_iid.get(value.lower())
        if ent is None:
            return None
        return (f"0x{ent:08x}", value.lower(), data.table_of_entity.get(ent), f"{source}; placement {value.lower()}")
    ent = int(value, 16)
    places = data.placements.get(ent, [])
    if len(places) == 1:
        return (f"0x{ent:08x}", places[0]["iid"], data.table_of_entity.get(ent), f"{source}; placement {places[0]['iid']}")
    if places:
        return (f"0x{ent:08x}", None, data.table_of_entity.get(ent), f"{source}; {len(places)} placements, the sets name the entity")
    return None


def resolve_named(d, data, name, field):
    """An NPC or world object the steps name: (entity, iid or None, table, source), or None."""
    qid = d["vault_id"]
    ov = next((o for o in overrides(qid, field) if key(o["key"]) == key(name)), None)
    if ov:
        return placement_of(data, ov["value"], "user override: " + ov["source"])
    giver = d["resolved"].get("start_npc") or {}
    if field == "npc" and giver.get("placed") and (key(name) in plural_keys(giver.get("name") or "")
                                                     or key(name) == key(d["fields"]["start_npc"]["name"] or "")):
        return (giver["entity"], giver["iid"], int(giver["table"], 16) if giver.get("table") else None, giver["source"])
    ents = data.entities(name)
    if not ents:
        parts = name.split()
        for n in range(len(parts) - 1, 0, -1):
            ents = data.entities(" ".join(parts[:n]))
            if ents:
                break
    places = [p for e in ents for p in data.placements.get(e, [])]
    src = f"Pyrealium world {os.path.basename(data.world_path)}: an entity named '{name}'"
    if len(places) == 1:
        p = places[0]
        return (f"0x{p['entity']:08x}", p["iid"], data.table_of_entity.get(p["entity"]), f"{src}, placement {p['iid']}")
    if places and len({p["entity"] for p in places}) == 1 and max(
            dist((x["north"], x["east"]), (y["north"], y["east"])) for x in places for y in places) <= 1.5:
        e = places[0]["entity"]
        return (f"0x{e:08x}", None, data.table_of_entity.get(e), f"{src}, {len(places)} nearby placements of one entity")
    return None


def escort_entity(d, data, name):
    """(`0x47...`, source) of the creature an `escort` step names: an `npc` override's entity, else
    the one entity of that name Pyrealium spawns (StartEscort takes the nearest living one)."""
    ov = next((o for o in overrides(d["vault_id"], "npc") if key(o["key"]) == key(name)), None)
    if ov:
        if hexval(ov["value"], {0x47}) is not None:
            return (f"0x{int(ov['value'], 16):08x}", "user override: " + ov["source"])
        p = placement_of(data, ov["value"], "user override: " + ov["source"])
        return (p[0], p[3]) if p else None
    ents = [e for e in data.entities(name) if data.pack_entities.get(e) or e in data.creature_entities]
    if len(ents) == 1:
        return (f"0x{ents[0]:08x}", f"Pyrealium world {os.path.basename(data.world_path)}: the one spawned creature named '{name}'")
    return None


def resolve_item(d, data, name):
    ov = next((o for o in overrides(d["vault_id"], "item") if key(o["key"]) == key(name)), None)
    if ov:
        return (f"0x{int(ov['value'], 16):08x}", "user override: " + ov["source"])
    it = d["resolved"]["items"].get(name)
    if it and len(it["entities"]) == 1:
        return (it["entities"][0], it["source"])
    ents = data.entities(name)
    if len(ents) == 1:
        e = next(iter(ents))
        return (f"0x{e:08x}", "definitions.tsv / DAT entity name")
    return None


def giver_lines(spec_q, nt, ns, level, F, vsrc):
    """The giver's refusals: its DAT 'come back when stronger' and 'no task today' lines, or
    Messages from the Vault's requirement and reset text."""
    nsrc = lambda k: f"DAT string table 0x{nt:08x} key 0x{k:08x}"
    weak = (("Tell", f"0x{nt:08x}", f"0x{NPC_TOO_WEAK:08x}", nsrc(NPC_TOO_WEAK) + ": 'come back when stronger' (role read from its text)")
            if nt and NPC_TOO_WEAK in ns else
            ("Message", f"This quest requires level {level}.", "", f"{vsrc} requirements 'Level: {level}+' (no DAT refusal line; Pyrealium wording)"))
    wait = (("Tell", f"0x{nt:08x}", f"0x{NPC_NO_TASK:08x}", nsrc(NPC_NO_TASK) + ": 'no task today' (role read from its text)")
            if nt and NPC_NO_TASK in ns else
            ("Message", (F["repeat_seconds"]["raw"] or "You cannot repeat this quest yet.").strip(), "",
             f"{vsrc} reset timer text (no DAT line)"))
    return weak, wait


def reward_actions(d, vsrc):
    """The turn-in's rewards: (action, params, source) rows."""
    qid, rw = d["vault_id"], d["fields"]["reward"]["value"]
    out = []
    if rw.get("xp"):
        out.append(("AwardXP", [str(rw["xp"])], xp_source(rw, vsrc, f"{rw['xp']} XP")))
    elif rw.get("xp_percent"):
        cap = rw.get("xp_cap") or ""
        out.append(("AwardLevelProportionalXP", [f"{rw['xp_percent']:g}", str(rw.get("xp_min") or ""), str(cap)],
                    xp_source(rw, vsrc, f"{rw['xp_percent']:g}% XP" + (f", capped at {cap}" if cap else " (the Vault states no cap)"))))
    if rw.get("gold"):
        out.append(("AwardGold", [str(rw["gold"])], f"AC2 Vault reward: {rw['gold']} gold"))
    if rw.get("kp") and not any(o["key"] == "kp" for o in overrides(qid, "drop_reward")):
        out.append(("AwardKingdomPoints", [str(rw["kp"]), ""], f"AC2 Vault reward: {rw['kp']} Kingdom Points (no cap is stated)"))
    for ent, cnt, gsrc in reward_gives(d) or []:
        out.append(("GiveItem", [ent, str(cnt)], gsrc))
    for o in overrides(qid, "random_reward"):
        out.append(("RandomGive", [o["value"]], "user override: " + o["source"]))
    return out


def spec_from_hunt(h, data):
    """A hunt's spec: its own sets (the kills, the 'not done yet' and the hand-in) and its giver."""
    d, q, n, gp, vsrc = h["d"], h["q"], h["n"], h["gp"], h["vsrc"]
    nt, ns = h["nt"], h["ns"]
    owner_e, owner_i = h["npc"]["entity"], h["npc"]["iid"] or ""
    nsrc = lambda k: f"DAT string table 0x{nt:08x} ({data.table_name.get(nt)}) key 0x{k:08x}"
    sets, acts = list(h["sets"]), list(h["acts"])
    if gp["progress"] in ns:
        sets.append([f"{q}_busy", owner_e, owner_i, "Use", "", q, 1, n, "", "", "the NPC while the kills are under way"])
        acts.append([f"{q}_busy", 1, "Tell", 0, f"0x{nt:08x}", f"0x{gp['progress']:08x}", "", "", "", nsrc(gp["progress"]) + ": the quest's 'not done yet' line"])
    sets.append([f"{q}_turn_in", owner_e, owner_i, "Use", "", q, n + 1, n + 1, "", "", f"{vsrc}: return to the NPC for the reward"])
    acts.append([f"{q}_turn_in", 1, "Tell", 0, f"0x{nt:08x}", f"0x{gp['reward']:08x}", "", "", "", nsrc(gp["reward"]) + ": the quest's reward line"])
    order = 2
    for a, params, src in [(h["pay"][0], h["pay"][1], h["pay"][2])] + [("GiveItem", [e, str(c)], s) for e, c, s in h["gives"]]:
        acts.append([f"{q}_turn_in", order, a, 0, *(params + [""] * (5 - len(params))), src])
        order += 1
    acts.append([f"{q}_turn_in", order, "SetStage", 0, q, n + 2, "", "", "", vsrc])
    weak, wait = giver_lines(q, nt, ns, h["level"], d["fields"], vsrc)
    return {"q": q, "d": d, "reg": h["reg"], "stages": h["stages"], "sets": sets, "acts": acts,
            "giver": (owner_e, owner_i), "offer": (f"0x{nt:08x}", f"0x{gp['offer']:08x}", nsrc(gp["offer"]) + ": the offer (the quest summary quotes it)"),
            "level": h["level"], "weak": weak, "wait": wait, "done": n + 2, "vsrc": vsrc,
            "start_text": d["fields"]["start_npc"]["text"][:120], "catalog": h["catalog"], "title": d["title"]}


def spec_from_catalog(src, cq, data, prereq_key):
    """A hunting-catalog hunt the Vault has no rows for, as rows from the catalog's own values
    (Pyrealium policy, as the catalog is): so its NPC's other hunts can leave the hunting handler."""
    q = f"c{cq['id'] & 0xffff:04x}"
    goal = cq["goal"]
    csrc = f"{src} hunt 0x{cq['id']:08x} (Pyrealium catalog, policy: the Vault has no rows for it)"
    reg = [[q, f"0x{cq['id']:08x}", "0x00000000", "0x00000000", cq["title"], "", "killer", cq["level"] or "",
            cq["limit_seconds"] or "", "", cq["repeat_seconds"] or "", f"{csrc}: title, level, time limit and repeat"]]
    stages = []
    for k in range(1, goal + 1):
        stages.append([q, k, 0, f"{cq['text']} Progress: {k - 1} / {goal}.", f"{csrc}: its journal text and progress"])
    stages.append([q, goal + 1, 0, f"{cq['text']} Progress: {goal} / {goal}. Return to {cq['npc_name']}.", f"{csrc}: its journal text and progress"])
    stages.append([q, goal + 2, 1, f"{cq['title']} complete.", f"{csrc}: Pyrealium text"])
    sets, acts = [], []
    for ent in cq["targets"]:
        for k in range(1, goal + 1):
            name = f"{q}_kill_{ent:08x}_{k}"
            sets.append([name, f"0x{ent:08x}", "", "Kill", "", q, k, k, "", "", f"{csrc}: its targets"])
            acts.append([name, 1, "SetStage", 0, q, k + 1, "", "", "", f"{csrc}: each kill counts"])
    npc = data.placements.get(cq["npc"], [])
    owner_e = f"0x{cq['npc']:08x}"
    owner_i = npc[0]["iid"] if len(npc) == 1 else ""
    sets.append([f"{q}_turn_in", owner_e, owner_i, "Use", "", q, goal + 1, goal + 1, "", "", f"{csrc}: the reward at its NPC"])
    if cq["xp_basis_points"]:
        pay = ("AwardLevelProportionalXP", [f"{cq['xp_basis_points'] / 100:g}", "", str(cq["xp_cap"])])
    else:
        pay = ("AwardXP", [str(cq["xp_cap"])])
    acts.append([f"{q}_turn_in", 1, "Message", 0, f"{cq['title']} complete.", "", "", "", "", f"{csrc}: Pyrealium text"])
    acts.append([f"{q}_turn_in", 2, pay[0], 0, *(pay[1] + [""] * (5 - len(pay[1]))), f"{csrc}: its experience"])
    acts.append([f"{q}_turn_in", 3, "SetStage", 0, q, goal + 2, "", "", "", csrc])
    weak = ("Message", f"This quest requires level {cq['level']}.", "", f"{csrc}: its level (Pyrealium wording)")
    wait = ("Message", "You cannot repeat this quest yet.", "", f"{csrc}: Pyrealium wording")
    return {"q": q, "d": None, "reg": reg, "stages": stages, "sets": sets, "acts": acts, "giver": (owner_e, owner_i),
            "offer": None, "offer_text": cq["text"], "level": cq["level"], "weak": weak, "wait": wait, "done": goal + 2,
            "vsrc": csrc, "start_text": "", "catalog": (src, cq), "title": cq["title"], "prereq": prereq_key}


def spec_steps(d, data, catalog=None):
    """Rows for any walkthrough whose steps the grammar expresses (talk, kill, loot, give, use,
    open, craft, effect, lift), each NPC, creature, item and object resolved. Journal lines are
    the quest table's where one names the step's NPC, else Pyrealium text from the step (the Shelter's
    precedent). None, with the gap in d["generator"], when a step does not resolve."""
    why = d.setdefault("generator", {})
    R, F, qid = d["resolved"], d["fields"], d["vault_id"]
    fail = lambda gap, detail: (why.__setitem__("steps", f"{gap}: {detail}"), None)[1]
    giver = R.get("start_npc") or {}
    if not giver.get("placed"):
        return fail("no giver", "no placed start NPC")
    gtab = int(giver["table"], 16) if giver.get("table") else None
    steps = [s for s in F["steps"] if s["kind"] != "travel"]
    if not steps:
        return fail("Write steps", "the walkthrough has no steps the parser reads: write them (`step` overrides)")
    if not overrides(qid, "step"):
        # Parsed prose ships only when nothing in it is left out: every NPC it names is in a step,
        # and it mentions no curse, cure or blessing the steps would have to carry.
        named = {s.get("npc") or "" for s in steps} | {s.get("target") or "" for s in steps} | {F["start_npc"]["name"] or ""}
        named_keys = {key(n) for n in named if n}
        text = " ".join([d.get("_walk", ""), d.get("_desc", "")])
        for m in re.finditer(r"\b([A-Z][a-z'\-]+(?:\s+[A-Z][a-z'\-]+){0,3})\b", text):
            n = m.group(1)
            if key(n) in named_keys or any(key(n) in k or k in key(n) for k in named_keys if len(k) > 3):
                continue
            ents = data.entities(n)
            # a named NPC ([mn], [fn], [vfn] ... markers) placed in the world, or a creature Pyrealium spawns
            npc_like = [e for e in ents if "n" in data.entity_marker.get(e, "") and data.placements.get(e)]
            creature = [e for e in ents if data.pack_entities.get(e) or e in data.creature_entities]
            if npc_like or creature:
                if not R["kill_targets"].get(n):
                    return fail("Write steps", f"the walkthrough names '{n}' but no parsed step uses it: write the steps")
        if re.search(r"\b(curse[sd]?|cured?|blessing|buff)\b", text, re.I):
            return fail("Write steps", "the walkthrough mentions a curse, cure or blessing: write the steps (with `effect` / `lift`)")
    beats, offer_extra, escort, escorts = [], [], None, []
    for s in steps:
        k = s["kind"]
        if k == "talk":
            n = resolve_named(d, data, s["npc"], "npc")
            if not n:
                return fail("Identify or place", f"the NPC '{s['npc']}' (an `npc` override, or place it)")
            if not beats and n[0] == giver["entity"]:
                continue
            beats.append({"kind": "talk", "npc": n, "name": s["npc"], "after": []})
        elif k == "kill":
            kt = R["kill_targets"].get(s["target"])
            if s.get("alias"):
                continue
            if catalog and kt and (catalog[1].get("evidence") or {}).keys() & {"native_death_effects", "native_death_effect"} \
                    and not any(o["field"] == "kill_target" for o in OVERRIDES.get(qid, [])):
                kt = dict(kt, entities=[f"0x{e:08x}" for e in catalog[1]["targets"]], spawned=True,
                          source=f"the DAT NPC tables' death effects for this hunt (as {catalog[0]} cites them; DAT over Vault)")
            if not kt:
                return fail("Identify entity", f"the creature '{s['target']}' (a `kill_target` override)")
            if not kt.get("spawned"):
                return fail("Place in the world", f"the creature '{s['target']}' is not placed")
            if beats and beats[-1]["kind"] == "kill" and not beats[-1].get("drop"):
                beats[-1]["kills"].append((s["target"], kt["entities"], s["count"], kt["source"]))
            else:
                beats.append({"kind": "kill", "kills": [(s["target"], kt["entities"], s["count"], kt["source"])], "after": []})
        elif k == "loot":
            it = resolve_item(d, data, s["item"])
            if not it:
                return fail("Identify item", f"the item '{s['item']}' (an `item` override)")
            if not beats or beats[-1]["kind"] != "kill" or len(beats[-1]["kills"]) != 1:
                return fail("Write steps", f"where '{s['item']}' comes from: a `kill` step before the `loot` step")
            beats[-1]["drop"] = it
        elif k == "give":
            it = resolve_item(d, data, s["item"]) if s.get("item") else next((b["drop"] for b in reversed(beats) if b.get("drop")), None)
            if not it:
                return fail("Identify item", f"the item handed to '{s['npc']}' (an `item` override, or write the steps)")
            n = resolve_named(d, data, s["npc"], "npc")
            if not n:
                return fail("Identify or place", f"the NPC '{s['npc']}' (an `npc` override, or place it)")
            beats.append({"kind": "give", "npc": n, "name": s["npc"], "item": it, "item_name": s.get("item") or "the item", "after": []})
        elif k == "use":
            it = resolve_item(d, data, s["item"])
            obj = resolve_named(d, data, s["target"], "object")
            if not it or not obj:
                return fail("Identify or place", f"'{s['item']}' used on '{s['target']}' (`item` / `object` overrides)")
            beats.append({"kind": "use", "item": it, "obj": obj, "name": s["target"], "item_name": s["item"], "after": []})
        elif k == "open":
            obj = resolve_named(d, data, s["target"], "object")
            chest = next((o for o in overrides(qid, "chest") if key(o["key"]) == key(s["target"])), None)
            if not obj or not chest:
                return fail("Supply values", f"the object '{s['target']}' (an `object` override) and what it gives (a `chest` override)")
            gives = [(f"0x{int(e.strip(), 16):08x}", int(c) if c.strip() else 1) for e, _, c in
                     (p.strip().partition(",") for p in chest["value"].split(";"))]
            beats.append({"kind": "open", "obj": obj, "name": s["target"], "gives": gives, "src": "user override: " + chest["source"], "after": []})
        elif k == "craft":
            it = resolve_item(d, data, s["item"])
            if not it:
                return fail("Identify item", f"the crafted item '{s['item']}' (an `item` override)")
            beats.append({"kind": "craft", "item": it, "item_name": s["item"], "after": []})
        elif k in ("effect", "lift"):
            act = (("ApplyEffect", [s["effect"], str(s["seconds"])]) if k == "effect" else ("RemoveEffect", [s["effect"]]))
            (beats[-1]["after"] if beats else offer_extra).append(act + (s.get("source", "the steps"),))
        elif k == "escort":
            if escort:
                return fail("Write steps", "one escort at a time: `arrive` (or the turn-in) ends the first before another `escort`")
            ent = escort_entity(d, data, s["npc"])
            if not ent:
                return fail("Identify or place", f"the escorted '{s['npc']}': a creature Pyrealium spawns near where the escort "
                            "starts (an `npc` override naming its `0x47...`)")
            (beats[-1]["after"] if beats else offer_extra).append(
                ("StartEscort", [ent[0]], f"AC2 Vault quests/db id {qid}: escort {s['npc']}; {ent[1]}"))
            escort = {"entity": ent[0], "name": s["npc"], "from": len(beats), "to": None}
            escorts.append(escort)
        elif k == "arrive":
            obj = resolve_named(d, data, s["target"], "object")
            if not obj or not obj[1]:
                return fail("Identify or place", f"where the escort arrives, '{s['target']}' (an `object` override with a "
                            "16-digit placement iid)")
            beats.append({"kind": "arrive", "obj": obj, "name": s["target"], "range": s["range"], "escort": escort, "after": []})
            if escort:
                escort["to"] = len(beats) - 1
                escort = None
    if beats and beats[-1]["kind"] in ("talk", "give"):
        turn = beats.pop()
    else:
        turn = {"kind": "talk", "npc": (giver["entity"], giver["iid"], gtab, giver["source"]), "name": giver.get("name") or "the quest giver", "after": []}
    if not beats and not offer_extra and turn["kind"] == "talk" and turn["npc"][0] == giver["entity"]:
        return fail("Write steps", "the walkthrough gives nothing to do between the offer and the reward: write the steps")
    gives = reward_gives(d)
    if gives is None:
        return fail("Identify item", "a reward item (a `reward_item` override)")
    q = f"v{qid}"
    vsrc = f"AC2 Vault quests/db id {qid}"
    # quotes in walkthrough order, each to the first step at its speaker's table
    quotes = [x for x in R["quotes"] if x["dat"]]
    used = set()
    def quote_for(table):
        for i, x in enumerate(quotes):
            if i not in used and table is not None and int(x["dat"]["table"], 16) == table:
                used.add(i)
                return (x["dat"]["table"], x["dat"]["key"], x["dat"]["source"] + ": the line the Vault quotes")
        return None
    offer = quote_for(gtab)
    title = R.get("title") or {}
    qt = int(title["table"], 16) if title.get("table") and (title.get("table_name") or "").startswith("quest") else None
    qs = table_strings(data, qt) if qt else {}
    def dat_line(table, ret):
        if not qt or table is None:
            return None
        ident = words(data.string(table, NAME_KEY)) | words(data.string(table, DESC_KEY))
        pat = r"^Return to " if ret else r"^(Seek out|Speak to|Speak with|Talk to|Find|Go to|Bring|Take|Give) "
        hits = sorted(((len(words(x) & ident), k, x) for k, x in qs.items() if re.match(pat, x.strip()) and len(words(x) & ident) >= (1 if ret else 2)), reverse=True)
        if not hits or (len(hits) > 1 and hits[0][0] == hits[1][0]):
            return None
        return hits[0][2].strip(), f"DAT string table 0x{qt:08x} ({data.table_name.get(qt)}) key 0x{hits[0][1]:08x}: the line naming the step's NPC"
    pyrealium = f"Pyrealium journal text (no retail line is recovered for this step); steps per {vsrc}"
    stages, sets, acts = [], [], []
    st = 1
    def after_rows(name, order, b):
        for a, params, src in b["after"]:
            acts.append([name, order, a, 0, *(params + [""] * (5 - len(params))), src])
            order += 1
        return order
    for i, b in enumerate(beats):
        name = f"{q}_s{i + 1}"
        b["stage"] = st
        if b["kind"] in ("talk", "give"):
            e, iid, tab, nsrc_ = b["npc"]
            line = dat_line(tab, False)
            text = (line[0], line[1]) if line else ((f"Speak to {b['name']}." if b["kind"] == "talk" else f"Bring {b['item_name']} to {b['name']}."), pyrealium)
            stages.append([q, st, 0, text[0], text[1]])
            sets.append([name, e, iid or "", "Use", "", q, st, st, "", "", f"{vsrc}: {b['kind']} {b['name']}; {nsrc_}"])
            order = 1
            body = name
            if b["kind"] == "give":
                body = f"{name}_ok"
                sets.append([body, "", "", "QuestSuccess", "", "", "", "", "", "", f"{vsrc}: the item handed in"])
                acts.append([name, 1, "InqItem", 0, b["item"][0], body, "", "", "", b["item"][1]])
                acts.append([body, 1, "TakeItem", 0, b["item"][0], 1, "", "", "", f"{vsrc}: hand {b['item_name']} to {b['name']}"])
                order = 2
            quote = quote_for(tab)
            if quote:
                acts.append([body, order, "Tell", 0, quote[0], quote[1], "", "", "", quote[2]])
                order += 1
            order = after_rows(body, order, b)
            acts.append([body, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
            st += 1
        elif b["kind"] == "kill":
            kills = b["kills"]
            if len(kills) == 1:
                target, ents, n, ksrc = kills[0]
                for k in range(n):
                    stages.append([q, st + k, 0, f"Kill {n} {target}: {n - k} left.", pyrealium])
                    for ent in ents:
                        sn = f"{q}_k{i + 1}_{ent[2:]}_{k + 1}"
                        sets.append([sn, ent, "", "Kill", "", q, st + k, st + k, "", "", f"{ksrc}; {vsrc}: kill {n} {target}"])
                        order = 1
                        if k == n - 1 and b.get("drop"):
                            acts.append([sn, order, "GiveItem", 0, b["drop"][0], 1, "", "", "", f"{b['drop'][1]}; Pyrealium policy: the last kill drops it"])
                            order += 1
                        if k == n - 1:
                            order = after_rows(sn, order, b)
                        acts.append([sn, order, "SetStage", 0, q, st + k + 1, "", "", "", vsrc])
                st += n
            else:
                # several kill tasks at once: a counter each, the stage moves when every one is done
                stages.append([q, st, 0, "Kill " + " and ".join(f"{n} {t}" for t, _, n, _ in kills) + ".", pyrealium])
                done_set = f"{q}_k{i + 1}_done"
                sets.append([done_set, "", "", "QuestSuccess", "", "", "", "", "", "", f"{vsrc}: every kill task done"])
                chain = []
                for c, (t, ents, n, ksrc) in enumerate(kills, 1):
                    chain.append((c, n))
                checks = []
                for c, n in chain:
                    nm = f"{q}_k{i + 1}_check{c}"
                    checks.append(nm)
                    sets.append([nm, "", "", "QuestSuccess", "", "", "", "", "", "", "the kill counters checked in turn"])
                for j, (c, n) in enumerate(chain):
                    nxt = checks[j + 1] if j + 1 < len(checks) else done_set
                    acts.append([checks[j], 1, "InqCounter", 0, q, c, n, nxt, "", "the kill counters checked in turn"])
                for c, (t, ents, n, ksrc) in enumerate(kills, 1):
                    for ent in ents:
                        sn = f"{q}_k{i + 1}_{c}_{ent[2:]}"
                        sets.append([sn, ent, "", "Kill", "", q, st, st, "", "", f"{ksrc}; {vsrc}: kill {n} {t}"])
                        acts.append([sn, 1, "IncrementCounter", 0, q, c, n, "", "", f"{vsrc}: kill {n} {t}"])
                        acts.append([sn, 2, "InqStage", 0, q, st, st, checks[0], "", "every counter full?"])
                order = after_rows(done_set, 1, b)
                acts.append([done_set, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
                st += 1
        elif b["kind"] == "use":
            e, iid, tab, osrc = b["obj"]
            stages.append([q, st, 0, f"Use {b['item_name']} on {b['name']}.", pyrealium])
            sets.append([name, e, iid or "", "ItemUsedOn", b["item"][0], q, st, st, "", "", f"{vsrc}: use {b['item_name']} on {b['name']}; {osrc}"])
            order = after_rows(name, 1, b)
            acts.append([name, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
            st += 1
        elif b["kind"] == "open":
            e, iid, tab, osrc = b["obj"]
            stages.append([q, st, 0, f"Open {b['name']}.", pyrealium])
            sets.append([name, e, iid or "", "Use", "", q, st, st, "", "", f"{vsrc}: open {b['name']}; {osrc}"])
            order = 1
            for ent, cnt in b["gives"]:
                acts.append([name, order, "GiveItem", 0, ent, cnt, "", "", "", b["src"]])
                order += 1
            order = after_rows(name, order, b)
            acts.append([name, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
            st += 1
        elif b["kind"] == "arrive":
            e, iid, tab, osrc = b["obj"]
            who = b["escort"]["name"] if b["escort"] else None
            stages.append([q, st, 0, f"Bring {who} to {b['name']}." if who else f"Go to {b['name']}.", pyrealium])
            sets.append([name, e, iid, "Enter", "", q, st, st, f"{b['range']:g}", "",
                         f"{vsrc}: arrive at {b['name']}; {osrc}; the radius is Pyrealium policy"])
            order = 1
            if who:
                acts.append([name, order, "EndEscort", 0, "", "", "", "", "", f"{vsrc}: the escort arrives"])
                order += 1
            order = after_rows(name, order, b)
            acts.append([name, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
            st += 1
        elif b["kind"] == "craft":
            stages.append([q, st, 0, f"Craft {b['item_name']}.", pyrealium])
            sets.append([name, b["item"][0], "", "Crafted", "", q, st, st, "", "", f"{vsrc}: craft {b['item_name']}; {b['item'][1]}"])
            order = after_rows(name, 1, b)
            acts.append([name, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
            st += 1
    # the turn-in
    e, iid, tab, tsrc_ = turn["npc"]
    line = dat_line(tab, True)
    stages.append([q, st, 0, line[0] if line else f"Return to {turn['name']}.", line[1] if line else pyrealium])
    tname = f"{q}_turn_in"
    sets.append([tname, e, iid or "", "Use", "", q, st, st, "", "", f"{vsrc}: the reward at {turn['name']}; {tsrc_}"])
    body, order = tname, 1
    if turn["kind"] == "give":
        body = f"{tname}_ok"
        sets.append([body, "", "", "QuestSuccess", "", "", "", "", "", "", f"{vsrc}: the item handed in"])
        acts.append([tname, 1, "InqItem", 0, turn["item"][0], body, "", "", "", turn["item"][1]])
        acts.append([body, 1, "TakeItem", 0, turn["item"][0], 1, "", "", "", f"{vsrc}: hand {turn['item_name']} to {turn['name']}"])
        order = 2
    quote = quote_for(tab)
    if quote:
        acts.append([body, order, "Tell", 0, quote[0], quote[1], "", "", "", quote[2]])
        order += 1
    for a, params, src in reward_actions(d, vsrc):
        acts.append([body, order, a, 0, *(params + [""] * (5 - len(params))), src])
        order += 1
    for a, params, src in turn["after"]:
        acts.append([body, order, a, 0, *(params + [""] * (5 - len(params))), src])
        order += 1
    if escorts and escorts[-1]["to"] is None:
        acts.append([body, order, "EndEscort", 0, "", "", "", "", "", f"{vsrc}: the escort is delivered"])
        order += 1
    acts.append([body, order, "SetStage", 0, q, st + 1, "", "", "", vsrc])
    # the escort's death, while it walks with the player, fails the quest back to its offer
    stage_of = lambda i: beats[i]["stage"] if i is not None and i < len(beats) else st
    for n, run in enumerate(escorts, 1):
        lost = f"{q}_escort_lost{n}"
        sets.append([lost, run["entity"], "", "EscortLost", "", q, stage_of(run["from"]), stage_of(run["to"]), "", "",
                     f"{vsrc}: escort {run['name']}"])
        acts.append([lost, 1, "Message", 0, ESCORT_LOST[0], "", "", "", "", ESCORT_LOST[1]])
        acts.append([lost, 2, "SetStage", 0, q, 0, "", "", "", ESCORT_LOST[1]])
        left = f"{q}_escort_abandoned{n}"
        sets.append([left, run["entity"], "", "EscortAbandoned", "", q, stage_of(run["from"]), stage_of(run["to"]), "", "",
                     f"{vsrc}: escort {run['name']} left behind"])
        acts.append([left, 1, "Message", 0, ESCORT_ABANDONED[0], "", "", "", "", ESCORT_ABANDONED[1]])
        acts.append([left, 2, "SetStage", 0, q, 0, "", "", "", ESCORT_ABANDONED[1]])
    stages.append([q, st + 1, 1, f"{d['title']} complete.", f"Pyrealium journal text (no retail completed line is recovered); {vsrc}"])
    if st + 1 > 250:
        return fail("Write steps", "too many stages")
    jid, jsrc = journal_id(d)
    if catalog:
        jid, jsrc = catalog[1]["id"], f"the id of the {catalog[0]} hunt this quest replaces (kept: its journal entries and progress carry over)"
    credit = next((o["value"] for o in overrides(qid, "credit")), "killer")
    level, tl, rs = F["level"]["value"] or 0, F["time_limit_seconds"]["value"], F["repeat_seconds"]["value"]
    ttab = title.get("table") or "0x00000000"
    tkey = title.get("key") or "0x00000000"
    reg = [[q, f"0x{jid:08x}", ttab, tkey, re.sub(r"\[[^\]]*\]", "", title.get("text") or d["title"]).strip(), "", credit,
            level or "", tl or "", "", rs or "",
            f"Title: {title.get('source', 'the Vault')}; journal id: {jsrc}; level, time limit and repeat: {vsrc}"
            + (f"; credit: user override" if overrides(qid, "credit") else "")]]
    ns = table_strings(data, gtab) if gtab else {}
    weak, wait = giver_lines(q, gtab, ns, level, F, vsrc)
    for a, params, src in offer_extra:
        pass
    why["steps"] = "generated"
    return {"q": q, "d": d, "reg": reg, "stages": stages, "sets": sets, "acts": acts,
            "giver": (giver["entity"], giver["iid"] or ""), "offer": offer, "offer_extra": offer_extra,
            "level": level, "weak": weak, "wait": wait, "done": st + 1, "vsrc": vsrc,
            "start_text": F["start_npc"]["text"][:120], "catalog": catalog, "title": d["title"]}


def front_door(specs):
    """The giver's Use sets for its quests, in order: each quest's offer at its stage 0 (a
    prerequisite unsolved, a level not reached or a repeat not due falls on to the next quest's
    offer or repeat, the last to the first quest's refusal line), then each repeat at its
    completed stage."""
    sets, acts = [], []
    S = lambda *r: sets.append(list(r))
    A = lambda *r: acts.append(list(r))
    e, iid = specs[0]["giver"]
    head = specs[0]
    for x in ("weak", "wait"):
        name = f"{head['q']}_refuse_{x}"
        S(name, "", "", "QuestFailure", "", "", "", "", "", "", "the giver's refusal")
        line = head[x]
        if line[0] == "Tell":
            A(name, 1, "Tell", 0, line[1], line[2], "", "", "", line[3])
        else:
            A(name, 1, "Message", 0, line[1], "", "", "", "", line[3])
    for i, sp in enumerate(specs):
        q, vsrc = sp["q"], sp["vsrc"]
        for x in ("weak", "wait"):
            nm = f"{q}_next_{x}"
            S(nm, "", "", "QuestFailure", "", "", "", "", "", "", "falls on to the giver's next quest")
            if i + 1 < len(specs):
                nq = specs[i + 1]
                A(nm, 1, "InqStage", 0, nq["q"], 0, 0, f"{nq['q']}_ask", f"{nq['q']}_next_{x}_again", "Pyrealium policy: the giver offers its quests in turn")
                S(f"{nq['q']}_next_{x}_again", "", "", "QuestFailure", "", "", "", "", "", "", "falls on")
                A(f"{nq['q']}_next_{x}_again", 1, "InqStage", 0, nq["q"], nq["done"], nq["done"], f"{nq['q']}_again", f"{nq['q']}_next_{x}", "Pyrealium policy")
            else:
                A(nm, 1, "InqStage", 0, q, 0, 255, f"{head['q']}_refuse_{x}", "", "the giver's refusal")
        S(f"{q}_ask", e, iid, "Use", "", q, 0, 0, "", "", f"{vsrc}: '{sp['start_text']}'")
        S(f"{q}_gate", "", "", "QuestSuccess", "", "", "", "", "", "", "prerequisite solved")
        S(f"{q}_able", "", "", "QuestSuccess", "", "", "", "", "", "", "level reached")
        S(f"{q}_start", "", "", "QuestSuccess", "", "", "", "", "", "", "a new run may start")
        if sp.get("prereq"):
            A(f"{q}_ask", 1, "InqSolved", 0, sp["prereq"], f"{q}_gate", f"{q}_next_wait", "", "", f"{vsrc}: its prerequisite")
        else:
            A(f"{q}_ask", 1, "InqLevel", 0, q, f"{q}_able", f"{q}_next_weak", "", "", f"{vsrc} requirements")
        A(f"{q}_gate", 1, "InqLevel", 0, q, f"{q}_able", f"{q}_next_weak", "", "", f"{vsrc} requirements")
        A(f"{q}_able", 1, "InqQuestReady", 0, q, f"{q}_start", f"{q}_next_wait", "", "", f"{vsrc} reset timer")
        order = 1
        if sp.get("offer"):
            A(f"{q}_start", order, "Tell", 0, sp["offer"][0], sp["offer"][1], "", "", "", sp["offer"][2])
            order += 1
        elif sp.get("offer_text"):
            A(f"{q}_start", order, "Message", 0, sp["offer_text"], "", "", "", "", sp["vsrc"] + ": its text")
            order += 1
        for a, params, src in sp.get("offer_extra", []):
            A(f"{q}_start", order, a, 0, *(params + [""] * (5 - len(params))), src)
            order += 1
        A(f"{q}_start", order, "SetStage", 0, q, 1, "", "", "", vsrc)
    for sp in specs:
        q = sp["q"]
        S(f"{q}_again", e, iid, "Use", "", q, sp["done"], sp["done"], "", "", f"{sp['vsrc']}: its repeat")
        A(f"{q}_again", 1, "InqLevel", 0, q, f"{q}_able", f"{q}_next_weak", "", "", f"{sp['vsrc']} requirements")
    return sets, acts


def ship_rows(drafts):
    """Every quest whose rows generate and that nothing stops ships: the hunts (DAT phases), the
    catalog hunts their NPCs also give (rows from the catalog's values), and any walkthrough the
    step grammar expresses. Quests whose prerequisite does not ship wait for it."""
    reg, stages, sets, acts = [], [], [], []
    shipped, retired = [], []
    for d in drafts:
        classify(d, DATA)
    hunting = {}
    for src, rows in DATA.catalog_hunts.items():
        for cq in rows:
            hunting.setdefault(cq["npc"], []).append((src, cq))
    specs = {}
    for d in drafts:
        cat = None
        cands = [(src, cq) for src, cq in DATA.catalog_quest_rows.get(key(d["title"]), []) if src.startswith("hunting")]
        if len(cands) == 1:
            cat = cands[0]
        other = [o for o in d["overlaps"] if o["by"] in ("journal id", "title") and not any(c.startswith("hunting") for c in o["catalogs"])]
        if other:
            d["blocks"].append(("Runs as a Pyrealium catalog quest", "operated", "; ".join(", ".join(o["catalogs"]) for o in other)
                                + " already runs it (Pyrealium's recovered quests)"))
            continue
        h = hunt_parts(d, DATA, cat)
        if h and not (cat and int(h["npc"]["entity"], 16) != cat[1]["npc"]):
            if cat:
                d["parity"] = parity(d, h, DATA)
            specs[d["vault_id"]] = spec_from_hunt(h, DATA)
            continue
        sp = spec_steps(d, DATA, cat)
        if sp:
            if cat and int(sp["giver"][0], 16) != cat[1]["npc"]:
                d["blocks"].append(("Identify or place", "you", f"its giver is not the catalog hunt's NPC 0x{cat[1]['npc']:08x}"))
                continue
            specs[d["vault_id"]] = sp
        elif not d["blocks"]:
            reason = d.get("generator", {}).get("steps", "no generator matches its steps")
            gap = reason.split(":")[0]
            cat_name = {"Write steps": "Write steps", "Identify entity": "Identify entity or item", "Identify item": "Identify entity or item",
                        "Identify or place": "Place in the world or identify", "Place in the world": "Place in the world",
                        "Supply values": "Supply values", "no giver": "Place in the world or identify"}.get(gap, "Write steps")
            d["blocks"].append((cat_name, "you", reason))
    # catalog NPCs: every catalog hunt of the NPC needs rows (the Vault's, else the catalog's own)
    by_catalog = {sp["catalog"][1]["id"]: sp for sp in specs.values() if sp.get("catalog")}
    extra = []
    vault_titles = {key(x["title"]) for x in drafts}
    for npc, lst in hunting.items():
        mine = [by_catalog.get(cq["id"]) for _, cq in lst]
        if not any(mine):
            continue
        for (src, cq), sp in zip(lst, mine):
            if sp is None and key(cq["title"]) in vault_titles:
                continue  # the Vault quest of that title has no rows yet: its own gaps hold the NPC
            if sp is None:
                extra.append(spec_from_catalog(src, cq, DATA, None))
    for sp in extra:
        by_catalog[sp["catalog"][1]["id"]] = sp
    # prerequisites: the Vault's titles, else the catalog's prerequisite field
    title_to = {key(sp["title"]): sp for sp in list(specs.values()) + extra}
    for sp in list(specs.values()) + extra:
        d = sp["d"]
        pre = None
        names = d["fields"]["prerequisites"]["value"] if d else []
        for n in names:
            if key(n) in title_to:
                pre = title_to[key(n)]
        if not pre and sp.get("catalog") and sp["catalog"][1].get("prerequisite"):
            pre = by_catalog.get(sp["catalog"][1]["prerequisite"])
        if d and names and not pre:
            known = any(key(n) == key(x["title"]) for x in drafts for n in names)
            d["blocks"].append(("Waits for its prerequisite" if known else "Identify prerequisite", "you",
                                f"prerequisite {names}: " + ("ships when that quest ships" if known else "no Vault quest of that title (a `prerequisite` override)")))
        sp["prereq_spec"] = pre
        sp["prereq"] = pre["q"] if pre else None
    # what ships: no blocks, and the prerequisite ships (fixpoint)
    ok = {id(sp) for sp in list(specs.values()) + extra if (sp["d"] is None or not sp["d"]["blocks"])}
    changed = True
    while changed:
        changed = False
        for sp in list(specs.values()) + extra:
            if id(sp) in ok and sp.get("prereq_spec") is not None and id(sp["prereq_spec"]) not in ok:
                ok.discard(id(sp))
                changed = True
                if sp["d"]:
                    sp["d"]["blocks"].append(("Waits for its prerequisite", "you", f"'{sp['prereq_spec']['title']}' does not ship yet"))
    # a catalog NPC retires only when every hunt it gives ships; until then the hunting handler
    # answers the NPC, so no Vault quest at it can run
    for npc, lst in hunting.items():
        sps = [by_catalog.get(cq["id"]) for _, cq in lst]
        at_npc = [sp for sp in list(specs.values()) + extra if int(sp["giver"][0], 16) == npc]
        if not at_npc:
            continue
        if not all(sp is not None and id(sp) in ok for sp in sps):
            waiting = [cq["title"] for (_, cq), sp in zip(lst, sps) if sp is None or id(sp) not in ok]
            for sp in at_npc:
                ok.discard(id(sp))
                if sp["d"] and not sp["d"]["blocks"]:
                    sp["d"]["blocks"].append(("Waits for its NPC's other hunts", "you",
                                              "the hunting catalog answers this NPC until all its hunts ship: " + ", ".join(waiting)))
    # group by giver and build
    groups = {}
    for sp in list(specs.values()) + extra:
        if id(sp) in ok:
            groups.setdefault(sp["giver"], []).append(sp)
    for giver, lst in sorted(groups.items()):
        ordered = []
        rest = list(lst)
        while rest:
            nxt = [sp for sp in rest if sp.get("prereq_spec") is None or sp["prereq_spec"] not in rest]
            nxt.sort(key=lambda sp: (sp["d"]["vault_id"] if sp["d"] else 10 ** 6, sp["q"]))
            ordered.append(nxt[0])
            rest.remove(nxt[0])
        for sp in ordered:
            reg += sp["reg"]; stages += sp["stages"]; sets += sp["sets"]; acts += sp["acts"]
            d = sp["d"]
            if d:
                d["shipped"] = True
                shipped.append(d)
                d["status"] = "resolved"
            if sp.get("catalog"):
                src, cq = sp["catalog"]
                changed_ = d.get("parity", {}).get(src, {}) if d else {}
                if d:
                    d["retired"] = {"catalog": src, "id": f"0x{cq['id']:08x}", "changed": changed_}
                retired.append([f"0x{cq['id']:08x}", sp["q"], cq["title"], src,
                                ("replaced by the AC2 Vault rows " + sp["q"] + "; the Vault's values win where it gives them: "
                                 + ("; ".join(f"{k} {v[1]} -> {v[0]}" for k, v in changed_.items()) or "at parity")) if d else
                                "rows from the catalog's own values (Pyrealium policy), so its NPC's Vault hunts can leave the hunting handler"])
        s2, a2 = front_door(ordered)
        sets += s2; acts += a2
    return shipped, reg, stages, sets, acts, retired



HEADERS = {
    "quest-registry": "quest\tjournal_id\ttitle_table\ttitle_key\ttitle\tgoal\tcredit\tmin_level\ttime_limit_seconds\tmax_solves\trepeat_seconds\tsource",
    "quest-stages": "quest\tstage\tcompleted\tjournal_text\tsource",
    "emote-sets": "set\towner_entity\towner_iid\ttrigger\titem\tquest\tstage_min\tstage_max\trange\tprobability\tsource",
    "emote-actions": "set\torder\taction\tdelay_ms\tp1\tp2\tp3\tp4\tp5\tsource",
}


def import_tables(content, rows):
    """Writes the rows as the four vault TSVs in a temporary directory and imports them into
    `content`'s quest database as the `vault` family."""
    with tempfile.TemporaryDirectory() as directory:
        write_tables(directory, rows)
        tool = os.environ.get("PYREALIUM_CONTENT")
        command = [tool] if tool else ["cargo", "run", "-q", "-p", "pyrealium", "--bin", "pyrealium-content", "--"]
        subprocess.run(command + ["import-tsv", directory, "vault", content], check=True)


def write_tables(directory, rows):
    for name, body in rows.items():
        path = os.path.join(directory, f"{name}-vault.tsv")
        with open(path, "w", encoding="utf-8") as f:
            f.write(HEADERS[name] + "\n")
            f.write("# Generated by tools/import_vault_quests.py from the AC2 Vault quest library; do not edit. "
                    "See reports/vault-quests-coverage.md.\n")
            for r in body:
                # a cell is one line: DAT text keeps its line breaks as spaces
                f.write("\t".join("" if v is None else re.sub(r"\s+", " ", str(v)).strip() for v in r) + "\n")


# ---------------------------------------------------------------------------------------------
# report


MISSING_KINDS = [
    ("journal id and title string", "journal id"),
    ("kill target not placed by Pyrealium", "entities are not placed"),
    ("start NPC entity", "entity (no DAT"),
    ("start NPC not named", "start NPC (the Vault names none"),
    ("start NPC placement", "start NPC"),
    ("quoted lines without a DAT match", "quoted lines have no DAT"),
    ("kill target entity", "kill target"),
    ("reward item entity", "reward item"),
    ("experience percentage", "experience percentage"),
    ("no parseable steps", "no parseable steps"),
]


def missing_kind(m):
    for label, needle in MISSING_KINDS:
        if needle in m:
            return label
    return "other"


def report(drafts, path, data, args):
    total = len(drafts)
    by = Counter(d["status"] for d in drafts)
    shipped = [d for d in drafts if d.get("shipped")]
    gaps = Counter(g for d in drafts for g in d["gaps"])
    sole = Counter(d["gaps"][0] for d in drafts if len(d["gaps"]) == 1)
    clean = [d for d in drafts if not d["missing"] and not d["overlaps"]]
    out = []
    w = out.append
    w("# AC2 Vault quests in Pyrealium: coverage\n")
    w("Generated by `tools/import_vault_quests.py` (do not edit). Inputs: the AC2 Vault quest library "
      f"({total} quests), the DAT index (7230), Pyrealium's prepared world `{os.path.basename(args.world)}` "
      f"({sum(len(v) for v in data.placements.values())} placements) and the emulator's `content/`. "
      "Per-quest drafts with every resolved field, its confidence and its source are in "
      "`reports/vault-quests.drafts.json`.\n")
    w("Evidence ranking: client/DAT, then AC2 Vault, then AC2Hero, then Pyrealium policy. A quest ships only when "
      "every field it needs resolves, today's engine has every feature it needs, and no older catalog "
      "already runs it.\n")
    w("## Totals\n")
    w("| Status | Quests |\n|---|---|")
    w(f"| Shipped (rows in `content/*-vault.tsv`) | {len(shipped)} |")
    w(f"| Fully resolved, not shipped | {by['resolved'] - len(shipped)} |")
    w(f"| Partially resolved (no engine gap; data missing) | {by['partial']} |")
    w(f"| Blocked by an engine gap | {by['blocked']} |")
    w(f"| ...of which every parsed field also resolves and no catalog overlaps | "
      f"{sum(1 for d in clean if d['status'] == 'blocked')} |")
    w(f"| **All** | {total} |\n")
    w("Phase 2 gave the engine the registry's level, time limit, repeat delay and solve limit, GiveItem of "
      "any item entity, AwardGold, InqSolved (prerequisites), InqLevel and InqQuestReady (refusal lines). "
      "A gap the engine now has no longer blocks; the rest do (`remaining_gaps` in the drafts). A quest "
      "ships only when its rows can also be generated: so far the hunt shape (one NPC, N kills of one "
      "creature, the journal phases and NPC lines from the DAT, an experience reward).\n")
    w("| Gap closed (Phase 2, in order) | Quests shipped by it |\n|---|---|")
    w(f"| level requirement + time limit + repeat timer | {len(shipped)} (every shipped quest needs all three) |")
    w("| item grants (any item entity) | 0 (no generated shape grants items yet) |")
    w("| gold reward, prerequisite quests | 0 (no generated shape uses them yet) |\n")
    gens = Counter(d.get("generator", {}).get("hunt", "") for d in drafts)
    w("Why the hunt generator passed over the rest:\n")
    w("| Reason | Quests |\n|---|---|")
    for r, c in gens.most_common():
        w(f"| {r} | {c} |")
    w("")
    res = Counter()
    for d in drafts:
        R = d["resolved"]
        res["journal id (DAT quest)"] += bool(R.get("journal"))
        res["start coordinates"] += bool(R.get("start_cell"))
        res["start NPC named"] += bool(d["fields"]["start_npc"]["name"])
        res["start NPC placed in Pyrealium's world"] += bool(R.get("start_npc") and R["start_npc"].get("placed"))
        res["...high confidence"] += bool(R.get("start_npc") and R["start_npc"].get("confidence") == "high")
        res["has quoted dialogue"] += bool(R["quotes"])
        res["every quote matched to a DAT string"] += bool(R["quotes"]) and all(q["dat"] for q in R["quotes"])
        res["has kill steps"] += bool([s for s in d["fields"]["steps"] if s["kind"] == "kill"])
        res["every kill target resolved"] += bool(R["kill_targets"]) and not any("kill target" in m for m in d["missing"])
        res["kill targets from the DAT's death effects"] += any(t["confidence"] == "high" for t in R["kill_targets"].values())
        res["named effects (eff_*.wc)"] += bool(R["effects"])
    w("### Resolution\n")
    w("| Field | Quests |\n|---|---|")
    for k, v in res.items():
        w(f"| {k} | {v} |")
    qn = sum(len(d["resolved"]["quotes"]) for d in drafts)
    qm = sum(1 for d in drafts for q in d["resolved"]["quotes"] if q["dat"])
    qh = sum(1 for d in drafts for q in d["resolved"]["quotes"] if q["dat"] and q["dat"]["confidence"] == "high")
    w(f"| quoted lines matched / all quoted lines | {qm} / {qn} ({qh} high confidence) |\n")
    mk = Counter(missing_kind(m) for d in drafts for m in d["missing"])
    mq = Counter(k for d in drafts for k in {missing_kind(m) for m in d["missing"]})
    w("### Missing data\n")
    w("| Missing | Quests |\n|---|---|")
    for k, v in mq.most_common():
        w(f"| {k} | {v} |")
    w("")
    w("## Engine gaps, ranked by the quests each blocks\n")
    w("`Needs` counts every quest that needs the feature; `Still blocks` the quests it blocks now "
      "(features the engine has show `has`); `Sole gap` the quests it alone still blocks.\n")
    rem = Counter(g for d in drafts for g in d["remaining_gaps"])
    rsole = Counter(d["remaining_gaps"][0] for d in drafts if len(d["remaining_gaps"]) == 1)
    w("| Rank | Feature | Needs | Still blocks | Sole gap |\n|---|---|---|---|---|")
    for n, (g, c) in enumerate(gaps.most_common(), 1):
        w(f"| {n} | {g} | {c} | {'has' if g in ENGINE_HAS else rem.get(g, 0)} | {rsole.get(g, 0)} |")
    w("")
    unneeded = [g for g in ("Kill-set Tell (a line on a kill)", "several kill counters in one quest", "delays (delay_ms)")
                if not gaps.get(g)]
    if unneeded:
        w("Detected for no quest: " + "; ".join(unneeded) + " (delays: the Vault's prose gives no timed beats; "
          "a kill-time chat line is quoted by no walkthrough).\n")
    soft = Counter(g for d in drafts for g in d["soft_gaps"])
    if soft:
        w("Not blocking (an existing pattern stands in): " + "; ".join(f"{g}: {c} quests" for g, c in soft.items()) + ".\n")
    # greedy: which feature to build next, counting quests that become fully runnable
    w("### Build order\n")
    w("Features in the order that makes the most quests runnable, counting only quests whose parsed data "
      "already resolves fully and that no catalog runs (cumulative). The data gaps above cap this; each "
      "quest's missing fields are listed per quest below.\n")
    w("| Step | Implement | Quests runnable (cumulative, data complete) | Quests unblocked (cumulative, any data) |\n|---|---|---|---|")
    def gain(gset, pool):
        return sum(1 for d in pool if set(d["gaps"]) <= gset)
    done = set(ENGINE_HAS)
    remaining = set(gaps) - done
    step = 0
    w(f"| 0 | (in the engine: {', '.join(sorted(ENGINE_HAS))}) | {gain(done, clean)} | {gain(done, drafts)} |")
    while remaining:
        best = max(sorted(remaining), key=lambda g: (gain(done | {g}, clean), gain(done | {g}, drafts), gaps[g]))
        done.add(best)
        remaining.discard(best)
        step += 1
        w(f"| {step} | {best} | {gain(done, clean)} | {gain(done, drafts)} |")
    w("")
    combos = Counter(" + ".join(d["gaps"]) for d in drafts if d["gaps"])
    w("### Most common gap sets\n")
    w("| Gaps | Quests | ...data complete, no overlap |\n|---|---|---|")
    for c, n in combos.most_common(12):
        w(f"| {c} | {n} | {sum(1 for d in clean if ' + '.join(d['gaps']) == c)} |")
    w("")
    w("## Overlaps with the older hand-coded catalogs\n")
    w("An operated catalog quest keeps running until its rows here replace it in one change: two handlers "
      "for one journal id would double-award, and `server.rs` dispatches the hunting/exploration NPC "
      "handlers before quest emotes, so a Use set on such an NPC never runs. The catalog wins today. "
      "The data tables should win once a quest's rows reach parity (NPC, targets, goal, reward, timers), "
      "because they carry a source per field where the catalogs carry policy defaults; retire the "
      "catalog entry in the same commit. `npc-dialogue-v1` profiles are information-only topics that "
      "`server.rs` runs after quest emotes, so vault rows win there without a change (listed separately).\n")
    w("| Vault quest | Overlap | Catalogs | Winner today | Why |\n|---|---|---|---|---|")
    for d in drafts:
        for o in d["overlaps"]:
            w(f"| {d['title']} | {o['by']} | {', '.join(o['catalogs'])} | {o['winner']} | {o['why']} |")
    w("")
    ret = [d for d in drafts if d.get("retired")]
    w("### Retired catalog hunts\n")
    w("These catalog hunts are their Vault rows: every catalog hunt of the NPC has generated rows, the "
      "rows keep the catalog's id (the journal entry and progress carry over; `Store::migrate_legacy_quests` "
      "moves a character's state from when hunts were code onto them), and the targets stay the "
      "catalog's where it took them from the DAT's NPC tables (DAT over Vault). Where the Vault gives a "
      "value, the Vault's wins; the catalogs' XP caps, which the Vault does not state, are gone.\n")
    w("| Vault quest | Catalog hunt | Fields changed (catalog -> Vault) |\n|---|---|---|")
    for d in ret:
        r = d["retired"]
        w(f"| {d['title']} | {r['catalog']} {r['id']} | " + ("; ".join(f"{k}: {v[1]} -> {v[0]}" for k, v in r["changed"].items()) or "none") + " |")
    w("")
    par = [d for d in drafts if d.get("parity") and not d.get("retired")]
    w("### Catalog quests whose Vault rows generate but stay with the catalog\n")
    w("Rows generate for these, but another catalog hunt of the same NPC has none (the hunting handler "
      "answers the NPC as a whole), so the catalog keeps running them. `Parity` lists the fields that "
      "differ (Vault rows / catalog).\n")
    w("| Vault quest | Catalog | Parity |\n|---|---|---|")
    for d in par:
        for src, diff in d["parity"].items():
            w(f"| {d['title']} | {src} | " + ("at parity" if not diff else "; ".join(f"{k}: {v[0]} / {v[1]}" for k, v in diff.items())) + " |")
    w("")
    soft_o = [d["title"] for d in drafts if d["soft_overlaps"]]
    w(f"Start NPCs that an older catalog only places, or that have an information-only `npc-dialogue-v1` "
      f"profile (vault rows win, nothing to retire): {len(soft_o)} quests: " + ", ".join(soft_o) + ".\n")
    w("## Per quest\n")
    w("| Id | Quest | Status | Journal | Start NPC (iid, confidence) | Quotes DAT/all | Gaps | Missing |\n|---|---|---|---|---|---|---|---|")
    for d in sorted(drafts, key=lambda d: (d["status"], d["title"])):
        R = d["resolved"]
        j = R["journal"]["id"] if R.get("journal") else ""
        npc = R.get("start_npc") or {}
        ns = (f"{npc.get('name')} {npc.get('iid', '')} ({npc.get('confidence')})" if npc else "")
        qs = R["quotes"]
        st = "shipped" if d.get("shipped") else d["status"]
        miss = "; ".join(d["missing"]).replace("|", "/")
        w(f"| {d['vault_id']} | {d['title'].replace('|', '/')} | {st} | {j} | {ns.replace('|', '/')} | "
          f"{sum(1 for q in qs if q['dat'])}/{len(qs)} | {', '.join(d['gaps'])} | {miss} |")
    w("")
    w("## Method and limits\n")
    w("- Steps come from regular expressions over the walkthrough prose (talk, kill N X, loot, give, use, "
      "travel); quoted dialogue is excluded from step parsing. A prose shape the patterns miss leaves the "
      "quest partial or under-gapped, never shipped. `data complete` means every field the parser found "
      "resolved; it is not a review of the walkthrough.")
    w("- Names resolve through `content/definitions.tsv` and the DAT's own name strings (key 0xa8607cec of "
      "the entity's `.stt` table). Kill targets prefer the DAT's death effects that name the quest's journal "
      "id (`native-quests.json` entity bindings, confidence high); otherwise same-named entities placed in "
      "Pyrealium's world (medium) or any same-named entity (low).")
    w("- Journal ids: the DAT quest list by name; 7230 keeps most quest names in private tables, so the "
      "quest package (`QuestDarkstone`) is matched too, exactly or by its words (medium).")
    w("- NPC placements: Pyrealium's world placements of a same-named entity within 1.0 map units (about 200 m) "
      "of the Vault's start coordinates; interior cells use their landblock's centre and 1.8 units.")
    w("- Quotes: 7230 string-table text by FTS phrase, then similarity >= 0.75 (high >= 0.9, or the Vault "
      "quoting part of the line); localisation copies (`_english.stt_ansi` and others) are skipped.")
    w("- Effects: names of two words or more of `eff_*.stt` tables (key 0xa8607cec) found in the prose with "
      "their capitals, with their `eff_*.wc` WSTATE. A quest that mentions a curse/buff/effect without a "
      "name match still counts under ApplyEffect.")
    w("- Gaps come from the Vault's fields (level, time limit, reset timer, reward) and prose keywords; "
      "the keyword gaps are conservative (a mention of a fellowship or a chest counts).")
    open(path, "w", encoding="utf-8").write("\n".join(out) + "\n")


# ---------------------------------------------------------------------------------------------


GAP_KINDS = [
    # (needle in a missing line, gap, who)
    ("journal id", "Journal / title", "us (Pyrealium id like the Shelter)"),
    ("prerequisite quests", "Prerequisite link", "us"),
    ("start NPC (the Vault names none", "Start NPC unknown", "you"),
    ("entity (no DAT", "Identify DAT entity", "you"),
    ("override (", "Place in the world", "you"),
    ("placement", "Place in the world", "you"),
    ("kill target '", "Identify DAT entity", "you"),
    ("reward item", "Reward item to identify", "you"),
    ("experience percentage", "Exact XP rule", "you"),
    ("quoted lines have no DAT", "NPC line not found in DAT", "you"),
    ("no parseable steps", "Steps to write", "you"),
]


GAP_GROUPS = {
    "Place in the world": "Place in the world builder",
    "Place in the world or identify": "Place in the world builder",
    "Identify DAT entity": "Identify an entity or item",
    "Reward item to identify": "Identify an entity or item",
    "Identify entity or item": "Identify an entity or item",
    "Start NPC unknown": "Identify an entity or item",
    "Identify prerequisite": "Identify an entity or item",
    "Identify or place": "Identify an entity or item",
    "Exact XP rule": "Supply values",
    "Supply values": "Supply values",
    "Steps to write": "Write steps",
    "Write steps": "Write steps",
    "Other data": "Write steps",
    "Decide": "Decide",
    "Waits for its prerequisite": "Waits for another quest",
    "Waits for its NPC's other hunts": "Waits for another quest",
    "Engine feature": "Engine (us)",
}


def gap_rows(drafts):
    """Every quest not shipped, one row per thing that stops it, grouped by what closes it: the
    world builder, an override (identify, supply values, write steps), a decision, another quest
    shipping first, or engine work (`us`). Quoted lines with no DAT string are listed as optional.
    Quests a Pyrealium catalog already runs are left out."""
    rows = []
    for d in sorted(drafts, key=lambda d: d["title"]):
        if d.get("shipped") or any(w == "operated" for _, w, _ in d.get("blocks", [])):
            continue
        link = f"http://localhost:8080/index.html?dir=quests%2Fdb&content=view&id={d['vault_id']}"
        for gap, who, detail in d.get("blocks", []):
            group = GAP_GROUPS.get(gap, "Write steps" if who == "you" else "Engine (us)")
            if who == "decision":
                group = "Decide"
            rows.append([d["title"], d["vault_id"], group, "us" if group == "Engine (us)" else "you", f"{gap}: {detail}", d["status"], link])
        for gap, who, detail in d.get("optional", []):
            rows.append([d["title"], d["vault_id"], "Optional (NPC lines)", "you", f"{gap}: {detail}", d["status"], link])
    return rows


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--vault", required=True)
    p.add_argument("--dat-index", required=True)
    p.add_argument("--world", required=True)
    p.add_argument("--content", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "content"))
    p.add_argument("--drafts")
    p.add_argument("--report")
    p.add_argument("--ship")
    p.add_argument("--overrides", help="the hand-edited overrides (default: content/vault-quest-overrides.tsv)")
    p.add_argument("--gaps", help="write the per-quest gap list (TSV: quest, vault_id, gap, who, detail, draft, link)")
    a = p.parse_args()
    a.content = os.path.abspath(a.content)
    data = Data(a)
    v = sqlite3.connect(f"file:{a.vault}?mode=ro", uri=True)
    v.row_factory = sqlite3.Row
    data.vault_titles = {i: t.strip().rstrip(":").strip() for i, t in v.execute("SELECT id, title FROM quests")}
    n = load_overrides(a.overrides or os.path.join(a.content, "vault-quest-overrides.tsv"), set(data.vault_titles))
    if n:
        print(f"{n} user overrides", file=sys.stderr)
    links = {"creatures": defaultdict(list), "trophies": defaultdict(list)}
    for name, quest in v.execute("SELECT c.name, q.quest FROM creature_quests q JOIN creatures c ON c.id=q.creature_id"):
        links["creatures"][quest.strip()].append(name)
    for name, quest in v.execute("SELECT t.name, q.quest FROM trophy_quests q JOIN trophies t ON t.id=q.trophy_id"):
        links["trophies"][quest.strip()].append(name)
    drafts = [draft(r, data, links) for r in v.execute("SELECT * FROM quests ORDER BY id")]
    global DATA
    DATA = data
    shipped, reg, stages, sets, acts, retired = ship_rows(drafts)
    if a.ship:
        import_tables(a.ship, {"quest-registry": reg, "quest-stages": stages, "emote-sets": sets, "emote-actions": acts})
    if a.drafts:
        # one quest per line, so a regenerated file diffs per quest
        with open(a.drafts, "w", encoding="utf-8") as f:
            f.write('{"generator": "tools/import_vault_quests.py", "world": %s, "quests": [\n'
                    % json.dumps(os.path.basename(a.world)))
            f.write(",\n".join(json.dumps(d, ensure_ascii=False, sort_keys=True, default=str) for d in drafts))
            f.write("\n]}\n")
    if a.report:
        report(drafts, a.report, data, a)
    if a.gaps:
        rows = gap_rows(drafts)
        with open(a.gaps, "w", encoding="utf-8") as f:
            f.write("quest\tvault_id\tgap\twho\tdetail\tdraft\tlink\n")
            for r in rows:
                f.write("\t".join(re.sub(r"\s+", " ", str(x)) for x in r) + "\n")
        print(f"{len(rows)} gaps in {len({r[1] for r in rows})} quests", file=sys.stderr)
    c = Counter(d["status"] for d in drafts)
    print(f"{len(drafts)} quests: {len(shipped)} shipped, {c['resolved']} resolved, {c['partial']} partial, {c['blocked']} blocked",
          file=sys.stderr)


if __name__ == "__main__":
    main()
