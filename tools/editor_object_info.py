"""Read-only data for the object selected in the world builder.

Everything here is data the server already loads: creature combat records and the
operator's rules, spawn groups (generators) with their member chances and respawn,
portal routes, NPC dialogue (npc-dialogue-v1.json and the emote sets), loot, and
every quest that names the object or its type (the content database's rows, `content_files`). Nothing is written.
"""
import json
from pathlib import Path
import sqlite3
import struct

from editor_rules import spawn_shares
import content_files
from place_content import bounded_text


def _rows(path):
    if not path.exists():
        return []
    return [line.split("\t") for line in bounded_text(path).splitlines()[1:] if line and not line.startswith("#")]


def _json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def _hex(value):
    """'0x4700157d', '4700157d' or an int -> '4700157d' (lowercase, no prefix)."""
    if isinstance(value, int):
        return f"{value:08x}" if value < 1 << 32 else f"{value:016x}"
    text = (value or "").strip().lower()
    return text[2:] if text.startswith("0x") else text


def seconds(ms):
    ms = int(float(ms))
    if ms % 60000 == 0 and ms >= 60000:
        return f"{ms // 60000} min"
    return f"{ms / 1000:g} s"


class ObjectData:
    """Loaded once per editor session from content/ (and the DAT index for text, if present)."""

    def __init__(self, content_folder, dat_index=None):
        folder = Path(content_folder)
        self.dialogue, self.emotes, self.actions, self.quests = {}, {}, {}, {}
        try:
            dialogue = json.loads(content_files.text(folder / "npc-dialogue-v1.json") or "{}")
        except ValueError:
            dialogue = {}
        for profile in dialogue.get("profiles", []):
            self.dialogue[_hex(profile["entity"])] = profile
        # Quests and their emote sets are content database rows (a hunt is a Kill set on its
        # target's type).
        db = content_files._database(folder)
        if db is not None:
            self.quests = dict(db.execute("SELECT name, title FROM quest"))
            text = lambda v: "" if v is None else str(v)
            for name, entity, iid, trigger, item, quest, low, high, source in db.execute(
                    "SELECT name, owner_entity, owner_iid, trigger, item, quest, stage_min, stage_max, source FROM emote_set"):
                row = dict(set=name, entity=_hex(entity), iid=_hex(iid), trigger=trigger, item=text(item), quest=text(quest),
                           stages=(text(low), text(high)), source=source)
                for key in filter(None, (row["iid"], row["entity"])):
                    self.emotes.setdefault(key, []).append(row)
            for f in db.execute("SELECT emote_set, ord, action, delay_ms, p1, p2, p3, p4, p5 FROM emote_action"):
                self.actions.setdefault(f[0], []).append([text(v) for v in f])
        native = _json(folder / "native-quests.json")
        names = {q["id"]: (q.get("name") or {}).get("text") or q.get("package") for q in native.get("quests", [])}
        self.native_deaths = {}
        for binding in native.get("entity_bindings", []):
            effects = [(e.get("effect"), names.get(e.get("quest")) or f"quest {e.get('quest', 0):08x}")
                       for e in binding.get("death_effects", []) if e.get("quest")]
            if effects:
                self.native_deaths[_hex(binding["entity"])] = effects
        self.overrides = {}
        for f in _rows(folder / "vault-quest-overrides.tsv"):
            if len(f) >= 4:
                for token in f[3].replace(",", " ").split():
                    if token.lower().startswith("0x47") or token.lower().startswith("0x8140"):
                        self.overrides.setdefault(_hex(token), []).append(f)
        self.records, self.portal_dat = {}, None
        self.strings = None
        if dat_index and Path(dat_index).exists():
            try:
                self.strings = sqlite3.connect(f"file:{dat_index}?mode=ro", uri=True)
            except sqlite3.Error:
                self.strings = None

    def text(self, table, key):
        """A DAT 7230 string, if the developer DAT index is present."""
        if self.strings is None:
            return None
        try:
            row = self.strings.execute("SELECT text FROM strings WHERE release='7230' AND table_did=? AND string_id=?",
                                       (int(table, 16), int(key, 16))).fetchone()
        except (sqlite3.Error, ValueError):
            return None
        return row[0] if row else None

    # ---- sections ---------------------------------------------------------
    def describe(self, editor, identity):
        """[(heading, [lines])] for one object id (int)."""
        iid = f"{identity:016x}"
        row = next((r for r in editor.placements if r[0].lower() == iid), None)
        base = editor.prepared_rows().get(iid) or {}
        entity = (row[1] if row else (base.get("raw") or [None, base.get("entity")])[1] or "").lower()
        if not entity:
            return [("Object", [f"{iid} is not in the working copy or the prepared world."])]
        population = getattr(editor, "population", None)
        bestiary = population.bestiary if population else None
        name = base.get("name") or (bestiary.name(entity) if bestiary else entity)
        cell = row[3] if row else (base.get("raw") or [None] * 4)[3]
        sections = [("Object", [f"{name}", f"Placement {iid}", f"Type {entity}" + (f" · cell {cell}" if cell else ""),
                                "In your content file" if row else "Prepared world (read only until moved)"])]
        sections += self.portal(editor, identity)
        if population is not None:
            sections += self.creature(population, entity, iid, editor)
            sections += self.generator(population, entity, iid)
        sections += self.dialogue_for(entity, iid)
        sections += self.quest_ties(entity, iid)
        return sections

    def portal(self, editor, identity):
        route = getattr(editor, "portal_routes", {}).get(f"{identity:016x}")
        if route is None:
            return []
        cell, position = route["destination"]
        place = editor.atlas.place_name(cell, position) if hasattr(editor, "atlas") else ""
        return [("Portal", [f"Destination: {place}".rstrip(": "), f"Cell {cell:08x} at "
                            + ", ".join(f"{v:.1f}" for v in position), "Double-click or T goes through."])]

    def dat_record(self, editor, entity):
        """The type's portal.dat entity record (parents merged), as the 2012 server's
        Entity.LoadFromPortalDat reads it; {} when there is no DAT folder or record."""
        if entity in self.records:
            return self.records[entity]
        record = {}
        try:
            folder = Path(editor.game.get()) if hasattr(editor, "game") else None
            if folder and (folder / "portal.dat").exists():
                from ac2_dat import Dat
                from import_creature_combat import load_index, read_entity
                if self.portal_dat is None or self.portal_dat[0] != folder:
                    archive = Dat(folder / "portal.dat")
                    self.portal_dat = (folder, archive, load_index(archive))
                record = read_entity(self.portal_dat[1], self.portal_dat[2], int(entity, 16))
        except (OSError, ValueError, KeyError, struct.error) as error:
            record = {"error": str(error)}
        self.records[entity] = record
        return record

    def creature(self, population, entity, iid, editor=None):
        bestiary, rules = population.bestiary, population.rules
        record = self.dat_record(editor, entity) if editor is not None else {}
        combat = bestiary.is_creature(entity) or rules.stats.get(entity) or rules.stats.get(iid)
        if not combat and not any(k in record for k in ("health", "damage", "level", "xp")):
            return []
        sections = []
        if record.get("error"):
            sections.append(("From the DAT", [f"portal.dat record {entity} could not be read: {record['error']}"]))
        elif record:
            dat = [f"portal.dat entity record {entity} (parents merged)"]
            def put(label, key, fmt=str):
                if record.get(key) not in (None, 0, 0.0):
                    dat.append(f"{label} {fmt(record[key])}")
            put("Health", "health"); put("Max vigor", "max_vigor")
            if record.get("damage"):
                variance = record.get("variance")
                spread = f" ±{100 * variance:.0f}%" if variance and 0 < variance <= 1 else ""
                time = record.get("time")
                speed = f" every {time:g} s" if time and time < 240 else f" every {seconds(time)}" if time else " (no natural weapon time)"
                dat.append(f"Natural damage {record['damage']}{spread}{speed}")
            put("Armor", "armor"); put("Level", "level"); put("Death experience", "xp", lambda v: f"{v:,}")
            put("Perception radius", "perception", lambda v: f"{v:g} m"); put("Homesick radius", "homesick", lambda v: f"{v:g} m")
            put("Damage type", "damage_type")
            skills, levels = record.get("skills", {}), record.get("skill_levels", {})
            if skills:
                dat.append("Starting skills (skill id @ level): " + ", ".join(f"{skills[slot]:x}@{levels.get(slot, 0)}" for slot in sorted(skills)))
            sections.append(("From the DAT", dat))
        stats, behaviour = bestiary.effective(rules, entity, iid)
        def value(name, fmt=str):
            item = stats.get(name)
            return f"{fmt(item[0])} ({item[1]})" if item else "-"
        group = rules.group_of(iid)
        respawn = (seconds(group["regeneration_ms"]) + " (spawn group)" if group and group["regeneration_ms"]
                   else value("respawn_ms", seconds))
        if combat:
            table = bestiary.combat_table.get(entity)
            lines = ["Applied by Pyrealium" + (f" (combat row in {table})" if table else " (your rules)") + ":",
                     f"Health {value('health')}", f"Damage {value('damage')} every {value('interval_ms', seconds)}",
                     f"Armor {value('armor')} · level {value('level')}", f"Experience {value('xp', lambda v: f'{int(v):,}')}",
                     f"Gold {value('gold')} · attack range {value('range', lambda v: f'{v} m')}",
                     f"Leash {value('leash', lambda v: f'{v} m')}", f"Respawn {respawn}"]
        else:
            lines = ["Not a combat creature on the server: it has a DAT record but no combat row and no rules."
                     " Give it Health and Damage on the Creatures page to make it one."]
        aggressive = behaviour.get("aggressive")
        if aggressive:
            lines.append(f"Aggressive {'yes' if aggressive[0] == '1' else 'no'} ({aggressive[1]})")
        loot = rules.loot.get(iid) or rules.loot.get(entity)
        source = "your rules"
        if loot is None:
            loot, source = bestiary.native_loot.get(entity), "observed in captures"
        if loot:
            total = sum(weight for weight, _ in loot)
            lines.append(f"Loot ({source}, {len(loot)} outcomes):")
            for weight, items in sorted(loot, key=lambda o: -o[0])[:12]:
                names = ", ".join(bestiary.item_name(i) for i in items) or "nothing"
                lines.append(f"  {100 * weight / total:5.1f}%  {names}")
            if len(loot) > 12:
                lines.append(f"  … {len(loot) - 12} more")
        sections.append(("Server", lines))
        return sections

    def generator(self, population, entity, iid):
        rules = population.rules
        group = rules.group_of(iid)
        if group is None:
            return []
        shares = spawn_shares(group)
        respawn = seconds(group["regeneration_ms"]) if group["regeneration_ms"] else "each member's own"
        lines = [f"Spawn group {group['id']:x}: {group['capacity']} alive at once of {len(group['members'])} points",
                 f"Respawn {respawn} after a death and corpse removal",
                 "Each free slot rolls once among the ready members with a chance (ACE SelectAProfile);"
                 " members marked 'always' fill on their own."]
        for member in group["members"]:
            row = population.find(member)
            label = population.label(row) if row else "(not placed)"
            share = shares[member]
            chance = group["chances"].get(member)
            own = ""
            if not group["regeneration_ms"] and row:
                stats, _ = population.bestiary.effective(rules, row["entity"], member)
                own = f" · respawn {seconds(stats['respawn_ms'][0])}"
            what = "always" if share is None else f"{100 * share:.1f}% of a roll (chance {100 * chance:g}%)"
            lines.append(f"  {label}: {what}{own}" + ("  ← this" if member == iid else ""))
        return [("Generator", lines)]

    def dialogue_for(self, entity, iid):
        lines = []
        profile = self.dialogue.get(entity)
        if profile:
            lines.append(f"{profile.get('name') or entity} ({profile.get('role') or 'npc'}):")
            for topic in profile.get("topics", [])[:8]:
                lines.append(f"  [{topic.get('title')}] {topic.get('text', '')}")
        for emote in self.emotes.get(iid, []) + self.emotes.get(entity, []):
            when = emote["trigger"] + (f" with {emote['item']}" if emote["item"] else "")
            if emote["quest"]:
                when += f" · quest {self.quests.get(emote['quest'], emote['quest'])} stage {emote['stages'][0] or '*'}–{emote['stages'][1] or '*'}"
            lines.append(f"On {when}  ({emote['set']})")
            for action in sorted(self.actions.get(emote["set"], []), key=lambda f: int(f[1]) if f[1].isdigit() else 0):
                kind, params = action[2], [p for p in action[4:9] if p]
                if kind == "Tell" and len(params) >= 2:
                    said = self.text(params[0], params[1])
                    lines.append(f"  Tell: {said!r}" if said else f"  Tell string {params[0]}/{params[1]}")
                else:
                    lines.append(f"  {kind} {' '.join(params)}".rstrip())
        return [("Dialogue", lines)] if lines else []

    def quest_ties(self, entity, iid):
        lines = []
        for emote in self.emotes.get(iid, []) + self.emotes.get(entity, []):
            if emote["trigger"] == "Kill" and emote["quest"]:
                lines.append(f"Death advances {self.quests.get(emote['quest'], emote['quest'])} ({emote['set']})")
        for effect, quest in self.native_deaths.get(entity, []):
            lines.append(f"Death advances {quest} ({effect}, native WLib)")
        quests = sorted({e["quest"] for e in self.emotes.get(iid, []) + self.emotes.get(entity, []) if e["quest"] and e["trigger"] != "Kill"})
        for quest in quests:
            lines.append(f"Quest dialogue: {self.quests.get(quest, quest)}")
        for f in self.overrides.get(iid, []) + self.overrides.get(entity, []):
            lines.append(f"Vault quest {f[0]} {f[1]} {f[2]}: {f[4] if len(f) > 4 else ''}".rstrip())
        return [("Quests", lines)] if lines else []


def render(sections):
    return "\n\n".join(heading.upper() + "\n" + "\n".join(lines) for heading, lines in sections)
