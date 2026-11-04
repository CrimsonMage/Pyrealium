"""The To place tab over the quest database's placement hints: a double-click opens the guide's spot, armed."""
import sqlite3
import tempfile
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

from editor_to_place import ToPlace

ROWS = [("0x47001234", "start NPC", "Kiraly Geddon", -4.8, -44.8, "0x2c4b0021", 12.5, 30.0, "The Eye (shipped)", "vault", "guide"),
        ("0x47005678", "creature to spawn", "Gurog Emissary", None, None, None, None, None, "Darkstone", "vault", "named only")]


class ToPlaceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        path = Path(self.temp.name) / "w.quests.sqlite"
        db = sqlite3.connect(path)
        db.execute("CREATE TABLE placement_hint (entity TEXT PRIMARY KEY, what TEXT, name TEXT, north REAL, east REAL, cell TEXT, "
                   "x REAL, y REAL, quests TEXT, family TEXT, source TEXT)")
        db.executemany("INSERT INTO placement_hint VALUES (?,?,?,?,?,?,?,?,?,?,?)", ROWS)
        db.commit()
        db.close()
        self.root = tk.Tk()
        self.root.withdraw()
        self.addCleanup(self.root.destroy)
        self.went = []
        self.editor = SimpleNamespace(fields={"Entity": tk.StringVar(master=self.root)}, recent_entities=[], has_travelled=False,
                                      navigate_to=lambda cell, target_xy=None: self.went.append((cell, target_xy)),
                                      view3d=SimpleNamespace(set_tool=lambda t: self.went.append(t)),
                                      workspace=SimpleNamespace(select=lambda i: None), status=tk.StringVar(master=self.root))
        self.tab = ToPlace(self.root, self.editor, path)

    def test_a_row_with_coordinates_opens_its_spot_with_the_entity(self):
        row = next(i for i in self.tab.list.get_children() if self.tab.list.item(i, "values")[1] == "Kiraly Geddon")
        self.tab.list.selection_set(row)
        self.tab.go()
        self.assertEqual(self.editor.fields["Entity"].get(), "47001234")
        self.assertEqual(self.went, [(0x2c4b0021, (12.5, 30.0)), "place"])

    def test_a_row_without_coordinates_only_fills_the_entity(self):
        self.tab.find.set("emissary")
        self.assertEqual(len(self.tab.list.get_children()), 1)
        self.tab.list.selection_set(self.tab.list.get_children()[0])
        self.tab.go()
        self.assertEqual((self.editor.fields["Entity"].get(), self.went), ("47005678", []))


if __name__ == "__main__":
    unittest.main()
