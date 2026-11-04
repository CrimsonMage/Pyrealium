"""The world builder's To place tab: NPCs and creatures that quest rows already name but the
world does not place yet (content/vault-to-place.tsv, written by tools/import_vault_quests.py
--to-place).

Double-click a row to open its cell at the guide's coordinates with the placement tool armed for
its entity. Click a surface to place it, then save as usual. The quests start working as soon as
their NPCs stand in the world. Rerun the importer after placing and more quests ship.
"""
from pathlib import Path
import tkinter as tk
from tkinter import ttk

DEFAULT = Path(__file__).resolve().parent.parent / "content" / "vault-to-place.tsv"


def read_rows(path=DEFAULT):
    try:
        lines = Path(path).read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    if not lines:
        return []
    header = lines[0].split("\t")
    return [dict(zip(header, line.split("\t"))) for line in lines[1:] if line.strip()]


class ToPlace(ttk.Frame):
    def __init__(self, parent, editor, path=DEFAULT):
        super().__init__(parent, padding=4)
        self.editor, self.path = editor, Path(path)
        self.find = tk.StringVar()
        self.find.trace_add("write", lambda *unused: self.fill())
        self.status = tk.StringVar()
        bar = ttk.Frame(self)
        bar.pack(fill="x")
        ttk.Button(bar, text="Reload list", command=self.load).pack(side="left")
        ttk.Label(bar, text="Find").pack(side="left", padx=(10, 4))
        ttk.Entry(bar, textvariable=self.find, width=30).pack(side="left")
        ttk.Label(bar, textvariable=self.status, style="Muted.TLabel").pack(side="left", padx=10)
        columns = (("what", "Kind", 110), ("name", "Name", 180), ("entity", "Entity", 90), ("where", "Guide coordinates", 130),
                   ("quests", "Quests it unlocks", 420))
        self.list = ttk.Treeview(self, columns=[c[0] for c in columns], show="headings", selectmode="browse")
        for key, title, width in columns:
            self.list.heading(key, text=title)
            self.list.column(key, width=width, stretch=key == "quests")
        self.list.pack(fill="both", expand=True, pady=4)
        self.list.bind("<Double-1>", lambda event: self.go())
        ttk.Label(self, text="Double-click: open the spot with the placement tool armed. Rows with no coordinates "
                             "need a spot from you; the quests name the entity, not a position.", style="Muted.TLabel",
                  wraplength=900).pack(anchor="w")
        self.rows = []
        self.load()

    def load(self):
        self.rows = read_rows(self.path)
        self.fill()
        self.status.set(f"{len(self.rows)} to place" if self.rows else f"No {self.path.name} yet: run the importer with --to-place")

    def fill(self):
        self.list.delete(*self.list.get_children())
        needle = self.find.get().strip().lower()
        for i, r in enumerate(self.rows):
            if needle and not any(needle in (r.get(k) or "").lower() for k in ("name", "entity", "quests", "what")):
                continue
            where = f"{r['north']}, {r['east']}" if r.get("north") else "—"
            self.list.insert("", "end", iid=str(i), values=(r.get("what"), r.get("name"), r.get("entity"), where, r.get("quests")))

    def go(self):
        chosen = self.list.selection()
        if not chosen:
            return
        r = self.rows[int(chosen[0])]
        editor = self.editor
        entity = r["entity"].lower().removeprefix("0x")  # the catalog's form: 8 hex digits
        editor.fields["Entity"].set(entity)
        editor.recent_entities = [entity] + [e for e in editor.recent_entities if e != entity][:23]
        if r.get("cell"):
            editor.navigate_to(int(r["cell"], 16), target_xy=(float(r["x"]), float(r["y"])))
            editor.has_travelled = True
            editor.view3d.set_tool("place")
            editor.workspace.select(1)
            editor.status.set(f"Opening the guide's spot for {r['name']}: click a surface to place {r['entity']}, then save.")
        else:
            editor.status.set(f"{r['name']} ({r['entity']}) is in the Entity field: travel to its spot, then place it.")
