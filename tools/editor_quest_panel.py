"""The world builder's Quests tab: browse and edit the quest database's sets and actions.

It edits NAME.quests.sqlite (`editor_quests.build_database`), the same database **Edit quests**
opens in an SQLite tool. **Save** commits it; the live link then pushes it, and the server checks
every row. The tab is a form over the rows, not a second source of rules. The schema refuses
the plainest mistakes as they are typed, and the server's check (or `pyrealium-content check`)
refuses the rest. What each action takes comes from docs/quest-actions.md, which the engine
writes.

Speech is data. `Tell table key` speaks a DAT string-table line by reference, and **Pick DAT
line** lists the NPC's own lines from the live server (`GET /admin/strings`). `Say` and
`Message` hold your own text.
"""
from pathlib import Path
import re
import sqlite3
import tkinter as tk
from tkinter import messagebox, ttk

import editor_quests

TRIGGERS = ["Use", "Give", "Kill", "ItemUsedOn", "Enter", "Crafted", "EscortLost", "QuestSuccess", "QuestFailure",
            "Goto", "PlayerDeath", "QuestExpired", "Logout", "EscortAbandoned", "EnterWorld", "UseItem", "Bind",
            "Travel", "Ask", "Talk", "HoldDone", "HoldLost"]
SET_FIELDS = ["name", "priority", "trigger", "owner_entity", "owner_iid", "owner_slots", "item", "phrase", "quest",
              "stage_min", "stage_max", "range", "probability", "family", "source"]
ACTIONS_DOC = Path(__file__).resolve().parent.parent / "docs" / "quest-actions.md"


def action_specs(path=ACTIONS_DOC):
    """{action: ([parameter help], doc)} from the generated action reference."""
    specs = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return specs
    for line in text.splitlines():
        found = re.match(r"\| `([A-Za-z]+)` \| (.*?) \| (.*) \|$", line)
        if found:
            name, params, doc = found.groups()
            specs[name] = ([p.strip() for p in params.split("<br>") if p.strip()], doc)
    return specs


def blank(value):
    return None if value is None or str(value).strip() == "" else str(value).strip()


class QuestPanel(ttk.Frame):
    def __init__(self, parent, editor):
        super().__init__(parent, padding=4)
        self.editor = editor
        self.db = None
        self.path = None
        self.specs = action_specs()
        self.lines = {}          # string table -> {key: text}, from the live server
        self.current = None      # the selected set's name
        self.filter = tk.StringVar()
        self.filter.trace_add("write", lambda *unused: self.refresh_tree())
        self.vars = {field: tk.StringVar() for field in SET_FIELDS}
        self.status = tk.StringVar(value="Load a content file, then Open quests.")
        self.build()

    # Layout
    def build(self):
        bar = ttk.Frame(self)
        bar.pack(fill="x")
        ttk.Button(bar, text="Open quests", command=self.open).pack(side="left")
        ttk.Button(bar, text="Save", command=self.save).pack(side="left", padx=4)
        ttk.Button(bar, text="Revert", command=self.revert).pack(side="left")
        ttk.Label(bar, text="Find").pack(side="left", padx=(12, 4))
        ttk.Entry(bar, textvariable=self.filter, width=28).pack(side="left")
        ttk.Label(bar, textvariable=self.status, style="Muted.TLabel").pack(side="left", padx=10)
        panes = ttk.PanedWindow(self, orient="horizontal")
        panes.pack(fill="both", expand=True, pady=4)
        left = ttk.Frame(panes)
        self.tree = ttk.Treeview(left, columns=("trigger", "owner", "stages"), show="tree headings", selectmode="browse")
        for column, title, width in (("#0", "Quest / set", 260), ("trigger", "Trigger", 80), ("owner", "Owner", 110), ("stages", "Stages", 60)):
            self.tree.heading(column, text=title)
            self.tree.column(column, width=width, stretch=column == "#0")
        scroll = ttk.Scrollbar(left, command=self.tree.yview)
        self.tree.configure(yscrollcommand=scroll.set)
        self.tree.pack(side="left", fill="both", expand=True)
        scroll.pack(side="left", fill="y")
        self.tree.bind("<<TreeviewSelect>>", lambda event: self.select())
        panes.add(left, weight=1)
        right = ttk.Frame(panes, padding=(6, 0))
        panes.add(right, weight=2)
        form = ttk.LabelFrame(right, text="Set", padding=4)
        form.pack(fill="x")
        for i, field in enumerate(SET_FIELDS):
            row, column = divmod(i, 3)
            ttk.Label(form, text=field).grid(row=row, column=column * 2, sticky="e", padx=(6, 2), pady=1)
            if field == "trigger":
                widget = ttk.Combobox(form, textvariable=self.vars[field], values=TRIGGERS, width=16, state="readonly")
            elif field == "quest":
                widget = self.quest_box = ttk.Combobox(form, textvariable=self.vars[field], width=26)
            else:
                widget = ttk.Entry(form, textvariable=self.vars[field], width=28 if field == "source" else 18)
            widget.grid(row=row, column=column * 2 + 1, sticky="ew", pady=1)
        buttons = ttk.Frame(form)
        buttons.grid(row=len(SET_FIELDS) // 3 + 1, column=0, columnspan=6, sticky="w", pady=(4, 0))
        for text, command in (("Apply", self.apply_set), ("New set", self.new_set), ("Duplicate", self.duplicate_set), ("Delete set", self.delete_set)):
            ttk.Button(buttons, text=text, command=command).pack(side="left", padx=2)
        ttk.Label(form, text="The first matching set by priority (then name) runs. probability is cumulative "
                             "against one roll. phrase: Ask sets only.", style="Muted.TLabel",
                  wraplength=640).grid(row=len(SET_FIELDS) // 3 + 2, column=0, columnspan=6, sticky="w")
        actions = ttk.LabelFrame(right, text="Actions (in order)", padding=4)
        actions.pack(fill="both", expand=True, pady=(6, 0))
        self.actions = ttk.Treeview(actions, columns=("delay", "action", "params", "says"), show="headings", selectmode="browse")
        for column, title, width in (("delay", "Delay ms", 64), ("action", "Action", 130), ("params", "Parameters", 260), ("says", "Says / note", 360)):
            self.actions.heading(column, text=title)
            self.actions.column(column, width=width, stretch=column in ("params", "says"))
        self.actions.pack(fill="both", expand=True)
        self.actions.bind("<Double-1>", lambda event: self.edit_action())
        row = ttk.Frame(actions)
        row.pack(fill="x", pady=(4, 0))
        for text, command in (("Add", self.add_action), ("Edit", self.edit_action), ("Delete", self.delete_action),
                              ("Up", lambda: self.move_action(-1)), ("Down", lambda: self.move_action(1))):
            ttk.Button(row, text=text, command=command).pack(side="left", padx=2)

    # The database
    def open(self):
        path = self.editor.loaded_path
        if path is None:
            self.status.set("Load a content file first.")
            return
        try:
            database = editor_quests.build_database(path)
            if self.db is not None:
                self.db.close()
            self.db = sqlite3.connect(database)
            self.db.execute("PRAGMA foreign_keys = ON")
        except (OSError, ValueError, sqlite3.Error) as error:
            self.status.set(f"Cannot open the quests: {error}")
            return
        self.path = database
        self.quest_box.configure(values=[""] + [r[0] for r in self.db.execute("SELECT name FROM quest ORDER BY name")])
        self.refresh_tree()
        self.status.set(f"{database.name}: {self.count('quest')} quests, {self.count('emote_set')} sets.")

    def count(self, table):
        return self.db.execute(f"SELECT count(*) FROM {table}").fetchone()[0]

    def save(self):
        if self.db is None:
            return
        try:
            broken = self.db.execute("PRAGMA foreign_key_check").fetchall()
            if broken:
                raise sqlite3.IntegrityError(f"{broken[0][0]} row names a missing {broken[0][2]}")
            self.db.commit()
        except sqlite3.Error as error:
            messagebox.showerror("Quests", f"Not saved: {error}")
            return
        live = " The live server checks and runs it." if self.editor.live.enabled.get() else " Turn on Live server to push it."
        self.status.set(f"Saved {self.path.name}.{live}")

    def revert(self):
        if self.db is not None:
            self.db.rollback()
            self.refresh_tree()
            self.status.set("Unsaved quest edits dropped.")

    def write(self, sql, args=()):
        try:
            self.db.execute(sql, args)
            return True
        except sqlite3.Error as error:
            messagebox.showerror("Quests", str(error))
            return False

    # The tree: quests, each with its sets; sets with no quest under "(no quest)"
    def refresh_tree(self):
        if self.db is None:
            return
        self.tree.delete(*self.tree.get_children())
        needle = self.filter.get().strip().lower()
        groups = {}
        for name, trigger, owner, iid, slots, quest, low, high in self.db.execute(
                "SELECT name, trigger, owner_entity, owner_iid, owner_slots, quest, stage_min, stage_max FROM emote_set ORDER BY priority, name"):
            owner = owner or iid or slots or ""
            if needle and not any(needle in str(v).lower() for v in (name, trigger, owner, quest or "")):
                continue
            stages = "" if low is None and high is None else f"{low if low is not None else 0}-{high if high is not None else 255}"
            groups.setdefault(quest or "(no quest)", []).append((name, trigger, owner, stages))
        for quest in sorted(groups):
            node = self.tree.insert("", "end", text=quest, open=bool(needle), values=("", "", ""))
            for name, trigger, owner, stages in groups[quest]:
                self.tree.insert(node, "end", iid="set:" + name, text=name, values=(trigger, owner, stages))
        if self.current and self.tree.exists("set:" + self.current):
            self.tree.see("set:" + self.current)

    def select(self):
        chosen = self.tree.selection()
        if not chosen or not chosen[0].startswith("set:"):
            return
        self.current = chosen[0][4:]
        row = self.db.execute(f"SELECT {', '.join(SET_FIELDS)} FROM emote_set WHERE name = ?", (self.current,)).fetchone()
        for field, value in zip(SET_FIELDS, row):
            self.vars[field].set("" if value is None else str(value))
        self.refresh_actions()

    # Sets
    def set_values(self):
        values = {field: blank(self.vars[field].get()) for field in SET_FIELDS}
        values["family"] = values["family"] or "hand"
        return values

    def apply_set(self):
        if self.db is None or not self.current:
            return
        v = self.set_values()
        if self.write(f"UPDATE emote_set SET {', '.join(f'{f} = ?' for f in SET_FIELDS)} WHERE name = ?",
                      [v[f] for f in SET_FIELDS] + [self.current]):
            self.current = v["name"]
            self.refresh_tree()
            self.status.set(f"Set {self.current} changed (not saved yet).")

    def insert_set(self, values):
        if self.write(f"INSERT INTO emote_set ({', '.join(SET_FIELDS)}) VALUES ({', '.join('?' * len(SET_FIELDS))})",
                      [values[f] for f in SET_FIELDS]):
            self.current = values["name"]
            self.refresh_tree()
            self.tree.selection_set("set:" + self.current)
            self.select()  # the form shows the new set at once, not on the next event pass
            return True
        return False

    def unique(self, base):
        name, n = base, 2
        while self.db.execute("SELECT 1 FROM emote_set WHERE name = ?", (name,)).fetchone():
            name, n = f"{base}_{n}", n + 1
        return name

    def new_set(self):
        if self.db is None:
            return
        top = self.db.execute("SELECT coalesce(max(priority), 0) + 1 FROM emote_set").fetchone()[0]
        values = {f: None for f in SET_FIELDS}
        values.update(name=self.unique("new_set"), priority=top, trigger="Use", family="hand", source="world builder")
        values["owner_entity"] = blank(self.vars["owner_entity"].get())
        self.insert_set(values)

    def duplicate_set(self):
        if self.db is None or not self.current:
            return
        values = self.set_values()
        values["name"] = self.unique(self.current + "_copy")
        old = self.current
        if self.insert_set(values):
            self.write("INSERT INTO emote_action (emote_set, ord, action, delay_ms, p1, p2, p3, p4, p5, family, source) "
                       "SELECT ?, ord, action, delay_ms, p1, p2, p3, p4, p5, family, source FROM emote_action WHERE emote_set = ?",
                       (values["name"], old))
            self.refresh_actions()

    def delete_set(self):
        if self.db is None or not self.current:
            return
        if not messagebox.askyesno("Quests", f"Delete set {self.current} and its actions?"):
            return
        if self.write("DELETE FROM emote_action WHERE emote_set = ?", (self.current,)) and \
                self.write("DELETE FROM emote_set WHERE name = ?", (self.current,)):
            self.current = None
            self.actions.delete(*self.actions.get_children())
            self.refresh_tree()

    # Actions
    def action_rows(self):
        return self.db.execute("SELECT ord, delay_ms, action, p1, p2, p3, p4, p5, source FROM emote_action "
                               "WHERE emote_set = ? ORDER BY ord", (self.current,)).fetchall()

    def says(self, action, p, source):
        if action in ("Say", "Message"):
            return p[0]
        if action == "Tell":
            text = self.line(p[0], p[1])
            return text if text is not None else source
        return ""

    def refresh_actions(self):
        self.actions.delete(*self.actions.get_children())
        for ord_, delay, action, *rest in self.action_rows():
            p, source = rest[:5], rest[5]
            params = " · ".join(v for v in p if v)
            self.actions.insert("", "end", iid=str(ord_), values=(delay or "", action, params, self.says(action, p, source)))

    def renumber(self, rows):
        """Rewrites the set's actions in the given order (ord 1, 2, ...)."""
        self.write("DELETE FROM emote_action WHERE emote_set = ?", (self.current,))
        family = self.vars["family"].get() or "hand"
        for i, (delay, action, p, source) in enumerate(rows, 1):
            self.write("INSERT INTO emote_action (emote_set, ord, action, delay_ms, p1, p2, p3, p4, p5, family, source) "
                       "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", (self.current, i, action, delay, *p, family, source))
        self.refresh_actions()

    def current_rows(self):
        return [(delay or 0, action, list(rest[:5]), rest[5]) for _, delay, action, *rest in self.action_rows()]

    def chosen_index(self):
        chosen = self.actions.selection()
        return None if not chosen else self.actions.index(chosen[0])

    def add_action(self):
        if self.db is None or not self.current:
            return
        ActionDialog(self, None, lambda row: self.renumber(self.current_rows() + [row]))

    def edit_action(self):
        index = self.chosen_index()
        if index is None:
            return
        rows = self.current_rows()

        def done(row):
            rows[index] = row
            self.renumber(rows)
        ActionDialog(self, rows[index], done)

    def delete_action(self):
        index = self.chosen_index()
        if index is not None:
            rows = self.current_rows()
            del rows[index]
            self.renumber(rows)

    def move_action(self, step):
        index = self.chosen_index()
        rows = self.current_rows()
        if index is None or not 0 <= index + step < len(rows):
            return
        rows[index], rows[index + step] = rows[index + step], rows[index]
        self.renumber(rows)
        self.actions.selection_set(str(index + step + 1))

    # DAT lines from the live server
    def fetch_lines(self, query):
        """{key: text} of a string table (`table=0x25...` or `entity=0x47...`), and its table."""
        from editor_live import request
        live = self.editor.live
        code, body = request(live.address.get().strip(), live.token(), "GET", f"/admin/strings?{query}")
        if code != 200:
            raise ValueError(body.get("error", f"HTTP {code}"))
        table = body["table"]
        self.lines[table] = {line["key"]: line["text"] for line in body["lines"]}
        return table, self.lines[table]

    def line(self, table, key):
        table, key = (table or "").lower(), (key or "").lower()
        if table not in self.lines:
            return None
        return self.lines[table].get(key)

    def owner_entity(self):
        """The set's owner as a DID, its alias resolved."""
        owner = blank(self.vars["owner_entity"].get())
        if owner and not owner.startswith("0x") and self.db is not None:
            found = self.db.execute("SELECT value FROM alias WHERE name = ? AND kind = 'entity'", (owner,)).fetchone()
            owner = found[0] if found else None
        return owner


class ActionDialog(tk.Toplevel):
    """One action row: the action, its delay and p1..p5, with the action's parameter help."""

    def __init__(self, panel, row, done):
        super().__init__(panel)
        self.panel, self.done = panel, done
        self.title("Quest action")
        self.transient(panel.winfo_toplevel())
        delay, action, params, source = row or (0, "Tell", ["", "", "", "", ""], "")
        self.action = tk.StringVar(value=action)
        self.delay = tk.StringVar(value=str(delay or 0))
        self.params = [tk.StringVar(value=p or "") for p in params]
        self.source = tk.StringVar(value=source or "world builder")
        self.help = tk.StringVar()
        body = ttk.Frame(self, padding=8)
        body.pack(fill="both", expand=True)
        ttk.Label(body, text="Action").grid(row=0, column=0, sticky="e")
        box = ttk.Combobox(body, textvariable=self.action, values=sorted(panel.specs), width=30)
        box.grid(row=0, column=1, sticky="w")
        box.bind("<<ComboboxSelected>>", lambda event: self.describe())
        ttk.Label(body, text="Delay ms").grid(row=1, column=0, sticky="e")
        ttk.Entry(body, textvariable=self.delay, width=10).grid(row=1, column=1, sticky="w")
        self.labels = []
        for i, var in enumerate(self.params):
            label = ttk.Label(body, text=f"p{i + 1}")
            label.grid(row=2 + i, column=0, sticky="e")
            self.labels.append(label)
            ttk.Entry(body, textvariable=var, width=70).grid(row=2 + i, column=1, sticky="ew", pady=1)
        ttk.Label(body, text="source").grid(row=7, column=0, sticky="e")
        ttk.Entry(body, textvariable=self.source, width=70).grid(row=7, column=1, sticky="ew")
        ttk.Label(body, textvariable=self.help, wraplength=560, style="Muted.TLabel").grid(row=8, column=0, columnspan=2, sticky="w", pady=6)
        buttons = ttk.Frame(body)
        buttons.grid(row=9, column=0, columnspan=2, sticky="e")
        ttk.Button(buttons, text="Pick DAT line…", command=self.pick_line).pack(side="left", padx=4)
        ttk.Button(buttons, text="OK", command=self.ok).pack(side="left", padx=4)
        ttk.Button(buttons, text="Cancel", command=self.destroy).pack(side="left")
        self.describe()

    def describe(self):
        params, doc = self.panel.specs.get(self.action.get(), ([], "Unknown action: the server's check will refuse it."))
        for i, label in enumerate(self.labels):
            label.configure(text=(re.sub(r"<[^>]+>", "", params[i]).split(":")[0] if i < len(params) else f"p{i + 1}"))
        self.help.set(doc + ("\n" + "\n".join(re.sub(r"<[^>]+>", "", p) for p in params) if params else ""))

    def pick_line(self):
        """Lists the owner NPC's own DAT lines (or p1's table) and fills Tell table key."""
        panel = self.panel
        table = self.params[0].get().strip() if self.action.get() == "Tell" and self.params[0].get().startswith("0x25") else None
        try:
            query = f"table={table}" if table else f"entity={panel.owner_entity() or ''}"
            if not table and not panel.owner_entity():
                raise ValueError("The set names no owner NPC: type a table (0x25...) in p1 first")
            table, lines = panel.fetch_lines(query)
        except (OSError, ValueError, KeyError) as error:
            messagebox.showerror("DAT lines", f"Cannot list DAT lines: {error}", parent=self)
            return
        LinePicker(self, table, lines, self.chosen)

    def chosen(self, table, key, text):
        self.action.set("Tell")
        self.params[0].set(table)
        self.params[1].set(key)
        for var in self.params[2:]:
            var.set("")
        quote = " ".join(text.split())
        self.source.set(f"DAT {table}: '{quote[:70]}{'...' if len(quote) > 70 else ''}'")
        self.describe()

    def ok(self):
        try:
            delay = int(self.delay.get() or 0)
        except ValueError:
            messagebox.showerror("Quest action", "Delay is whole milliseconds.", parent=self)
            return
        self.done((delay, self.action.get().strip(), [v.get().strip() for v in self.params], self.source.get().strip() or "world builder"))
        self.destroy()


class LinePicker(tk.Toplevel):
    def __init__(self, parent, table, lines, chosen):
        super().__init__(parent)
        self.title(f"DAT lines of {table}")
        self.transient(parent)
        self.table, self.lines, self.chosen = table, lines, chosen
        self.find = tk.StringVar()
        self.find.trace_add("write", lambda *unused: self.fill())
        ttk.Entry(self, textvariable=self.find).pack(fill="x", padx=6, pady=6)
        self.list = ttk.Treeview(self, columns=("key", "text"), show="headings", height=16)
        self.list.heading("key", text="Key")
        self.list.heading("text", text="Line")
        self.list.column("key", width=90, stretch=False)
        self.list.column("text", width=640)
        self.list.pack(fill="both", expand=True, padx=6)
        self.list.bind("<Double-1>", lambda event: self.pick())
        ttk.Button(self, text="Use this line", command=self.pick).pack(anchor="e", padx=6, pady=6)
        self.fill()

    def fill(self):
        self.list.delete(*self.list.get_children())
        needle = self.find.get().lower()
        for key, text in self.lines.items():
            if text and (not needle or needle in text.lower() or needle in key):
                self.list.insert("", "end", iid=key, values=(key, " ".join(text.split())))

    def pick(self):
        chosen = self.list.selection()
        if chosen:
            self.chosen(self.table, chosen[0], self.lines[chosen[0]])
            self.destroy()
