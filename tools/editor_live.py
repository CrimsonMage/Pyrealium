"""Live server link: every saved content file is pushed to a local running server.

The server (`--admin-bind 127.0.0.1:PORT --content-overlay FILE`) applies the
overlay world and rules texts between game ticks and saves them to its overlay
paths. The editor's files are the source: connecting pushes them, and each save
pushes again. Requests run on their own thread; one sync is in flight at a time
and later saves coalesce into one follow-up with the newest texts.
"""
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import tkinter as tk
from tkinter import ttk
import urllib.error
import urllib.request

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

    def build(self, parent, row):
        frame = ttk.Frame(parent)
        frame.grid(row=row, column=0, columnspan=3, sticky="ew", pady=4)
        ttk.Checkbutton(frame, text="Live server", variable=self.enabled, command=self.toggle).pack(side="left")
        ttk.Entry(frame, textvariable=self.address, width=20).pack(side="left", padx=6)
        ttk.Label(frame, text="Server data folder").pack(side="left", padx=(10, 4))
        ttk.Entry(frame, textvariable=self.data_folder).pack(side="left", fill="x", expand=True)
        ttk.Label(frame, textvariable=self.state, style="Muted.TLabel").pack(side="left", padx=8)

    def token(self):
        folder = self.data_folder.get().strip()
        if not folder:
            raise ValueError("Set the server data folder (where admin.token is)")
        return bounded_text(Path(folder) / "admin.token").strip()

    def toggle(self):
        if self.enabled.get():
            self.revision = None
            self.changed()
        else:
            self.state.set("Live server: off")
        if hasattr(self.editor, "show_unsaved"):
            self.editor.show_unsaved()

    def texts(self):
        path = self.editor.loaded_path
        if path is None:
            raise ValueError("Load a content file first")
        world = bounded_text(path) if path.exists() else ""
        rules = rules_path(path)
        return world, bounded_text(rules) if rules.exists() else ""

    def changed(self):
        """A file was saved: push it (or queue one push after the current one)."""
        if not self.enabled.get() or self.editor.closing:
            return
        if self.future is not None:
            self.dirty = True
            return
        try:
            token, (world, rules) = self.token(), self.texts()
        except (OSError, ValueError) as error:
            self.state.set(f"Live server: {error}")
            return
        self.dirty = False
        self.state.set("Live server: syncing…")
        self.future = self.executor.submit(self.push, self.address.get().strip(), token, self.revision, world, rules)
        self.editor.root.after(20, self.finish)

    @staticmethod
    def push(address, token, base, world, rules):
        """Worker thread. Refuse stale edits without overwriting another editor."""
        if base is None:
            code, status = request(address, token, "GET", "/admin/status")
            if code != 200:
                return code, status
            base = status["revision"]
        code, reply = request(address, token, "POST", "/admin/sync", dict(base=base, world=world, rules=rules))
        return code, reply

    def finish(self):
        if self.future is None or self.editor.closing:
            return
        if not self.future.done():
            self.editor.root.after(20, self.finish)
            return
        future, self.future = self.future, None
        try:
            code, reply = future.result()
        except (OSError, ValueError, KeyError) as error:
            code, reply = 0, {"error": f"cannot reach the server ({error})"}
        if code == 200:
            self.revision = reply["revision"]
            moved = reply["added"] + reply["removed"] + reply["replaced"]
            self.state.set(f"Live: +{reply['added']} −{reply['removed']} ~{reply['replaced']} objects, "
                           f"{reply['rules']} rule changes, applied in {reply['apply_us']} µs")
            if moved or reply["rules"]:
                self.editor.status.set("Live server updated: players nearby see the change now.")
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
