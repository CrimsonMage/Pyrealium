"""World-builder creature tools: zone creature list, roaming/patrol overlays and
the creature inspector (stats, behaviour, loot, spawn groups and patrols).

Rules are saved to NAME.rules.tsv beside the content file (see editor_rules).
Creatures from a read-only base world can be inspected and given placement
rules too: the server keys rules by placement id, whichever file placed it.
"""
from pathlib import Path
from types import SimpleNamespace
import math
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

from editor_rules import BEHAVIOUR, STATS, Bestiary, Rules, check_value, load_rules, save_rules
from place_content import bounded_text

COLORS = dict(leash="#8a4b55", aggro="#e0913a", wander="#58b97a", patrol="#4fc3e8",
              group="#a77be0", mine="#72b9e5", base="#7d8a96", selected="#efb65e")


def outdoor(cell):
    return 1 <= (cell & 65535) <= 64


def read_reference(path):
    """Lenient reader for any world manifest version (the prepared base world is v4)."""
    lines = bounded_text(Path(path)).splitlines()
    if not lines or not lines[0].startswith("pyrealium-world-v"):
        raise ValueError("Base world must be a Pyrealium world file (the prepared world the server loads)")
    rows = []
    for line in lines[1:]:
        if not line or line.startswith("#"):
            continue
        f = line.split("\t")
        if len(f) >= 7:
            rows.append(dict(id=f[0].lower(), entity=f[1].lower(), cell=int(f[3], 16),
                             x=float(f[4]), y=float(f[5]), z=float(f[6]),
                             name=f[12] if len(f) >= 13 and f[12] != "-" else None, source="base",
                             raw=f[:12] if len(f) >= 11 else None))
    return rows


class Population:
    def __init__(self, editor):
        self.editor = editor
        self.bestiary = Bestiary(Path(editor.args.catalog).parent)
        self.rules, self.rules_text = Rules(), None
        self.rules_world = None
        self.map_reference = None
        self.map_hits = []
        self.reference = []
        self.reference_error = None
        self.selected = None
        self.inspector = None
        self.patrol_capture = None
        self.search = tk.StringVar()
        self.zone_only = tk.BooleanVar(value=True)
        self.show = {name: tk.BooleanVar(value=value) for name, value in (
            ("Base world", True), ("Aggro", False), ("Wander", True), ("Leash", False),
            ("Patrols", True), ("Groups", True), ("Labels", True))}
        self.base_path = tk.StringVar(value=editor.settings.get("reference_world", ""))

    # ---- data -------------------------------------------------------------
    def reload(self):
        """Called after the content file (re)loads; rules follow their world file."""
        path = self.editor.loaded_path
        if path is None:
            return
        if self.rules_world != path:
            self.load_reference()
        try:
            self.rules, self.rules_text = load_rules(path)
            self.rules_world = path
        except (OSError, ValueError) as error:
            self.rules_world = None
            messagebox.showerror("World rules could not be loaded", str(error), parent=self.editor.root)
        self.refresh()

    def load_reference(self):
        text = self.base_path.get().strip()
        self.reference, self.reference_error = [], None
        self.editor.content_revision += 1  # the 3D scene redraws with the new base world
        if text:
            try:
                self.reference = read_reference(text)
            except (OSError, ValueError) as error:
                self.reference_error = str(error)
                self.editor.status.set(f"Base world not loaded: {error}")

    def pick_base(self):
        path = filedialog.askopenfilename(title="Base world (read-only), e.g. the prepared legions-world.tsv",
                                          filetypes=[("World content", "*.tsv")])
        if path:
            self.base_path.set(path)
            self.load_reference()
            self.refresh()

    def mine(self):
        return [dict(id=r[0].lower(), entity=r[1].lower(), cell=int(r[3], 16), x=float(r[4]), y=float(r[5]),
                     z=float(r[6]), name=None, source="mine", row=r) for r in self.editor.placements]

    def creature(self, row):
        """Placed things the server will treat as creatures."""
        if self.bestiary.is_creature(row["entity"]):
            return True
        stats = {**self.rules.stats.get(row["entity"], {}), **self.rules.stats.get(row["id"], {})}
        return "health" in stats and "damage" in stats

    def creatures(self):
        mine = self.mine()
        ids = {row["id"] for row in mine}
        rows = mine + [row for row in self.reference if row["id"] not in ids]
        return [row for row in rows if self.creature(row)]

    def find(self, identity):
        return next((row for row in self.creatures() if row["id"] == identity), None)

    def label(self, row):
        return row["name"] or self.bestiary.name(row["entity"])

    def scene_rows(self, cell, enabled=True):
        """Base-world rows for the 3D scene around `cell` (its zone and outdoor neighbours).
        Runs on the DAT thread: reads only plain data, never Tk variables."""
        if not enabled:
            return []
        return [row["raw"] for row in self.reference if row["raw"] and self.frame(row, cell) is not None]

    def click_3d(self, view, event):
        """3D click while the creature tools are active; True when consumed."""
        if self.patrol_capture is not None:
            hit = view.surface_hit(event, {"world"})
            if hit:
                self.patrol_capture(hit[3][0], hit[3][1], self.editor.active_scene_cell)
            return True
        if self.editor.library.select() != str(self.editor.page_creatures):
            return False
        hit = view.surface_hit(event, {"world", "saved", "base", "preview"})
        identity = hit and hit[2].get("object_id")
        if identity is None:
            return False
        row = self.find(f"{identity:016x}")
        if row is None:
            return False
        self.select(row["id"], inspect=bool(event.state & 4) or self.inspector is not None)
        return True

    def reference_cell(self):
        editor = self.editor
        if editor.active_scene_cell is not None:
            return editor.active_scene_cell
        try:
            return int(editor.fields["Cell"].get(), 16)
        except ValueError:
            return None

    def block_side(self):
        grid = self.editor.outdoor_grid
        return grid.get("block_side", 160) if grid else 160

    def frame(self, row, reference):
        """Row position in the reference block frame (the 3D view's frame)."""
        cell = row["cell"]
        if outdoor(reference) and outdoor(cell):
            dx, dy = (cell >> 24) - (reference >> 24), ((cell >> 16) & 255) - ((reference >> 16) & 255)
            if abs(dx) > 1 or abs(dy) > 1:
                return None
            side = self.block_side()
            return row["x"] + dx * side, row["y"] + dy * side, row["z"]
        if not outdoor(reference) and not outdoor(cell) and cell >> 16 == reference >> 16:
            return row["x"], row["y"], row["z"]
        return None

    def save(self, message):
        path = self.editor.loaded_path
        if path is None:
            raise ValueError("Load a content file first")
        self.rules_text = save_rules(path, self.rules_text, self.rules)
        self.editor.live.changed()
        self.editor.status.set(message + " Saved to " + path.with_name(path.stem + ".rules.tsv").name
                               + ". Restart the server with --content-overlay to apply.")
        self.refresh()

    # ---- list panel -------------------------------------------------------
    def build_panel(self, page):
        ttk.Label(page, text="Creatures in this zone", style="Section.TLabel").pack(anchor="w", pady=5)
        ttk.Entry(page, textvariable=self.search).pack(fill="x", pady=4)
        self.search.trace_add("write", lambda *unused: self.refresh_list())
        ttk.Checkbutton(page, text="Current zone only", variable=self.zone_only, command=self.refresh_list).pack(anchor="w")
        self.count = ttk.Label(page, style="Muted.TLabel")
        self.count.pack(anchor="w", pady=3)
        toggles = ttk.Frame(page)
        toggles.pack(side="bottom", fill="x", pady=4)
        for index, (name, variable) in enumerate(self.show.items()):
            ttk.Checkbutton(toggles, text=name, variable=variable, command=self.editor.preview).grid(
                row=index // 3, column=index % 3, sticky="w", padx=2)
        ttk.Button(page, text="Inspect / edit creature…", style="Accent.TButton", command=self.open_inspector).pack(
            side="bottom", fill="x", pady=4, before=toggles)
        ttk.Label(page, text="Click a creature in 3D, on the Position map or in the list; Ctrl-click opens it. "
                  "Base-world creatures can't be moved, but their stats, loot, patrol and group can be set. "
                  "The selected creature always shows all its rings.", style="Muted.TLabel",
                  wraplength=280).pack(side="bottom", anchor="w", pady=4, before=toggles)
        frame = ttk.Frame(page)
        frame.pack(fill="both", expand=True)
        frame.columnconfigure(0, weight=1)
        frame.rowconfigure(0, weight=1)
        columns = ("Name", "Lvl", "HP", "Group", "From")
        self.tree = ttk.Treeview(frame, columns=columns, show="headings", height=14, selectmode="browse")
        scroll = ttk.Scrollbar(frame, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=scroll.set)
        self.tree.grid(row=0, column=0, sticky="nsew")
        scroll.grid(row=0, column=1, sticky="ns")
        for column, width in zip(columns, (150, 36, 56, 44, 46)):
            self.tree.heading(column, text=column)
            self.tree.column(column, width=width, minwidth=30, stretch=column == "Name")
        self.tree.bind("<<TreeviewSelect>>", self.list_selected)
        self.tree.bind("<Double-1>", lambda event: self.open_inspector())
        self.tree.bind("<Return>", lambda event: self.open_inspector())

    def refresh(self):
        self.refresh_list()
        self.editor.preview()
        if self.inspector is not None:
            self.inspector.refresh_group()

    def refresh_list(self):
        if not hasattr(self, "tree"):
            return
        query = self.search.get().strip().casefold()
        reference = self.reference_cell()
        rows = []
        for row in self.creatures():
            if self.zone_only.get() and (reference is None or self.frame(row, reference) is None):
                continue
            label = self.label(row)
            if query and query not in f"{label} {row['entity']} {row['id']}".casefold():
                continue
            rows.append((label, row))
        rows.sort(key=lambda item: (item[1]["source"] != "mine", item[0].casefold(), item[1]["id"]))
        self.tree.delete(*self.tree.get_children())
        for label, row in rows[:1000]:
            stats, unused = self.bestiary.effective(self.rules, row["entity"], row["id"])
            group = self.rules.group_of(row["id"])
            self.tree.insert("", "end", iid=row["id"], values=(
                label, stats.get("level", ("-",))[0], stats.get("health", ("-",))[0],
                f"{group['id']:x}" if group else "", "mine" if row["source"] == "mine" else "base"))
        if self.selected and self.tree.exists(self.selected):
            self.tree.selection_set(self.selected)
        note = f"{len(rows):,} creatures" + (" · showing 1,000" if len(rows) > 1000 else "")
        if self.reference_error:
            note += " · base world failed to load"
        elif not self.reference:
            note += " · set a Base world in Project / DATs to see the existing population"
        self.count.configure(text=note)

    def list_selected(self, event):
        selection = self.tree.selection()
        if selection and selection[0] != self.selected:
            self.select(selection[0])

    def select(self, identity, inspect=False):
        self.selected = identity
        if self.tree.exists(identity):
            self.tree.selection_set(identity)
            self.tree.see(identity)
        if self.inspector is not None and self.inspector.row["id"] != identity:
            self.inspector.close()
        if inspect:
            self.open_inspector()
        # Outline the creature in 3D (base-world creatures have no placement selection).
        if not self.editor.group_ids and self.editor.selected_id is None:
            self.editor.view3d.set_selection([int(identity, 16)])
        self.editor.preview()

    def open_inspector(self):
        row = self.find(self.selected) if self.selected else None
        if row is None:
            self.editor.status.set("Select a creature in the list or click one on the Position map.")
            return
        if self.inspector is not None:
            if self.inspector.row["id"] == row["id"]:
                self.inspector.window.lift()
                return
            self.inspector.close()
        self.inspector = CreatureInspector(self, row)

    # ---- overlays ---------------------------------------------------------
    def shapes(self, reference):
        """Overlay primitives in the reference frame: (kind, color, points, closed, id)."""
        if reference is None:
            return []
        show = {name: variable.get() for name, variable in self.show.items()}
        result = []
        positions = {}
        for row in self.creatures():
            if row["source"] == "base" and not show["Base world"] and row["id"] != self.selected:
                continue
            point = self.frame(row, reference)
            if point is None:
                continue
            positions[row["id"]] = point
            x, y, z = point
            radii = self.bestiary.roaming(self.rules, row["entity"], row["id"])
            focus = row["id"] == self.selected
            for name, key in (("Leash", "leash"), ("Wander", "wander"), ("Aggro", "aggro")):
                if (show[name] or focus) and radii[key] > 0:
                    result.append(("circle", COLORS[key], [(x, y, z), radii[key]], True, row["id"]))
            patrol = self.rules.patrols.get(row["id"])
            if patrol and (show["Patrols"] or focus):
                route = [(x, y, z)] + [(x+dx, y+dy, z) for dx, dy in patrol[1]] + [(x, y, z)]
                result.append(("path", COLORS["patrol"], route, False, row["id"]))
            result.append(("dot", COLORS["selected"] if focus else COLORS[row["source"]], [(x, y, z)],
                           False, row["id"]))
        if show["Groups"]:
            for group in self.rules.groups:
                points = [positions[m] for m in group["members"] if m in positions]
                if len(points) > 1:
                    center = tuple(sum(p[i] for p in points) / len(points) for i in range(3))
                    for point in points:
                        result.append(("link", COLORS["group"], [center, point], False, None))
        return result

    def draw_map(self, canvas, project, scale, reference):
        labels = self.show["Labels"].get()
        shapes = self.shapes(reference)
        self.map_hits = []
        for kind, color, points, closed, identity in shapes:
            if kind == "circle":
                (x, y, z), radius = points
                sx, sy = project(x, y)
                r = radius * scale
                if r > 2:
                    canvas.create_oval(sx-r, sy-r, sx+r, sy+r, outline=color, dash=(4, 3) if color == COLORS["leash"] else None,
                                       width=2 if identity == self.selected else 1)
            elif kind in ("path", "link"):
                flat = [c for point in points for c in project(point[0], point[1])]
                canvas.create_line(*flat, fill=color, width=2 if kind == "path" else 1,
                                   dash=(2, 3) if kind == "link" else None, arrow="last" if kind == "path" else None)
                if kind == "path":
                    for index, point in enumerate(points[1:-1], 1):
                        sx, sy = project(point[0], point[1])
                        canvas.create_rectangle(sx-3, sy-3, sx+3, sy+3, fill=color, outline="")
                        canvas.create_text(sx+6, sy-6, text=str(index), fill=color, anchor="w", font=("TkDefaultFont", 8))
        for kind, color, points, closed, identity in shapes:
            if kind == "dot":
                x, y, z = points[0]
                sx, sy = project(x, y)
                size = 6 if identity == self.selected else 4
                canvas.create_oval(sx-size, sy-size, sx+size, sy+size, fill=color, outline="white" if identity == self.selected else "")
                self.map_hits.append((sx, sy, identity))
                if labels or identity == self.selected:
                    row = self.find(identity)
                    canvas.create_text(sx+8, sy, anchor="w", text=self.label(row) if row else identity,
                                       fill="#d7e4ef" if identity == self.selected else "#9fb1c7", font=("TkDefaultFont", 8))
        if self.patrol_capture:
            canvas.create_text(12, 30, anchor="nw", fill=COLORS["patrol"],
                               text="Patrol: click to add waypoints · Escape or Finish to stop")

    def map_click(self, event, unproject):
        """Returns True when the click was consumed by creature tools."""
        if self.patrol_capture is not None:
            x, y = unproject(event.x, event.y)
            self.patrol_capture(x, y, self.map_reference)
            return True
        hits = [(math.hypot(sx-event.x, sy-event.y), identity) for sx, sy, identity in getattr(self, "map_hits", ())]
        hits = [hit for hit in hits if hit[0] <= 10]
        creatures_tab = self.editor.library.select() == str(self.editor.page_creatures)
        if hits and (creatures_tab or event.state & 4):
            self.select(min(hits)[1], inspect=bool(event.state & 4) or self.inspector is not None)
            return True
        return False

    def lines_3d(self, reference):
        """Line strips for the 3D view: (rgb, points, closed)."""
        result = []
        for kind, color, points, closed, identity in self.shapes(reference):
            rgb = tuple(int(color[i:i+2], 16) / 255 for i in (1, 3, 5))
            if kind == "circle":
                (x, y, z), radius = points
                steps = max(24, min(96, int(radius * 2)))
                result.append((rgb, [(x+math.cos(a*2*math.pi/steps)*radius, y+math.sin(a*2*math.pi/steps)*radius, z+.25)
                                     for a in range(steps)], True))
            elif kind in ("path", "link"):
                result.append((rgb, [(p[0], p[1], p[2]+.4) for p in points], False))
            elif kind == "dot":
                x, y, z = points[0]
                result.append((rgb, [(x, y, z), (x, y, z+4)], False))
        return result[:4000]


class CreatureInspector:
    def __init__(self, population, row):
        self.population, self.row = population, row
        self.bestiary = population.bestiary
        editor = population.editor
        self.window = tk.Toplevel(editor.root)
        self.window.title(f"Creature · {population.label(row)}")
        self.window.geometry("560x640")
        self.window.configure(background="#151c26")
        self.window.protocol("WM_DELETE_WINDOW", self.close)
        self.window.bind("<Escape>", lambda event: self.finish_patrol())
        self.scope = tk.StringVar(value="placement")
        head = ttk.Frame(self.window, padding=10)
        head.pack(fill="x")
        ttk.Label(head, text=population.label(row), style="Title.TLabel").pack(anchor="w")
        where = "your world file" if row["source"] == "mine" else "the read-only base world"
        ttk.Label(head, style="Muted.TLabel", text=f"Type {row['entity']} · placement {row['id']} · cell {row['cell']:08x} "
                  f"({row['x']:.1f}, {row['y']:.1f}, {row['z']:.1f}) · from {where}", wraplength=520).pack(anchor="w")
        scope = ttk.Frame(head)
        scope.pack(anchor="w", pady=(8, 0))
        ttk.Label(scope, text="Edit rules for").pack(side="left")
        ttk.Radiobutton(scope, text="this spawn only", value="placement", variable=self.scope,
                        command=self.load_scope).pack(side="left", padx=6)
        ttk.Radiobutton(scope, text=f"every {population.label(row)}", value="type", variable=self.scope,
                        command=self.load_scope).pack(side="left")
        actions = ttk.Frame(self.window, padding=8)
        actions.pack(side="bottom", fill="x")
        ttk.Button(actions, text="Apply  ·  Ctrl+S", style="Accent.TButton", command=self.apply).pack(side="right", padx=3)
        ttk.Button(actions, text="Revert", command=self.load_scope).pack(side="right", padx=3)
        ttk.Button(actions, text="Reset scope to native", command=self.reset_scope).pack(side="left", padx=3)
        self.window.bind("<Control-s>", lambda event: self.apply())
        self.tabs = ttk.Notebook(self.window)
        self.tabs.pack(fill="both", expand=True, padx=8)
        self.build_values("Stats", STATS)
        self.build_values("Behaviour", BEHAVIOUR)
        self.build_loot()
        self.build_spawning()
        self.build_patrol()
        self.load_scope()

    @property
    def target(self):
        return self.row["id"] if self.scope.get() == "placement" else self.row["entity"]

    # ---- stats & behaviour --------------------------------------------------
    def build_values(self, title, specs):
        page = ttk.Frame(self.tabs, padding=10)
        self.tabs.add(page, text=title)
        page.columnconfigure(1, weight=1)
        ttk.Label(page, text="Blank inherits the value shown on the right.", style="Muted.TLabel").grid(
            row=0, column=0, columnspan=3, sticky="w", pady=(0, 8))
        table = self.values = getattr(self, "values", {})
        self.hints = getattr(self, "hints", {})
        for index, spec in enumerate(specs, 1):
            name, label, kind = spec[:3]
            ttk.Label(page, text=label).grid(row=index, column=0, sticky="w", pady=3)
            variable = tk.StringVar()
            if kind is bool:
                widget = ttk.Combobox(page, textvariable=variable, values=("", "yes", "no"), state="readonly", width=12)
            else:
                widget = ttk.Entry(page, textvariable=variable, width=14)
            widget.grid(row=index, column=1, sticky="w", padx=8)
            hint = ttk.Label(page, style="Muted.TLabel")
            hint.grid(row=index, column=2, sticky="w")
            table[name], self.hints[name] = (spec, variable), hint

    def load_scope(self):
        rules = self.population.rules
        target = self.target
        own = {**rules.stats.get(target, {}), **rules.behaviour.get(target, {})}
        # The inherited value is what applies without this scope's own rule.
        saved_stats, saved_behaviour = rules.stats.pop(target, None), rules.behaviour.pop(target, None)
        try:
            stats, behaviour = self.bestiary.effective(rules, self.row["entity"],
                                                       self.row["id"] if self.scope.get() == "placement" else None)
        finally:
            if saved_stats:
                rules.stats[target] = saved_stats
            if saved_behaviour:
                rules.behaviour[target] = saved_behaviour
        inherited = {**stats, **behaviour}
        for name, (spec, variable) in self.values.items():
            value = own.get(name, "")
            if spec[2] is bool:
                value = {"1": "yes", "0": "no"}.get(value, "")
            variable.set(value)
            base = inherited.get(name)
            text = "—" if base is None else {"1": "yes", "0": "no"}.get(base[0], base[0]) if spec[2] is bool else base[0]
            self.hints[name].configure(text=f"inherits {text}" + (f" ({base[1]})" if base else ""))
        self.loot_rows = list(rules.loot.get(target, []))
        self.refresh_loot()
        self.refresh_group()
        self.load_patrol()

    def collect(self, specs):
        values = {}
        for spec in specs:
            text = self.values[spec[0]][1].get()
            if spec[2] is bool:
                text = {"yes": "1", "no": "0"}.get(text, "")
            values[spec[0]] = check_value(spec, text)
        return values

    def apply(self):
        rules = self.population.rules
        try:
            target = self.target
            rules.set_stats(target, self.collect(STATS))
            rules.set_behaviour(target, self.collect(BEHAVIOUR))
            rules.set_loot(target, self.loot_rows)
            if self.scope.get() == "placement":
                rules.set_patrol(target, self.patrol_pause.get() or 0, self.waypoints)
            stats = {**rules.stats.get(self.row["entity"], {}), **rules.stats.get(self.row["id"], {})}
            if not self.bestiary.is_creature(self.row["entity"]) and not ("health" in stats and "damage" in stats):
                raise ValueError("This type has no native combat record: give it Health and Damage to make it a creature.")
            self.population.save(f"Saved rules for {self.population.label(self.row)}.")
            self.load_scope()
        except (OSError, ValueError) as error:
            messagebox.showerror("Rules were not saved", str(error), parent=self.window)
            try:
                self.population.rules = type(rules).parse(self.population.rules_text)
            except ValueError:
                pass

    def reset_scope(self):
        scope = "this spawn" if self.scope.get() == "placement" else f"every {self.population.label(self.row)}"
        if not messagebox.askyesno("Reset rules", f"Remove all custom stats, behaviour and loot for {scope}?", parent=self.window):
            return
        for table in (self.population.rules.stats, self.population.rules.behaviour, self.population.rules.loot):
            table.pop(self.target, None)
        if self.scope.get() == "placement":
            self.population.rules.patrols.pop(self.target, None)
        self.population.save("Reset rules to native values.")
        self.load_scope()

    # ---- loot -------------------------------------------------------------
    def build_loot(self):
        page = ttk.Frame(self.tabs, padding=10)
        self.tabs.add(page, text="Loot")
        self.loot_note = ttk.Label(page, style="Muted.TLabel", wraplength=500)
        self.loot_note.pack(anchor="w", pady=(0, 6))
        frame = ttk.Frame(page)
        frame.pack(fill="both", expand=True)
        self.loot_tree = ttk.Treeview(frame, columns=("Chance", "Weight", "Drops"), show="headings", height=9)
        for column, width in (("Chance", 60), ("Weight", 60), ("Drops", 360)):
            self.loot_tree.heading(column, text=column)
            self.loot_tree.column(column, width=width, stretch=column == "Drops")
        self.loot_tree.pack(fill="both", expand=True)
        self.loot_tree.bind("<Double-1>", lambda event: self.edit_outcome())
        buttons = ttk.Frame(page)
        buttons.pack(fill="x", pady=6)
        for text, command in (("Add drop…", self.add_outcome), ("Edit…", self.edit_outcome),
                              ("Remove", self.remove_outcome), ("Add “nothing”", self.add_nothing),
                              ("Copy native loot", self.copy_native)):
            ttk.Button(buttons, text=text, command=command).pack(side="left", padx=2)

    def refresh_loot(self):
        self.loot_tree.delete(*self.loot_tree.get_children())
        total = sum(weight for weight, items in self.loot_rows) or 1
        for index, (weight, items) in enumerate(self.loot_rows):
            drops = ", ".join(self.bestiary.item_name(item) for item in items) or "nothing"
            self.loot_tree.insert("", "end", iid=str(index), values=(f"{100*weight/total:.1f}%", weight, drops))
        native = self.bestiary.native_loot.get(self.row["entity"], [])
        if self.loot_rows:
            self.loot_note.configure(text="One outcome is rolled per kill and replaces this creature's native loot. "
                                     "Apply saves the table.")
        else:
            self.loot_note.configure(text=f"No custom table: the server uses native loot ({len(native)} observed "
                                     "bundles plus the generic treasure pool). Add a drop to replace it.")

    def selected_outcome(self):
        selection = self.loot_tree.selection()
        return int(selection[0]) if selection else None

    def add_outcome(self):
        ItemPicker(self, None)

    def edit_outcome(self):
        index = self.selected_outcome()
        if index is not None:
            ItemPicker(self, index)

    def remove_outcome(self):
        index = self.selected_outcome()
        if index is not None:
            del self.loot_rows[index]
            self.refresh_loot()

    def add_nothing(self):
        self.loot_rows.append((50, []))
        self.refresh_loot()

    def copy_native(self):
        native = self.bestiary.native_loot.get(self.row["entity"], [])
        usable = [(w, [i.lower() for i in items]) for w, items in native if all(i.lower() in self.bestiary.items for i in items)]
        if not usable:
            messagebox.showinfo("No native loot", "No supported observed loot bundles exist for this creature type.", parent=self.window)
            return
        self.loot_rows.extend(usable[:256-len(self.loot_rows)])
        self.refresh_loot()

    # ---- spawning ----------------------------------------------------------
    def build_spawning(self):
        page = ttk.Frame(self.tabs, padding=10)
        self.tabs.add(page, text="Spawning")
        self.group_note = ttk.Label(page, wraplength=500)
        self.group_note.pack(anchor="w", pady=(0, 8))
        form = ttk.Frame(page)
        form.pack(fill="x")
        self.capacity = tk.StringVar()
        self.group_respawn = tk.StringVar()
        ttk.Label(form, text="Alive at once").grid(row=0, column=0, sticky="w")
        ttk.Spinbox(form, textvariable=self.capacity, from_=1, to=64, width=6).grid(row=0, column=1, sticky="w", padx=6)
        ttk.Label(form, text="Respawn (seconds, blank = each spawn's own)").grid(row=1, column=0, sticky="w", pady=4)
        ttk.Entry(form, textvariable=self.group_respawn, width=8).grid(row=1, column=1, sticky="w", padx=6)
        ttk.Button(form, text="Save group settings", command=self.save_group).grid(row=0, column=2, rowspan=2, padx=10)
        self.members = tk.Listbox(page, height=8, background="#111925", foreground="#e5edf7", borderwidth=0,
                                  highlightthickness=0, selectbackground="#245e87")
        self.members.pack(fill="both", expand=True, pady=8)
        buttons = ttk.Frame(page)
        buttons.pack(fill="x")
        ttk.Button(buttons, text="Add spawn point (copy here)", command=self.add_spawn).pack(side="left", padx=2)
        ttk.Button(buttons, text="Create group", command=self.create_group).pack(side="left", padx=2)
        ttk.Button(buttons, text="Remove selected member", command=self.remove_member).pack(side="left", padx=2)
        ttk.Label(page, style="Muted.TLabel", wraplength=500, text=(
            "A spawn group is a generator: each member is a spawn point, and at most "
            "'alive at once' of them are populated. Add spawn point copies this creature "
            "into your world file 3 m east; move it with the placement tools.")).pack(anchor="w", pady=8)

    def refresh_group(self):
        group = self.population.rules.group_of(self.row["id"])
        self.members.delete(0, "end")
        if group is None:
            self.group_note.configure(text="This spawn is not in a group: it respawns on its own timer.")
            self.capacity.set("")
            self.group_respawn.set("")
            return
        self.group_note.configure(text=f"Spawn group {group['id']:x} · {len(group['members'])} spawn points")
        self.capacity.set(str(group["capacity"]))
        self.group_respawn.set("" if group["regeneration_ms"] is None else f"{group['regeneration_ms']/1000:g}")
        for member in group["members"]:
            row = self.population.find(member)
            self.members.insert("end", f"{member}  {self.population.label(row) if row else '(not placed)'}"
                                + ("  ← this" if member == self.row["id"] else ""))

    def create_group(self):
        rules = self.population.rules
        if rules.group_of(self.row["id"]):
            self.editor_status("This spawn already belongs to a group.")
            return
        identity = max([g["id"] for g in rules.groups] + [0]) + 1
        rules.groups.append(dict(id=identity, capacity=1, regeneration_ms=None, members=[self.row["id"]]))
        self.commit_group(f"Created spawn group {identity:x}.")

    def save_group(self):
        group = self.population.rules.group_of(self.row["id"])
        if group is None:
            return
        try:
            capacity = int(self.capacity.get())
            text = self.group_respawn.get().strip()
            seconds = float(text) if text else None
            if seconds is not None and not math.isfinite(seconds):
                raise ValueError()
        except ValueError:
            messagebox.showerror("Group not saved", "Enter whole-number capacity and a respawn in seconds.", parent=self.window)
            return
        group["capacity"] = max(1, min(capacity, len(group["members"])))
        group["regeneration_ms"] = None if seconds is None else int(round(seconds * 1000))
        self.commit_group("Saved spawn group settings.")

    def commit_group(self, message):
        try:
            self.population.rules.check_groups()
            self.population.save(message)
        except (OSError, ValueError) as error:
            messagebox.showerror("Group not saved", str(error), parent=self.window)
            self.population.rules = type(self.population.rules).parse(self.population.rules_text)
        self.refresh_group()

    def remove_member(self):
        group = self.population.rules.group_of(self.row["id"])
        selection = self.members.curselection()
        if group is None or not selection:
            return
        member = group["members"][selection[0]]
        group["members"].remove(member)
        group["capacity"] = max(1, min(group["capacity"], len(group["members"])))
        if not group["members"]:
            self.population.rules.groups.remove(group)
        self.commit_group(f"Removed {member} from the group. The placement itself was kept.")

    def add_spawn(self):
        editor = self.population.editor
        row = self.row
        try:
            editor.require_loaded_path()
            if not editor.game.get().strip():
                raise ValueError("Choose the DAT folder first (Project / DATs).")
            from editor_document import prepare_change
            args = SimpleNamespace(catalog=editor.args.catalog, game_data=Path(editor.game.get()),
                                   output=editor.loaded_path, entity=int(row["entity"], 16), cell=row["cell"],
                                   position=[row["x"] + 3, row["y"], row["z"]], heading=0, scale=None)
            result, identity = prepare_change(args, editor.original)
            if len((self.population.rules.group_of(row["id"]) or {"members": []})["members"]) >= 64:
                raise ValueError("A spawn group holds at most 64 spawn points.")
            # Committing reloads the rules from disk; group the copy afterwards.
            editor.commit_change(result)
            rules = self.population.rules
            group = rules.group_of(row["id"])
            if group is None:
                group = dict(id=max([g["id"] for g in rules.groups] + [0]) + 1, capacity=1,
                             regeneration_ms=None, members=[row["id"]])
                rules.groups.append(group)
            group["members"].append(f"{identity:016x}")
            rules.check_groups()
            self.population.save(f"Added spawn point {identity:016x} to group {group['id']:x}.")
        except (OSError, ValueError) as error:
            messagebox.showerror("Spawn point not added", str(error), parent=self.window)
            self.population.rules = type(self.population.rules).parse(self.population.rules_text)
        self.refresh_group()

    def editor_status(self, text):
        self.population.editor.status.set(text)

    # ---- patrol -----------------------------------------------------------
    def build_patrol(self):
        page = ttk.Frame(self.tabs, padding=10)
        self.tabs.add(page, text="Patrol")
        self.patrol_page = page
        self.patrol_note = ttk.Label(page, wraplength=500, style="Muted.TLabel")
        self.patrol_note.pack(anchor="w", pady=(0, 8))
        form = ttk.Frame(page)
        form.pack(fill="x")
        self.patrol_pause = tk.StringVar(value="2000")
        ttk.Label(form, text="Pause at each waypoint (ms)").pack(side="left")
        ttk.Entry(form, textvariable=self.patrol_pause, width=8).pack(side="left", padx=6)
        self.waypoint_list = tk.Listbox(page, height=10, background="#111925", foreground="#e5edf7", borderwidth=0,
                                        highlightthickness=0, selectbackground="#245e87")
        self.waypoint_list.pack(fill="both", expand=True, pady=8)
        buttons = ttk.Frame(page)
        buttons.pack(fill="x")
        self.capture_button = ttk.Button(buttons, text="Click map to add waypoints", command=self.toggle_capture)
        self.capture_button.pack(side="left", padx=2)
        ttk.Button(buttons, text="Remove selected", command=self.remove_waypoint).pack(side="left", padx=2)
        ttk.Button(buttons, text="Clear route", command=self.clear_waypoints).pack(side="left", padx=2)

    def load_patrol(self):
        pause, points = self.population.rules.patrols.get(self.row["id"], (2000, []))
        self.patrol_pause.set(str(pause))
        self.waypoints = list(points)
        self.refresh_waypoints()

    def refresh_waypoints(self):
        self.waypoint_list.delete(0, "end")
        for index, (dx, dy) in enumerate(self.waypoints, 1):
            self.waypoint_list.insert("end", f"{index:2d}.  {dx:+.1f} m east, {dy:+.1f} m north of spawn")
        placement = self.scope.get() == "placement"
        self.patrol_note.configure(text=(
            "The creature walks the waypoints in order, back to its spawn, then repeats. It chases players "
            "and returns to the route; it only patrols while a player is within 80 m. Apply saves the route."
            if placement else "Patrols belong to one spawn: switch the scope to 'this spawn only'."))
        self.population.editor.preview()

    def toggle_capture(self):
        if self.population.patrol_capture is not None:
            self.finish_patrol()
            return
        if self.scope.get() != "placement":
            self.scope.set("placement")
            self.load_scope()
        def add(x, y, reference=None):
            reference = reference or self.population.map_reference or self.population.reference_cell()
            home = self.population.frame(self.row, reference) if reference is not None else None
            if home is None:
                return
            if len(self.waypoints) >= 64:
                self.editor_status("A patrol has at most 64 waypoints.")
                return
            dx, dy = x - home[0], y - home[1]
            if math.hypot(dx, dy) > 500:
                self.editor_status("Waypoints must be within 500 m of the spawn.")
                return
            self.waypoints.append((round(dx, 2), round(dy, 2)))
            self.refresh_waypoints()
        self.population.patrol_capture = add
        self.capture_button.configure(text="Finish adding waypoints")
        editor = self.population.editor
        if editor.workspace.index("current") not in (1, 2):
            editor.workspace.select(1 if editor.active_scene_cell is not None else 2)
        editor.status.set("Click the ground in 3D, or the Position map, to add patrol waypoints. Escape or Finish adding waypoints stops.")

    def finish_patrol(self):
        self.population.patrol_capture = None
        self.capture_button.configure(text="Click map to add waypoints")
        self.population.editor.preview()

    def remove_waypoint(self):
        selection = self.waypoint_list.curselection()
        if selection:
            del self.waypoints[selection[0]]
            self.refresh_waypoints()

    def clear_waypoints(self):
        self.waypoints = []
        self.refresh_waypoints()

    def close(self):
        self.finish_patrol()
        self.population.inspector = None
        self.window.destroy()


class ItemPicker:
    """Choose a weight and up to 32 supported loot items for one outcome."""

    def __init__(self, inspector, index):
        self.inspector, self.index = inspector, index
        bestiary = inspector.bestiary
        weight, items = inspector.loot_rows[index] if index is not None else (100, [])
        self.window = tk.Toplevel(inspector.window)
        self.window.title("Loot drop")
        self.window.geometry("520x520")
        self.window.configure(background="#151c26")
        self.window.transient(inspector.window)
        self.weight = tk.StringVar(value=str(weight))
        self.query = tk.StringVar()
        top = ttk.Frame(self.window, padding=10)
        top.pack(fill="x")
        ttk.Label(top, text="Weight").pack(side="left")
        ttk.Entry(top, textvariable=self.weight, width=8).pack(side="left", padx=6)
        ttk.Label(top, text="Search").pack(side="left", padx=(16, 0))
        ttk.Entry(top, textvariable=self.query).pack(side="left", fill="x", expand=True, padx=6)
        self.query.trace_add("write", lambda *unused: self.filter())
        ttk.Label(self.window, text="All selected items drop together as one outcome. Ctrl-click selects several.",
                  style="Muted.TLabel").pack(anchor="w", padx=10)
        frame = ttk.Frame(self.window, padding=(10, 4))
        frame.pack(fill="both", expand=True)
        self.tree = ttk.Treeview(frame, columns=("Item", "Definition"), show="headings", selectmode="extended")
        self.tree.heading("Item", text="Item")
        self.tree.heading("Definition", text="Definition")
        self.tree.column("Definition", width=90, stretch=False)
        self.tree.pack(fill="both", expand=True)
        self.tree.bind("<<TreeviewSelect>>", self.selection_changed)
        self.all = sorted(bestiary.items.items(), key=lambda pair: pair[1].casefold())
        self.chosen = set(items)
        self.filter()
        bar = ttk.Frame(self.window, padding=10)
        bar.pack(fill="x")
        ttk.Button(bar, text="OK", style="Accent.TButton", command=self.accept).pack(side="right")
        ttk.Button(bar, text="Cancel", command=self.window.destroy).pack(side="right", padx=6)

    def selection_changed(self, event=None):
        if not getattr(self, "filtering", False):
            visible = set(self.tree.get_children())
            self.chosen = (self.chosen - visible) | set(self.tree.selection())

    def filter(self):
        self.filtering = True
        query = self.query.get().strip().casefold()
        self.tree.delete(*self.tree.get_children())
        for item, name in self.all:
            if query in f"{name} {item}".casefold() or item in self.chosen:
                self.tree.insert("", "end", iid=item, values=(name, item))
        self.tree.selection_set([item for item in self.chosen if self.tree.exists(item)])
        self.tree.update_idletasks()
        self.filtering = False

    def accept(self):
        self.selection_changed()
        items = sorted(self.chosen)
        try:
            weight = int(self.weight.get())
            if not 1 <= weight <= 1_000_000:
                raise ValueError()
        except ValueError:
            messagebox.showerror("Loot drop", "Weight must be a whole number from 1 to 1000000.", parent=self.window)
            return
        if not items or len(items) > 32:
            messagebox.showerror("Loot drop", "Select 1 to 32 items (use Add “nothing” for an empty outcome).", parent=self.window)
            return
        if self.index is None:
            self.inspector.loot_rows.append((weight, items))
        else:
            self.inspector.loot_rows[self.index] = (weight, items)
        self.inspector.refresh_loot()
        self.window.destroy()
