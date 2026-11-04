"""Live server link against a stand-in admin endpoint."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sqlite3
import tempfile
import threading
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

import editor_quests
from editor_live import LiveLink

SERVER_QUESTS = ("PRAGMA user_version = 1;\nCREATE TABLE IF NOT EXISTS quest (name TEXT PRIMARY KEY, title TEXT);\n"
                 "INSERT INTO quest (name, title) VALUES ('shelter', 'Rescue the Shelter Hold');\n")


class Endpoint(BaseHTTPRequestHandler):
    revision = "r0"
    bodies = []
    quests = SERVER_QUESTS

    def log_message(self, *args):
        pass

    def reply(self, code, value):
        data = json.dumps(value).encode()
        self.send_response(code)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/admin/quests":
            return self.reply(200, dict(revision=Endpoint.revision, quests=Endpoint.quests))
        self.reply(200, dict(revision=Endpoint.revision, objects=0, players=0))

    def do_POST(self):
        assert self.headers["Authorization"] == "Bearer secret"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if body["base"] != Endpoint.revision:
            return self.reply(409, dict(error="stale", revision=Endpoint.revision))
        Endpoint.bodies.append(body)
        Endpoint.revision = f"r{len(Endpoint.bodies)}"
        changed = None
        if "quests" in body and body["quests"] != Endpoint.quests:
            # The real server answers with the canonical text of what it took.
            Endpoint.quests, changed = "-- canonical\n" + body["quests"], 2
        self.reply(200, dict(revision=Endpoint.revision, added=1, removed=0, replaced=0, rules=0, quests=changed,
                             messages=1, apply_us=12, saved=True))


class LiveLinkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        folder = Path(self.temp.name)
        (folder / "admin.token").write_text("secret\n")
        self.world = folder / "w.tsv"
        self.world.write_text("pyrealium-world-v2\n")
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Endpoint)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        Endpoint.revision, Endpoint.bodies, Endpoint.quests = "r0", [], SERVER_QUESTS
        self.root = tk.Tk()
        self.root.withdraw()
        self.addCleanup(self.root.destroy)
        self.editor = SimpleNamespace(root=self.root, closing=False, loaded_path=self.world,
                                      status=tk.StringVar(master=self.root), settings=dict(
                                          live_address=f"127.0.0.1:{self.server.server_port}", live_data=str(folder)))
        self.link = LiveLink(self.editor)
        self.addCleanup(self.link.close)

    def wait(self):
        for _ in range(500):
            self.root.update()
            if self.link.future is None and not self.link.dirty:
                return
            self.root.after(5)
        self.fail("sync did not finish")

    def test_stale_revision_preserves_other_edits_and_stops_queued_pushes(self):
        self.link.enabled.set(True)
        self.link.toggle()
        self.wait()
        self.assertEqual(len(Endpoint.bodies), 1)
        self.assertIn("applied in 12 µs", self.link.state.get())
        Endpoint.revision = "changed elsewhere"
        self.world.write_text("pyrealium-world-v2\n# a\n")
        self.link.changed()
        self.world.write_text("pyrealium-world-v2\n# b\n")
        self.link.changed()  # in flight: queued, sent once with the newest text
        self.link.changed()
        self.wait()
        self.assertEqual(len(Endpoint.bodies), 1)
        self.assertEqual(Endpoint.revision, "changed elsewhere")
        self.assertFalse(self.link.enabled.get())
        self.assertFalse(self.link.dirty)

    def test_connecting_loads_the_servers_quests_and_a_database_save_pushes_them(self):
        quests = editor_quests.quests_path(self.world)
        self.link.enabled.set(True)
        self.link.toggle()
        self.wait()
        # No quest file here yet: the first push leaves the server's quests, and keeps them.
        self.assertNotIn("quests", Endpoint.bodies[0])
        self.assertEqual(quests.read_text(), SERVER_QUESTS)
        # Edited as a database in another tool: the watch pushes its dump, and the server's
        # canonical text becomes the dump here.
        database = editor_quests.build_database(self.world)
        db = sqlite3.connect(database)
        db.execute("UPDATE quest SET title = 'Edited' WHERE name = 'shelter'")
        db.commit()
        db.close()
        for _ in range(400):
            self.root.update()
            if len(Endpoint.bodies) == 2 and self.link.future is None:
                break
            self.root.after(10)
        self.assertIn("'Edited'", Endpoint.bodies[1]["quests"])
        self.assertEqual(quests.read_text(), Endpoint.quests)
        self.assertTrue(quests.read_text().startswith("-- canonical\n"))
        self.assertIn("quest tables replaced (2 quests)", self.link.state.get())
        # The database keeps its edits (a tool may have it open): Edit quests does not rebuild
        # it, and nothing more is pushed until the next save.
        stamp = editor_quests.mtime(database)
        self.assertEqual(editor_quests.build_database(self.world), database)
        self.assertEqual(editor_quests.mtime(database), stamp)
        self.root.after(1100)
        for _ in range(150):
            self.root.update()
            self.root.after(10)
        self.assertEqual(len(Endpoint.bodies), 2)

    def test_disabled_link_sends_nothing(self):
        self.link.changed()
        self.root.update()
        self.assertEqual(Endpoint.bodies, [])


if __name__ == "__main__":
    unittest.main()
