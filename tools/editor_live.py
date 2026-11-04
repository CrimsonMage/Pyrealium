"""Live server link: every saved content file is pushed to a local running server.

The server (`--admin-bind 127.0.0.1:PORT --content-overlay FILE`) applies the
overlay world and rules texts between game ticks and saves them to its overlay
paths. The editor's files are the source: connecting pushes them, and each save
pushes again. Requests run on their own thread; one sync is in flight at a time
and later saves coalesce into one follow-up with the newest texts.

The quest database (`editor_quests`) travels with them: with no NAME.quests.sql
yet, connecting loads the server's. A push carries the quests only when one of
the quest files changed since the last sync (a save in any SQLite tool pushes on
its own), so placement edits never pin the server's quests: it keeps only quest
edits, as a layer over its shipped quests.
"""
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sqlite3
import tkinter as tk
from tkinter import ttk
import urllib.error
import urllib.request

import editor_content
import editor_quests
from editor_rules import rules_path
from place_content import bounded_text


def request(address, token, method, path, body=None, timeout=10):
    data = None if body is None else json.dumps(body).encode("utf-8")
    req = urllib.request.Request(f"http://{address}{path}", data=data, method=method,
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        try:
            return error.code, json.loads(error.read() or b"{}")
        except ValueError:
            return error.code, {"error": str(error)}


class LiveLink:
    def __init__(self, editor):
        self.editor = editor
        settings = editor.settings
        self.address = tk.StringVar(value=settings.get("live_address", "127.0.0.1:19373"))
        self.data_folder = tk.StringVar(value=settings.get("live_data", ""))
        self.enabled = tk.BooleanVar(value=False)
        self.state = tk.StringVar(value="Live server: off")
        self.executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="live-server")
        self.future = None
        self.revision = None
        self.dirty = False
        self.quest_signature = None

    def build(self, parent, row):
        frame = ttk.Frame(parent)
        frame.grid(row=row, column=0, columnspan=3, sticky="ew", pady=4)
        ttk.Checkbutton(frame, text="Live server", variable=self.enabled, command=self.toggle).pack(side="left")
        ttk.Entry(frame, textvariable=self.address, width=20).pack(side="left", padx=6)
        ttk.Label(frame, text="Server data folder").pack(side="left", padx=(10, 4))
        ttk.Entry(frame, textvariable=self.data_folder).pack(side="left", fill="x", expand=True)
        ttk.Label(frame, textvariable=self.state, style="Muted.TLabel").pack(side="left", padx=8)
        ttk.Button(frame, text="Edit quests", command=self.edit_quests).pack(side="left", padx=4)

    def edit_quests(self):
        """Opens NAME.quests.sqlite in the system's SQLite tool; its saves push live."""
        path = self.editor.loaded_path
        if path is None:
            self.editor.status.set("Load a content file first.")
            return
        try:
            database = editor_quests.build_database(path)
            editor_quests.open_in_tool(database)
        except (OSError, ValueError, sqlite3.Error) as error:
            self.editor.status.set(f"Cannot edit the quests: {error}")
            return
        live = " Saves there push to the live server." if self.enabled.get() else " Turn on Live server to push its saves."
        self.editor.status.set(f"Editing {database.name}.{live}")

    def token(self):
        folder = self.data_folder.get().strip()
        if not folder:
            raise ValueError("Set the server data folder (where admin.token is)")
        return bounded_text(Path(folder) / "admin.token").strip()

    def toggle(self):
        if self.enabled.get():
            self.revision = None
            self.changed()
            self.editor.root.after(1000, self.watch)
        else:
            self.state.set("Live server: off")
        if hasattr(self.editor, "show_unsaved"):
            self.editor.show_unsaved()

    def texts(self):
        """The world's content dump (its placements, removals, rules and quests)."""
        path = self.editor.loaded_path
        if path is None:
            raise ValueError("Load a content file first")
        return editor_quests.quest_text(path)

    def watch(self):
        """While live, a save of the quest files (in another tool) pushes them."""
        if not self.enabled.get() or self.editor.closing:
            return
        path = self.editor.loaded_path
        if path is not None and self.quest_signature is not None and editor_quests.signature(path) != self.quest_signature:
            self.changed()
        self.editor.root.after(1000, self.watch)

    def changed(self):
        """A file was saved: push it (or queue one push after the current one)."""
        if not self.enabled.get() or self.editor.closing:
            return
        if self.future is not None:
            self.dirty = True
            return
        try:
            token, content = self.token(), self.texts()
        except (OSError, ValueError, sqlite3.Error) as error:
            self.state.set(f"Live server: {error}")
            return
        self.dirty = False
        self.state.set("Live server: syncing…")
        self.future = self.executor.submit(self.push, self.address.get().strip(), token, self.revision, content)
        self.editor.root.after(20, self.finish)

    @staticmethod
    def push(address, token, base, content):
        """Worker thread. Refuse stale edits without overwriting another editor. With no content
        here yet (the first connection), the server's is fetched; else the dump is pushed and
        the server's canonical text of it comes back. Returns the reply and the dump to keep."""
        if base is None:
            code, status = request(address, token, "GET", "/admin/status")
            if code != 200:
                return code, status, None
            base = status["revision"]
        if content is None:
            code, fetched = request(address, token, "GET", "/admin/content", timeout=60)
            if code != 200:
                return code, fetched, None
            reply = dict(revision=fetched["revision"], added=0, removed=0, replaced=0, rules=0, quests=None, apply_us=0,
                         fetched=True)
            return code, reply, fetched["content"]
        code, reply = request(address, token, "POST", "/admin/sync", dict(base=base, content=content), timeout=60)
        return code, reply, reply.get("content") if code == 200 else None

    def finish(self):
        if self.future is None or self.editor.closing:
            return
        if not self.future.done():
            self.editor.root.after(20, self.finish)
            return
        future, self.future = self.future, None
        try:
            code, reply, quests = future.result()
        except (OSError, ValueError, KeyError) as error:
            code, reply, quests = 0, {"error": f"cannot reach the server ({error})"}, None
        if code == 200:
            self.revision = reply["revision"]
            moved = reply["added"] + reply["removed"] + reply["replaced"]
            changed_quests = reply.get("quests")
            quest_note = f", content reloaded ({changed_quests} quests)" if changed_quests is not None else ""
            self.state.set(f"Live: +{reply['added']} −{reply['removed']} ~{reply['replaced']} objects, "
                           f"{reply['rules']} rule changes{quest_note}, applied in {reply['apply_us']} µs")
            if moved or reply["rules"] or changed_quests is not None:
                self.editor.status.set("Live server updated: players nearby see the change now.")
            if quests and self.editor.loaded_path is not None:
                try:
                    if reply.get("fetched"):
                        # The server's world replaces the one started here (nothing of its own yet).
                        editor_content.content_db_path(self.editor.loaded_path).unlink(missing_ok=True)
                        editor_content.content_path(self.editor.loaded_path).unlink(missing_ok=True)
                    if editor_quests.save_dump(self.editor.loaded_path, quests) and changed_quests is None:
                        self.editor.status.set(f"Loaded the server's content into "
                                               f"{editor_quests.quests_path(self.editor.loaded_path).name}.")
                    if reply.get("fetched"):
                        editor_content.ensure_database(self.editor.loaded_path)
                        # Received from the server: from now on this world's edits push (the server's own
                        # NAME.content.base.sql is the shipped content its merge needs; it is not touched).
                        editor_content.received_path(self.editor.loaded_path).write_text(reply["revision"] + "\n", encoding="utf-8")
                        if hasattr(self.editor, "reload"):
                            self.editor.reload(force=True)
                except (OSError, ValueError, sqlite3.Error) as error:
                    self.editor.status.set(f"Could not save the server's content: {error}")
                self.quest_signature = editor_quests.signature(self.editor.loaded_path)
            if not reply.get("saved", True):
                self.editor.status.set(f"Live server applied the edit but could not save it: {reply.get('save_error')}")
        else:
            # An uncertain response or stale revision requires an explicit reconnect.
            # Do not queue another save against a freshly fetched revision.
            self.enabled.set(False)
            self.dirty = False
            message = reply.get("error", f"HTTP {code}")
            self.state.set("Live server: not applied")
            self.editor.status.set(f"Live sync stopped; check the server before reconnecting: {message}")
        if self.dirty:
            self.changed()

    def close(self):
        self.enabled.set(False)
        self.executor.shutdown(wait=False, cancel_futures=True)
