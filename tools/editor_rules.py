"""World-builder rules: custom creature stats, behaviour, loot, patrols and groups.

Mirrors src/world_rules.rs. The file sits beside its world overlay as
NAME.rules.tsv and is custom operator content, not recovered retail data.
Targets are a creature type (47xxxxxx) or one placement (8140xxxxxxxxxxxx).
"""
from pathlib import Path
import math

from editor_document import atomic_write
from place_content import bounded_text

HEADER = "pyrealium-world-rules-v1"
# name, label, kind, low, high
STATS = (("health", "Health", int, 1, 5_000_000), ("damage", "Damage per hit", int, 1, 5_000_000),
         ("armor", "Armor", int, 0, 1_000_000), ("level", "Level", int, 0, 999),
         ("xp", "XP reward", int, 0, 2**63-1), ("gold", "Gold reward", int, 0, 2**31-1),
         ("interval_ms", "Attack interval (ms)", int, 250, 60_000), ("range", "Attack range (m)", float, .5, 30),
         ("leash", "Leash radius (m)", float, 1, 500), ("respawn_ms", "Respawn (ms)", int, 1000, 86_400_000))
BEHAVIOUR = (("perception", "Aggro radius (m)", float, 0, 500), ("homesick", "Homesick radius (m)", float, 0, 500),
             ("wandering", "Wanders", bool, 0, 1), ("wander_range", "Wander range (m)", float, 0, 500),
             ("wander_probability", "Wander chance / s", float, 0, 1), ("aggressive", "Attacks on sight", bool, 0, 1),
             ("idle_only", "Idle only (never moves)", bool, 0, 1), ("loot_proof", "Drops no loot", bool, 0, 1))


def rules_path(world_path):
    path = Path(world_path)
    return path.with_name(path.stem + ".rules.tsv")


def check_target(text, placement_only=False):
    text = text.strip().lower()
    try:
        value = int(text, 16)
    except ValueError:
        value = -1
    if len(text) == 16 and value >> 48 == 0x8140 and value != 0x8140 << 48:
        return text
    if len(text) == 8 and value >> 24 == 0x47 and not placement_only:
        return text
    raise ValueError("Target must be a placement id (8140…)" + ("" if placement_only else " or a creature type (47xxxxxx)"))


def check_value(spec, text):
    name, label, kind, low, high = spec
    text = str(text).strip()
    if text in ("", "-"):
        return None
    if kind is bool:
        if text not in ("0", "1"):
            raise ValueError(f"{label}: choose yes, no or inherit")
        return text
    try:
        value = kind(text)
    except ValueError as error:
        raise ValueError(f"{label}: enter {'a whole number' if kind is int else 'a number'}") from error
    if not math.isfinite(value) or not low <= value <= high:
        raise ValueError(f"{label}: must be within {low:g}..{high:g}")
    return str(value) if kind is int else format(value, ".6g")


class Rules:
    def __init__(self):
        self.stats, self.behaviour, self.loot, self.patrols, self.groups = {}, {}, {}, {}, []

    @classmethod
    def parse(cls, text):
        rules = cls()
        lines = (text or HEADER + "\n").splitlines()
        if not lines or lines[0] != HEADER:
            raise ValueError("Not a world rules file (" + HEADER + ")")
        for number, line in enumerate(lines[1:], 2):
            if not line or line.startswith("#"):
                continue
            try:
                rules._row(line.split("\t"))
            except (ValueError, IndexError) as error:
                raise ValueError(f"Rules line {number}: {error}") from error
        return rules

    def _row(self, f):
        kind = f[0]
        if kind == "stats" and len(f) == 12:
            target = check_target(f[1])
            values = {spec[0]: check_value(spec, value) for spec, value in zip(STATS, f[2:])}
            self.set_stats(target, values)
        elif kind == "ai" and len(f) == 10:
            target = check_target(f[1])
            self.set_behaviour(target, {spec[0]: check_value(spec, value) for spec, value in zip(BEHAVIOUR, f[2:])})
        elif kind == "loot" and len(f) == 4:
            target = check_target(f[1])
            items = [] if f[3] == "-" else f[3].split(",")
            self.loot.setdefault(target, []).append((int(f[2]), items))
            self.set_loot(target, self.loot[target])
        elif kind == "patrol" and len(f) == 4:
            points = [tuple(float(v) for v in point.split(",")) for point in f[3].split(";")]
            self.set_patrol(check_target(f[1], True), int(f[2]), points)
        elif kind == "group" and len(f) == 5:
            self.groups.append(dict(id=int(f[1], 16), capacity=int(f[2]),
                                    regeneration_ms=None if f[3] == "-" else int(f[3]),
                                    members=[check_target(m, True) for m in f[4].split(",")]))
            self.check_groups()
        else:
            raise ValueError(f"unrecognized row {kind!r}")

    # Mutators validate exactly as the server does, so a saved file always loads.
    def set_stats(self, target, values):
        values = {k: v for k, v in values.items() if v is not None}
        if values.get("health") == "0" or values.get("damage") == "0":
            raise ValueError("Health and damage must be above zero")
        self.stats.pop(target, None)
        if values:
            self.stats[target] = values

    def set_behaviour(self, target, values):
        values = {k: v for k, v in values.items() if v is not None}
        self.behaviour.pop(target, None)
        if values:
            self.behaviour[target] = values

    def set_loot(self, target, outcomes):
        cleaned = []
        for weight, items in outcomes:
            if not 1 <= int(weight) <= 1_000_000:
                raise ValueError("Loot weights must be 1..1000000")
            items = [item.strip().lower() for item in items]
            if len(items) > 32 or any(len(i) != 8 or int(i, 16) >> 24 != 0x47 for i in items):
                raise ValueError("A loot outcome holds up to 32 item definitions (47xxxxxx)")
            cleaned.append((int(weight), items))
        if len(cleaned) > 256:
            raise ValueError("A loot table holds at most 256 outcomes")
        self.loot.pop(target, None)
        if cleaned:
            self.loot[target] = cleaned

    def set_patrol(self, actor, pause_ms, points):
        actor = check_target(actor, True)
        if not points:
            self.patrols.pop(actor, None)
            return
        if not 0 <= int(pause_ms) <= 600_000:
            raise ValueError("Patrol pause must be 0..600000 ms")
        if len(points) > 64 or any(len(p) != 2 or not all(math.isfinite(v) and abs(v) <= 500 for v in p) for p in points):
            raise ValueError("A patrol has up to 64 waypoints within 500 m of the spawn")
        self.patrols[actor] = (int(pause_ms), [(float(x), float(y)) for x, y in points])

    def check_groups(self):
        seen, ids = set(), set()
        for group in self.groups:
            if not 0 < group["id"] < 2**63-1 or group["id"] in ids:
                raise ValueError("Duplicate or invalid spawn group id")
            ids.add(group["id"])
            if not 1 <= len(group["members"]) <= 64 or not 1 <= group["capacity"] <= 64:
                raise ValueError("A spawn group has 1..64 members and capacity 1..64")
            regen = group["regeneration_ms"]
            if regen is not None and not 1000 <= regen <= 86_400_000:
                raise ValueError("Group respawn must be 1000..86400000 ms")
            if len(set(group["members"])) != len(group["members"]) or seen & set(group["members"]):
                raise ValueError("A placement can belong to only one spawn group")
            seen |= set(group["members"])

    def group_of(self, actor):
        return next((group for group in self.groups if actor in group["members"]), None)

    def remove_placement(self, actor):
        """Drop every rule that names a deleted placement."""
        for table in (self.stats, self.behaviour, self.loot, self.patrols):
            table.pop(actor, None)
        for group in self.groups:
            if actor in group["members"]:
                group["members"].remove(actor)
                group["capacity"] = min(group["capacity"], max(1, len(group["members"])))
        self.groups = [group for group in self.groups if group["members"]]

    def dumps(self):
        out = [HEADER, "# Written by the Pyrealium world builder. Custom server content, not retail data.",
               "# stats  target " + " ".join(s[0] for s in STATS),
               "# ai     target " + " ".join(s[0] for s in BEHAVIOUR),
               "# loot   target weight items(comma, or - for nothing)",
               "# patrol placement pause_ms dx,dy;dx,dy…   (metres from spawn)",
               "# group  hex_id capacity respawn_ms|- members(comma)"]
        for target in sorted(self.stats):
            out.append("\t".join(["stats", target] + [self.stats[target].get(s[0], "-") for s in STATS]))
        for target in sorted(self.behaviour):
            out.append("\t".join(["ai", target] + [self.behaviour[target].get(s[0], "-") for s in BEHAVIOUR]))
        for target in sorted(self.loot):
            for weight, items in self.loot[target]:
                out.append("\t".join(["loot", target, str(weight), ",".join(items) or "-"]))
        for actor in sorted(self.patrols):
            pause, points = self.patrols[actor]
            out.append("\t".join(["patrol", actor, str(pause), ";".join(f"{x:.6g},{y:.6g}" for x, y in points)]))
        for group in self.groups:
            out.append("\t".join(["group", f"{group['id']:x}", str(group["capacity"]),
                                  "-" if group["regeneration_ms"] is None else str(group["regeneration_ms"]),
                                  ",".join(group["members"])]))
        return "\n".join(out) + "\n"


def load_rules(world_path):
    """The world's creature rules: its content database's rule rows, as rules text."""
    import editor_content
    text = editor_content.rules_text(world_path)
    return Rules.parse(text), text


def save_rules(world_path, expected, rules):
    """Writes the rules into the world's content database; refuses if they changed elsewhere."""
    import editor_content
    current = editor_content.rules_text(world_path)
    if current != expected:
        raise ValueError("The world's rules changed outside the editor. Reload before saving.")
    text = rules.dumps()
    Rules.parse(text)
    if len(text.encode("utf-8")) > 4 * 1024 * 1024:
        raise ValueError("Rules exceed 4 MiB")
    editor_content.write_rules(world_path, text)
    return editor_content.rules_text(world_path)


class Bestiary:
    """Native creature defaults, loot items and observed bundles from content/."""

    def __init__(self, content_folder):
        folder = Path(content_folder)
        self.combat, self.ai, self.names, self.items, self.native_loot = {}, {}, {}, {}, {}
        for name in ("creature-combat.tsv", "inferred-creature-combat-v1.tsv", "regional-creature-combat-v1.tsv"):
            for f in self._rows(folder / name):
                if len(f) in (13, 14) and f[0].startswith("47"):
                    # entity health damage interval armor xp level variance attack defense perception homesick type [vigor]
                    self.combat.setdefault(f[0], dict(health=f[1], damage=f[2], interval_ms=f[3] if f[3] != "0" else "1500",
                        armor=f[4], xp=f[5], level=f[6], leash=f[11] if float(f[11]) > 0 else "18"))
        for name in ("creature-ai.tsv", "inferred-creature-ai-v1.tsv", "regional-creature-ai-v1.tsv"):
            for f in self._rows(folder / name):
                if len(f) == 11:
                    self.ai.setdefault(f[0], dict(perception=f[1], homesick=f[2], wandering=f[3], wander_range=f[4],
                        wander_probability=f[5], aggressive=f[7], idle_only=f[8], loot_proof=f[10]))
        for f in self._rows(folder / "definitions.tsv"):
            if len(f) == 3:
                self.names[f[0].lower()] = f[2].split("[")[0].strip() or f[2]
        for f in self._rows(folder / "loot-items.tsv"):
            if len(f) == 3:
                self.items[f[0].lower()] = f[2]
        for f in self._rows(folder / "observed-loot.tsv"):
            if len(f) == 4:
                self.native_loot.setdefault(f[0].lower(), []).append((int(f[2]), f[3].split(",")))

    @staticmethod
    def _rows(path):
        if not path.exists():
            return []
        return [line.split("\t") for line in bounded_text(path).splitlines()[1:] if line and not line.startswith("#")]

    def is_creature(self, entity):
        return entity.lower() in self.combat

    def name(self, entity):
        return self.names.get(entity.lower(), entity)

    def item_name(self, item):
        return self.items.get(item.lower(), item)

    def effective(self, rules, entity, actor):
        """(stats, behaviour) as the server will apply them, with each value's source."""
        entity, actor = entity.lower(), (actor or "").lower()
        stats = {name: (value, "native") for name, value in self.combat.get(entity, {}).items()}
        stats.setdefault("range", ("3", "native"))
        stats.setdefault("gold", ("2" if entity in self.combat else "0", "native"))
        stats.setdefault("respawn_ms", ("180000" if actor.startswith("8140c00000") else "120000" if actor.startswith("8140a0") else "30000", "server policy"))
        behaviour = {name: (value, "native") for name, value in self.ai.get(entity, {}).items() if value != "-"}
        for source, target in (("type", entity), ("placement", actor)):
            for name, value in rules.stats.get(target, {}).items():
                stats[name] = (value, source)
            for name, value in rules.behaviour.get(target, {}).items():
                behaviour[name] = (value, source)
        if stats.get("leash", (0, "native"))[1] == "native":
            # The leash the server applies: a homesick radius or the aggro radius can set it.
            leash = self.roaming(rules, entity, actor, (stats, behaviour))["leash"]
            if float(stats.get("leash", ("18",))[0]) != leash:
                stats["leash"] = (format(leash, "g"), "from homesick/aggro radius")
        return stats, behaviour

    def roaming(self, rules, entity, actor, effective=None):
        """Radii (m) for map overlays: aggro, wander and leash."""
        stats, behaviour = effective or self.effective(rules, entity, actor)
        number = lambda table, name, default=0.: float(table.get(name, (default,))[0] or default)
        # Same order as Encounters: native leash, perception floor without a
        # homesick radius, custom homesick radius, then an explicit leash.
        leash = float(self.combat.get(entity.lower(), {}).get("leash", 18))
        if "homesick" not in behaviour:
            leash = max(leash, number(behaviour, "perception"))
        if behaviour.get("homesick", (0, "native"))[1] != "native" and number(behaviour, "homesick") > 0:
            leash = number(behaviour, "homesick")
        if stats.get("leash", (0, "native"))[1] != "native":
            leash = number(stats, "leash", 18.)
        idle = behaviour.get("idle_only", ("0",))[0] == "1"
        wanders = (not idle and behaviour.get("wandering", ("-",))[0] != "0"
                   and number(behaviour, "wander_range") > 0 and behaviour.get("wander_probability", ("-",))[0] != "0")
        return dict(aggro=number(behaviour, "perception"),
                    wander=min(number(behaviour, "wander_range"), leash) if wanders else 0., leash=leash)
