"""World map from an AC2Hero map export (`ac2hero-map --export DIR`).

The export holds the terrain overview (one pixel per 20 m land cell, north up),
a 64-pixel tile per land block for close zoom, and the game's map notes. This
widget draws them with the map notes, portals and their destinations, creatures
and the camera marker over them. Without an export it falls back to the DAT's
painted world map (WorldAtlas).

World pixel space: gx east from the west edge, gy south from the north edge.
A block-local point (cell, x, y) is at gx = bx*8 + x/20, gy = 2040 - (by*8 + y/20).
"""
from collections import OrderedDict
import json
import math
import re
from pathlib import Path
import tkinter as tk
from tkinter import ttk

from editor_travel import WorldAtlas
from place_content import bounded_text

FORMAT = "ac2hero-map-export-v1"
LEVELS = (0.25, 0.5, 1, 2, 4, 8, 16, 32)  # screen pixels per land cell
TILE_FROM = 8
PORTAL_ENTITIES = {"4700084d"}
# Screen pixels per cell from which a kind's icon / name shows (ac2hero-map notes.rs).
SHOWN = {"Towns": (0, 0), "Outposts": (.9, 2), "Gateways": (.9, 2), "Dungeons": (.9, 2), "Vaults": (.9, 2),
         "Ringways": (1.8, 3.5), "Points of interest": (1.8, 3.5), "Other places": (1.8, 3.5)}


def default_export():
    path = Path.home() / ".local" / "share" / "pyrealium" / "hero-map"
    return str(path) if (path / "manifest.json").exists() else ""


def outdoor(cell):
    return 1 <= (cell & 65535) <= 64


def world_point(cell, x, y, cells=2040):
    """Block-local outdoor position -> world pixels (gx, gy)."""
    return (cell >> 24) * 8 + x / 20, cells - (((cell >> 16) & 255) * 8 + y / 20)


def cell_at(gx, gy, cells=2040):
    """World pixels -> (cell, block-local x, y); raises outside the world."""
    north = cells - gy
    if not 0 <= gx < cells or not 0 <= north < cells:
        raise ValueError("Choose a point inside the world map")
    bx, by = int(gx // 8), int(north // 8)
    x, y = (gx - bx * 8) * 20, (north - by * 8) * 20
    return (bx << 24) | (by << 16) | (1 + min(7, int(x // 20)) * 8 + min(7, int(y // 20))), x, y


COORDINATE = re.compile(r"(\d+(?:\.\d+)?)\s*([NS])[\s,]*(\d+(?:\.\d+)?)\s*([EW])", re.IGNORECASE)
HEX = re.compile(r"(?:0x)?([0-9a-f]{4}|[0-9a-f]{8})", re.IGNORECASE)


def parse_destination(text, cells=2040):
    """A typed destination -> (cell, block-local x, y), or None when it is not one.

    Map coordinates as players write them ("31.2N, 93.1E", "8.6s 52.6w"), a landblock
    ("CF8C" or "0xCF8C": its middle) or a cell ("CF8C0005": its middle; an interior cell
    opens as a room). Raises ValueError for a coordinate or block outside the world.
    """
    text = text.strip()
    match = COORDINATE.fullmatch(text)
    if match:
        north = float(match.group(1)) * (1 if match.group(2).upper() == "N" else -1)
        east = float(match.group(3)) * (1 if match.group(4).upper() == "E" else -1)
        return cell_at((east + 100) * 10.2, cells - (north + 100) * 10.2, cells)
    match = HEX.fullmatch(text)
    if not match or not (any(c.isdigit() for c in match.group(1)) or text.lower().startswith("0x")):
        return None
    value = int(match.group(1), 16)
    if len(match.group(1)) == 4:
        bx, by = value >> 8, value & 255
        if bx >= cells // 8 or by >= cells // 8:
            raise ValueError(f"Landblock {value:04X} is outside the world")
        return (bx << 24) | (by << 16) | (1 + 4 * 8 + 4), 80.0, 80.0
    if not outdoor(value):
        return value, None, None
    index = (value & 65535) - 1
    return value, (index // 8) * 20 + 10.0, (index % 8) * 20 + 10.0


def snap_tile_scale(scale, block_side, tile=64):
    """Screen pixels per metre snapped so a block tile scales by a whole zoom/subsample pair.

    Returns (scale, (zoom, subsample)): Tk scales images only by integer factors, so the map
    takes the nearest ratio and the tiles then meet the block grid exactly.
    """
    wanted = max(1e-6, scale * block_side / tile)
    best = min(((up, down) for down in range(1, 9) for up in range(1, 17)),
               key=lambda pair: (abs(math.log(pair[0] / pair[1] / wanted)), pair[1]))
    return tile * best[0] / best[1] / block_side, best


def map_units(gx, gy, cells=2040):
    north, east = (cells - gy) / 10.2 - 100, gx / 10.2 - 100
    return f"{abs(north):.1f}{'N' if north >= 0 else 'S'}, {abs(east):.1f}{'E' if east >= 0 else 'W'}"


def read_routes(path):
    """pyrealium-portal-routes-v1 rows: source portal -> destination."""
    routes = {}
    if not Path(path).exists():
        return routes
    for line in bounded_text(Path(path)).splitlines()[1:]:
        f = line.split("\t")
        if line.startswith("#") or len(f) < 11:
            continue
        try:
            routes[f[0].lower()] = dict(id=f[0].lower(), entity=f[1].lower(), cell=int(f[2], 16),
                                        x=float(f[3]), y=float(f[4]), z=float(f[5]),
                                        destination=(int(f[6], 16), [float(f[7]), float(f[8]), float(f[9])]))
        except ValueError:
            continue
    return routes


def load_export(folder):
    """Plain data (no Tk objects), safe to read on a worker thread."""
    folder = Path(folder)
    manifest = json.loads(bounded_text(folder / "manifest.json"))
    if manifest.get("format") != FORMAT or manifest.get("world_cells") != 2040:
        raise ValueError("Not an ac2hero-map export (run ac2hero-map --export DIR)")
    notes = json.loads((folder / "notes.json").read_text(encoding="utf-8"))
    for note in notes:
        note["gx"] = (note["east"] + 100) * 10.2
        note["gy"] = 2040 - (note["north"] + 100) * 10.2
    blocks = {path.stem for path in (folder / "blocks").glob("*.png")}
    return dict(folder=folder, manifest=manifest, notes=notes, blocks=blocks)


class HeroMap(WorldAtlas):
    """Drop-in for WorldAtlas: same travel callback, marker and note."""

    def __init__(self, parent, on_travel, compact=False, editor=None):
        self.editor = editor
        self.export = None
        self.world_image = None
        self.icons = {}
        self.tiles = OrderedDict()
        self.pending_tiles = []
        self.tile_timer = None
        self.level = 2 if compact else 3
        self.center = [1020.0, 1020.0]
        self.hits = []
        self.selected_portal = None
        self.layers = {name: tk.BooleanVar(value=value) for name, value in (
            ("Places", True), ("Portals", True), ("Portal links", not compact), ("Creatures", not compact))}
        super().__init__(parent, on_travel, compact)
        self.canvas.bind("<Motion>", self.hover)
        self.canvas.bind("<Button-4>", lambda event: self.zoom_at(event, 1))
        self.canvas.bind("<Button-5>", lambda event: self.zoom_at(event, -1))
        self.canvas.bind("<MouseWheel>", lambda event: self.zoom_at(event, 1 if event.delta > 0 else -1))
        self.canvas.bind("<ButtonPress-3>", lambda event: setattr(self, "drag", (event.x, event.y, *self.center)))
        bar = ttk.Frame(self)
        bar.pack(fill="x", padx=8, before=self.canvas)
        self.query = tk.StringVar()
        if not compact:
            entry = ttk.Entry(bar, textvariable=self.query, width=28)
            entry.pack(side="left", pady=4)
            entry.bind("<Return>", lambda event: self.find())
            ttk.Button(bar, text="Go / find", command=self.find).pack(side="left", padx=4)
            ttk.Label(bar, text="place, portal, 31.2N 93.1E, landblock CF8C or cell CF8C0005", style="Muted.TLabel").pack(side="left", padx=(0, 8))
        for name, variable in self.layers.items():
            ttk.Checkbutton(bar, text=name, variable=variable, command=self.draw).pack(side="left", padx=3)

    def destroy(self):
        if self.tile_timer is not None:
            self.after_cancel(self.tile_timer)
            self.tile_timer = None
        super().destroy()

    # ---- data -------------------------------------------------------------
    def set_export(self, data):
        self.export = data
        self.world_image = tk.PhotoImage(file=str(data["folder"] / "world.png"))
        self.icons.clear()
        self.tiles.clear()
        for note in data["notes"]:
            icon = data["folder"] / "icons" / f"{note['icon']}.png"
            if note["icon"] not in self.icons and icon.exists():
                self.icons[note["icon"]] = tk.PhotoImage(file=str(icon)).zoom(2)
        self.note.set(f"Your AC2Hero map: {len(data['notes'])} places, {len(data['blocks']):,} land blocks. "
                      "Click to travel · click a portal for its destination · wheel zooms · right-drag pans.")
        self.draw()

    def set_atlas(self, data):
        # The DAT atlas still supplies block geometry, and is the fallback picture.
        super().set_atlas(data)
        if self.export is not None:
            self.draw()

    def portals(self):
        """Every known portal: routed ones plus placed portals without a route."""
        editor = self.editor
        routes = getattr(editor, "portal_routes", {}) if editor else {}
        result = dict(routes)
        rows = []
        if editor is not None:
            population = editor.population
            rows = population.mine() + population.reference
        for row in rows:
            if row["id"] in result:
                continue
            if row["entity"] in PORTAL_ENTITIES or "portal" in (row.get("name") or "").casefold():
                result[row["id"]] = dict(row, destination=None)
        return [portal for portal in result.values() if outdoor(portal["cell"])]

    def place_name(self, cell, point=None):
        """A destination's name: a supplied area name, else the nearest map note."""
        editor = self.editor
        for row in getattr(editor, "locations", ()) if editor else ():
            if row["cell"] == cell:
                return row["name"]
        if not outdoor(cell):
            same = next((row["name"] for row in getattr(editor, "locations", ()) if row["cell"] >> 16 == cell >> 16), None) if editor else None
            return same or f"dungeon {cell >> 16:04X}"
        if self.export and point is not None:
            gx, gy = world_point(cell, point[0], point[1])
            near = min(self.export["notes"], key=lambda n: (n["gx"]-gx)**2 + (n["gy"]-gy)**2, default=None)
            if near and math.hypot(near["gx"]-gx, near["gy"]-gy) <= 24:
                return near["name"]
            return map_units(gx, gy)
        return f"{cell:08X}"

    # ---- view ---------------------------------------------------------------
    @property
    def scale(self):
        return LEVELS[self.level]

    def fit(self):
        if self.export is None:
            return super().fit()
        self.level = 2 if self.compact else 1
        self.center = [1020.0, 1020.0]
        self.draw()

    def to_screen(self, gx, gy):
        width, height = self.canvas.winfo_width(), self.canvas.winfo_height()
        return width / 2 + (gx - self.center[0]) * self.scale, height / 2 + (gy - self.center[1]) * self.scale

    def to_world(self, sx, sy):
        width, height = self.canvas.winfo_width(), self.canvas.winfo_height()
        return self.center[0] + (sx - width / 2) / self.scale, self.center[1] + (sy - height / 2) / self.scale

    def zoom_at(self, event, steps):
        if self.export is None:
            return self.zoom_map(type("Wheel", (), {"delta": steps})())
        before = self.to_world(event.x, event.y)
        self.level = max(0, min(len(LEVELS) - 1, self.level + steps))
        after = self.to_world(event.x, event.y)
        self.center = [self.center[0] + before[0] - after[0], self.center[1] + before[1] - after[1]]
        self.draw()

    def pan_map(self, event):
        if self.export is None:
            return super().pan_map(event)
        if getattr(self, "drag", None):
            x, y, cx, cy = self.drag
            self.center = [cx - (event.x - x) / self.scale, cy - (event.y - y) / self.scale]
            self.draw()

    def show(self, gx, gy, level=None):
        if level is not None:
            self.level = level
        self.center = [gx, gy]
        self.draw()

    # ---- drawing ------------------------------------------------------------
    def draw(self):
        if self.export is None:
            return super().draw()
        canvas = self.canvas
        canvas.delete("all")
        self.hits = []
        width, height = canvas.winfo_width(), canvas.winfo_height()
        if width < 20 or height < 20:
            return
        scale = self.scale
        x1, y1 = self.to_world(0, 0)
        x2, y2 = self.to_world(width, height)
        self.draw_overview(x1, y1, x2, y2)
        if scale >= TILE_FROM:
            self.draw_tiles(x1, y1, x2, y2)
        if self.marker:
            cell, point = self.marker
            if outdoor(cell):
                sx, sy = self.to_screen(*world_point(cell, point[0], point[1]))
                canvas.create_oval(sx-6, sy-6, sx+6, sy+6, fill="#ffd479", outline="#142233", width=2)
                canvas.create_text(sx+10, sy-10, text="Camera", anchor="w", fill="white")
        if self.layers["Creatures"].get() and scale >= 2 and self.editor is not None:
            self.draw_creatures(x1, y1, x2, y2)
        if self.layers["Places"].get():
            self.draw_notes(x1, y1, x2, y2)
        if self.layers["Portals"].get():
            self.draw_portals(x1, y1, x2, y2)
        canvas.create_text(10, height-10, anchor="sw", fill="#c6d8eb",
                           text=f"{scale:g} px per cell · {map_units(*self.center)}")

    def draw_overview(self, x1, y1, x2, y2):
        scale = self.scale
        sx1, sy1 = max(0, int(x1)), max(0, int(y1))
        sx2, sy2 = min(2040, int(math.ceil(x2))), min(2040, int(math.ceil(y2)))
        if sx2 <= sx1 or sy2 <= sy1:
            return
        zoom, subsample = (int(scale), 1) if scale >= 1 else (1, int(round(1 / scale)))
        # Tk crops, subsamples and zooms the visible region in C: a screen-sized image at
        # any level, and the backdrop under block tiles that are still loading.
        image = tk.PhotoImage(master=self.canvas)
        image.tk.call(image, "copy", self.world_image, "-from", sx1, sy1, sx2, sy2,
                      "-zoom", zoom, zoom, "-subsample", subsample, subsample)
        self.shown = image  # keep a reference while it is on the canvas
        self.canvas.create_image(*self.to_screen(sx1, sy1), image=image, anchor="nw")

    def draw_tiles(self, x1, y1, x2, y2):
        scale = self.scale
        wanted = []
        for bx in range(max(0, int(x1 // 8)), min(255, int(x2 // 8) + 1)):
            for north in range(max(0, int((2040 - y2) // 8)), min(255, int((2040 - y1) // 8) + 1)):
                name = f"{bx:02x}{north:02x}"
                if name not in self.export["blocks"]:
                    continue
                key = (name, scale)
                sx, sy = self.to_screen(bx * 8, 2040 - (north + 1) * 8)
                if key in self.tiles:
                    self.tiles.move_to_end(key)
                    self.canvas.create_image(sx, sy, image=self.tiles[key], anchor="nw")
                else:
                    wanted.append(key)
        self.pending_tiles = wanted
        if wanted and self.tile_timer is None:
            self.tile_timer = self.after(1, self.load_tiles)

    def load_tiles(self):
        """Load a few tiles per Tk turn so panning stays responsive."""
        self.tile_timer = None
        loaded = 0
        while self.pending_tiles and loaded < 24:
            name, scale = key = self.pending_tiles.pop()
            if key in self.tiles or scale != self.scale:
                continue
            image = tk.PhotoImage(master=self.canvas, file=str(self.export["folder"] / "blocks" / f"{name}.png"))
            factor = scale / 8  # tiles hold 8 pixels per cell
            self.tiles[key] = image.zoom(int(factor)) if factor > 1 else image
            loaded += 1
            while len(self.tiles) > 800:
                self.tiles.popitem(last=False)
        if loaded:
            self.draw()

    def visible(self, gx, gy, x1, y1, x2, y2, margin=4):
        return x1 - margin <= gx <= x2 + margin and y1 - margin <= gy <= y2 + margin

    def draw_notes(self, x1, y1, x2, y2):
        scale = self.scale
        for note in self.export["notes"]:
            shown, named = SHOWN.get(note["kind"], (1.8, 3.5))
            if scale < shown or not self.visible(note["gx"], note["gy"], x1, y1, x2, y2):
                continue
            sx, sy = self.to_screen(note["gx"], note["gy"])
            icon = self.icons.get(note["icon"])
            if icon is not None:
                self.canvas.create_image(sx, sy, image=icon)
            else:
                self.canvas.create_oval(sx-4, sy-4, sx+4, sy+4, fill="#e8d9a8", outline="")
            if scale >= named:
                self.canvas.create_text(sx+12, sy, text=note["name"], anchor="w", fill="#f2ead2",
                                        font=("TkDefaultFont", 9, "bold" if note["kind"] == "Towns" else "normal"))
            self.hits.append((sx, sy, "note", note))

    def draw_portals(self, x1, y1, x2, y2):
        scale = self.scale
        links = self.layers["Portal links"].get()
        for portal in self.portals():
            gx, gy = world_point(portal["cell"], portal["x"], portal["y"])
            destination = portal.get("destination")
            target = None
            if destination and outdoor(destination[0]):
                target = world_point(destination[0], *destination[1][:2])
            on_screen = self.visible(gx, gy, x1, y1, x2, y2)
            if not on_screen and not (target and self.selected_portal == portal["id"]):
                continue
            sx, sy = self.to_screen(gx, gy)
            focus = self.selected_portal == portal["id"]
            if target and (links and scale >= 1 or focus):
                tx, ty = self.to_screen(*target)
                self.canvas.create_line(sx, sy, tx, ty, fill="#c58cff" if focus else "#8a63b8",
                                        width=2 if focus else 1, dash=(5, 3), arrow="last")
            if not on_screen:
                continue
            color = "#c58cff" if destination else "#8a8f99"
            r = 7 if focus else 5
            self.canvas.create_polygon(sx, sy-r, sx+r, sy, sx, sy+r, sx-r, sy, fill=color, outline="white" if focus else "#2a1d3a")
            if scale >= 16 or focus:
                label = ("→ " + self.place_name(*destination)) if destination else "portal (destination unknown)"
                self.canvas.create_text(sx+10, sy+11, text=label, anchor="w", fill="#e3ccff", font=("TkDefaultFont", 8))
            self.hits.append((sx, sy, "portal", portal))

    def draw_creatures(self, x1, y1, x2, y2):
        population = self.editor.population
        for row in population.creatures():
            if not outdoor(row["cell"]):
                continue
            gx, gy = world_point(row["cell"], row["x"], row["y"])
            if not self.visible(gx, gy, x1, y1, x2, y2):
                continue
            sx, sy = self.to_screen(gx, gy)
            mine = row["source"] == "mine"
            size = 3 if mine else 2
            self.canvas.create_oval(sx-size, sy-size, sx+size, sy+size, outline="",
                                    fill="#efb65e" if row["id"] == population.selected else "#72b9e5" if mine else "#d0675f")
            if self.scale >= 16:
                self.canvas.create_text(sx+6, sy, text=population.label(row), anchor="w", fill="#e6b9b4", font=("TkDefaultFont", 7))
            self.hits.append((sx, sy, "creature", row))

    # ---- interaction --------------------------------------------------------
    def hit(self, event):
        best = min(((math.hypot(sx-event.x, sy-event.y), kind, item) for sx, sy, kind, item in self.hits),
                   key=lambda h: h[0], default=None)
        return best if best and best[0] <= 9 else None

    def hover(self, event):
        if self.export is None:
            return
        gx, gy = self.to_world(event.x, event.y)
        text = map_units(gx, gy)
        hit = self.hit(event)
        if hit:
            kind, item = hit[1], hit[2]
            if kind == "note":
                text += f" · {item['name']}"
            elif kind == "portal":
                destination = item.get("destination")
                text += " · portal → " + (self.place_name(*destination) + f" ({destination[0]:08X})" if destination else "unknown destination")
            else:
                text += f" · {self.editor.population.label(item)}"
        self.note.set(text)

    def travel(self, event):
        if self.export is None:
            return super().travel(event)
        hit = self.hit(event)
        if hit and hit[1] == "portal":
            return self.portal_menu(event, hit[2])
        if hit and hit[1] == "creature" and self.editor is not None:
            row = hit[2]
            self.editor.show_creatures()
            self.editor.population.select(row["id"], inspect=True)
            self.on_travel(row["cell"], [row["x"], row["y"], row["z"]])
            return
        try:
            cell, x, y = cell_at(*self.to_world(event.x, event.y))
        except ValueError as error:
            self.note.set(str(error))
            return
        self.note.set("Opening the selected region in 3D…")
        self.on_travel(cell, target_xy=(x, y))

    def portal_menu(self, event, portal):
        self.selected_portal = portal["id"]
        self.draw()
        menu = tk.Menu(self, tearoff=False)
        menu.add_command(label=f"Go to this portal ({portal['cell']:08X})",
                         command=lambda: self.on_travel(portal["cell"], [portal["x"], portal["y"], portal["z"]]))
        destination = portal.get("destination")
        if destination:
            menu.add_command(label="Go to destination: " + self.place_name(*destination),
                             command=lambda: self.on_travel(destination[0], list(destination[1])))
            if outdoor(destination[0]):
                menu.add_command(label="Show destination on map",
                                 command=lambda: self.show(*world_point(destination[0], *destination[1][:2]), level=max(self.level, 3)))
        else:
            menu.add_command(label="Destination unknown", state="disabled")
        menu.tk_popup(event.x_root, event.y_root)

    def go_to(self, text):
        """Teleport the 3D camera to a typed coordinate, landblock or cell; False if it is none."""
        try:
            destination = parse_destination(text)
        except ValueError as error:
            self.note.set(str(error))
            return True
        if destination is None:
            return False
        cell, x, y = destination
        if x is None:
            self.note.set(f"Opening room {cell:08X} in 3D…")
            self.on_travel(cell)
            return True
        gx, gy = world_point(cell, x, y)
        if self.export is not None:
            self.show(gx, gy, level=max(self.level, 4))
        self.note.set(f"{map_units(gx, gy)} · landblock {cell >> 16:04X} · cell {cell:08X} · opening in 3D…")
        self.on_travel(cell, target_xy=(x, y))
        return True

    def find(self):
        query = self.query.get().strip().casefold()
        if not query or self.go_to(query):
            return
        if self.export is None:
            return
        for note in self.export["notes"]:
            if query in note["name"].casefold():
                self.show(note["gx"], note["gy"], level=max(self.level, 4))
                self.note.set(f"{note['name']} · {map_units(note['gx'], note['gy'])}")
                return
        for portal in self.portals():
            destination = portal.get("destination")
            if destination and query in self.place_name(*destination).casefold():
                self.selected_portal = portal["id"]
                self.show(*world_point(portal["cell"], portal["x"], portal["y"]), level=max(self.level, 4))
                return
        self.note.set(f"No place or portal destination matches “{self.query.get().strip()}”.")
