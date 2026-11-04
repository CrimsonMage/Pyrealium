"""Live server link against a stand-in admin endpoint (the content-database protocol)."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sqlite3
import tempfile
import threading
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

import editor_content
from editor_live import LiveLink

# What the server hands a world builder: the shipped content without the shipped world's rows.
SERVER_CONTENT = editor_content.without_world(editor_content.read_dump(editor_content.SHIPPED, limit=None))


class Endpoint(BaseHTTPRequestHandler):
    revision = "r0"
    bodies = []
    content = SERVER_CONTENT

    def log_message(self, *args):
        pass

    def reply(self, code, value):
        data = json.dumps(value).encode()
        self.send_response(code)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path in ("/admin/content", "/admin/quests"):
            return self.reply(200, dict(revision=Endpoint.revision, content=Endpoint.content))
        self.reply(200, dict(revision=Endpoint.revision, objects=0, players=0))

    def do_POST(self):
        assert self.headers["Authorization"] == "Bearer secret"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert set(body) == {"base", "content"}, body.keys()
        if body["base"] != Endpoint.revision:
            return self.reply(409, dict(error="stale", revision=Endpoint.revision))
        Endpoint.bodies.append(body)
        Endpoint.revision = f"r{len(Endpoint.bodies)}"
        changed = None
        if body["content"] != Endpoint.content:
            # The real server answers with the canonical text of what it took.
            Endpoint.content, changed = "-- canonical\n" + body["content"], 2
        self.reply(200, dict(revision=Endpoint.revision, added=1, removed=0, replaced=0, rules=0, quests=changed,
                             messages=1, apply_us=12, saved=True, content=Endpoint.content))


class LiveLinkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        folder = Path(self.temp.name)
        (folder / "admin.token").write_text("secret\n")
        self.world = folder / "w.tsv"
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Endpoint)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        Endpoint.revision, Endpoint.bodies, Endpoint.content = "r0", [], SERVER_CONTENT
        self.root = tk.Tk()
        self.root.withdraw()
        self.addCleanup(self.root.destroy)
        self.editor = SimpleNamespace(root=self.root, closing=False, loaded_path=self.world,
                                      status=tk.StringVar(master=self.root), settings=dict(
                                          live_address=f"127.0.0.1:{self.server.server_port}", live_data=str(folder)))
        self.link = LiveLink(self.editor)
        self.addCleanup(self.link.close)

    def wait(self):
        for _ in range(1000):
            self.root.update()
            if self.link.future is None and not self.link.dirty:
                return
            self.root.after(5)
        self.fail("sync did not finish")

    def connect(self):
        self.link.enabled.set(True)
        self.link.toggle()
        self.wait()

    def place(self, x):
        """A placement saved in the editor (its world's builder rows)."""
        editor_content.write_world(self.world, "pyrealium-world-v3\n8140000000020000\t47000001\t1f000001\t02990109\t"
                                   f"{x}\t2\t3\t1\t0\t0\t0\t-\n")

    def test_connecting_loads_the_servers_content_and_saves_push_it(self):
        self.connect()
        # Nothing of its own here yet: the server's content is fetched, nothing is pushed.
        self.assertEqual(Endpoint.bodies, [])
        self.assertEqual(editor_content.content_path(self.world).read_text(), SERVER_CONTENT)
        self.assertTrue(editor_content.content_db_path(self.world).exists())
        # Edited as a database in another tool: the watch pushes its dump, and the server's
        # canonical text becomes the dump here.
        db = sqlite3.connect(editor_content.content_db_path(self.world))
        db.execute("UPDATE quest SET title = 'Edited' WHERE name = (SELECT MIN(name) FROM quest)")
        db.commit()
        db.close()
        editor_content.os.utime(editor_content.content_db_path(self.world))
        for _ in range(400):
            self.root.update()
            if len(Endpoint.bodies) == 1 and self.link.future is None:
                break
            self.root.after(10)
        self.assertIn("'Edited'", Endpoint.bodies[0]["content"])
        self.assertEqual(editor_content.content_path(self.world).read_text(), Endpoint.content)
        self.assertTrue(Endpoint.content.startswith("-- canonical\n"))
        # A placement save pushes the dump with its builder row.
        self.place(5)
        self.link.changed()
        self.wait()
        self.assertIn("'0x8140000000020000'", Endpoint.bodies[-1]["content"])

    def test_stale_revision_preserves_other_edits_and_stops_queued_pushes(self):
        self.connect()
        Endpoint.revision = "changed elsewhere"
        self.place(5)
        self.link.changed()
        self.place(6)
        self.link.changed()  # in flight: queued, sent once with the newest text
        self.link.changed()
        self.wait()
        self.assertEqual(Endpoint.bodies, [])
        self.assertEqual(Endpoint.revision, "changed elsewhere")
        self.assertFalse(self.link.enabled.get())
        self.assertFalse(self.link.dirty)

    def test_disabled_link_sends_nothing(self):
        self.link.changed()
        self.root.update()
        self.assertEqual(Endpoint.bodies, [])


if __name__ == "__main__":
    unittest.main()
