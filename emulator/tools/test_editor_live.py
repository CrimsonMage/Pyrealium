"""Live server link against a stand-in admin endpoint."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import tempfile
import threading
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

from editor_live import LiveLink


class Endpoint(BaseHTTPRequestHandler):
    revision = "r0"
    bodies = []

    def log_message(self, *args):
        pass

    def reply(self, code, value):
        data = json.dumps(value).encode()
        self.send_response(code)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.reply(200, dict(revision=Endpoint.revision, objects=0, players=0))

    def do_POST(self):
        assert self.headers["Authorization"] == "Bearer secret"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if body["base"] != Endpoint.revision:
            return self.reply(409, dict(error="stale", revision=Endpoint.revision))
        Endpoint.bodies.append(body)
        Endpoint.revision = f"r{len(Endpoint.bodies)}"
        self.reply(200, dict(revision=Endpoint.revision, added=1, removed=0, replaced=0, rules=0,
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
        Endpoint.revision, Endpoint.bodies = "r0", []
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

    def test_disabled_link_sends_nothing(self):
        self.link.changed()
        self.root.update()
        self.assertEqual(Endpoint.bodies, [])


if __name__ == "__main__":
    unittest.main()
