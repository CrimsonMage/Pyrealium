"""The Quests tab over the shipped quest dump, with a stand-in for the server's DAT line list."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import shutil
import sqlite3
import tempfile
import threading
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

from editor_live import LiveLink
from editor_quest_panel import QuestPanel, action_specs

SHIPPED = Path(__file__).resolve().parent.parent / "content" / "quests.sql"
MOIRA = {"table": "0x2500895c", "lines": [
    {"key": "0x09200efe", "text": "Sorry, I don't have a task for you today."},
    {"key": "0x68f492a3", "text": "Come back to me when you are stronger."}]}


class Strings(BaseHTTPRequestHandler):
    asked = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        Strings.asked.append(self.path)
        data = json.dumps(MOIRA).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class QuestPanelTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        folder = Path(self.temp.name)
        (folder / "admin.token").write_text("secret\n")
        self.world = folder / "w.tsv"
        self.world.write_text("pyrealium-world-v2\n")
        shutil.copy(SHIPPED, folder / "w.quests.sql")
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Strings)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.root = tk.Tk()
        self.root.withdraw()
        self.addCleanup(self.root.destroy)
        self.editor = SimpleNamespace(root=self.root, closing=False, loaded_path=self.world,
                                      status=tk.StringVar(master=self.root), settings=dict(
                                          live_address=f"127.0.0.1:{self.server.server_port}", live_data=str(folder)))
        self.editor.live = LiveLink(self.editor)
        self.addCleanup(self.editor.live.close)
        self.panel = QuestPanel(self.root, self.editor)
        self.panel.open()

    def test_every_action_has_its_parameters_from_the_reference(self):
        specs = action_specs()
        self.assertIn("Tell", specs)
        self.assertTrue(specs["Tell"][0][0].startswith("p1 table"))
        self.assertGreater(len(specs), 40)

    def test_a_set_its_actions_and_a_dat_line_round_trip(self):
        panel = self.panel
        panel.filter.set("hunt_npc_47003fae")
        self.assertTrue(panel.tree.exists("set:hunt_npc_47003fae_none"))
        panel.tree.selection_set("set:hunt_npc_47003fae_none")
        self.root.update()
        self.assertEqual(panel.vars["trigger"].get(), "Goto")
        # A Tell shows its source quote until the live server lists the table, then the line.
        before = panel.actions.item("1", "values")
        self.assertEqual(before[1], "Tell")
        panel.fetch_lines("entity=0x47003fae")
        panel.refresh_actions()
        self.assertEqual(panel.actions.item("1", "values")[3], "Sorry, I don't have a task for you today.")
        self.assertEqual(Strings.asked[-1], "/admin/strings?entity=0x47003fae")
        # A new own-text line after it, moved first, then saved: the database holds both.
        panel.renumber(panel.current_rows() + [(0, "Say", ["Hello there.", "", "", "", ""], "world builder")])
        panel.actions.selection_set("2")
        panel.move_action(-1)
        panel.save()
        db = sqlite3.connect(self.world.with_name("w.quests.sqlite"))
        rows = db.execute("SELECT ord, action, p1 FROM emote_action WHERE emote_set = 'hunt_npc_47003fae_none' ORDER BY ord").fetchall()
        self.assertEqual(rows, [(1, "Say", "Hello there."), (2, "Tell", "0x2500895c")])

    def test_a_new_set_is_refused_when_the_schema_refuses_it(self):
        panel = self.panel
        panel.new_set()
        self.assertTrue(panel.current.startswith("new_set"))
        panel.vars["phrase"].set("gear")  # a phrase on a Use set
        shown = []
        import editor_quest_panel
        original = editor_quest_panel.messagebox.showerror
        editor_quest_panel.messagebox.showerror = lambda title, text, **kw: shown.append(text)
        try:
            panel.apply_set()
        finally:
            editor_quest_panel.messagebox.showerror = original
        self.assertTrue(shown and "CHECK" in shown[0], shown)


if __name__ == "__main__":
    unittest.main()
