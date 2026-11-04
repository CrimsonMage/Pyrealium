"""The To place tab over a placement list: a double-click opens the guide's spot, armed."""
import tempfile
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

from editor_to_place import ToPlace

ROWS = ("what\tname\tentity\tentities\tnorth\teast\tcell\tx\ty\tquests\n"
        "start NPC\tKiraly Geddon\t0x47001234\t0x47001234\t-4.8\t-44.8\t0x2c4b0021\t12.5\t30.0\tThe Eye (shipped)\n"
        "creature to spawn\tGurog Emissary\t0x47005678\t0x47005678\t\t\t\t\t\tDarkstone\n")


class ToPlaceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        path = Path(self.temp.name) / "to-place.tsv"
        path.write_text(ROWS)
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
        self.tab.list.selection_set("0")
        self.tab.go()
        self.assertEqual(self.editor.fields["Entity"].get(), "47001234")
        self.assertEqual(self.went, [(0x2c4b0021, (12.5, 30.0)), "place"])

    def test_a_row_without_coordinates_only_fills_the_entity(self):
        self.tab.find.set("emissary")
        self.assertEqual(self.tab.list.get_children(), ("1",))
        self.tab.list.selection_set("1")
        self.tab.go()
        self.assertEqual((self.editor.fields["Entity"].get(), self.went), ("47005678", []))


if __name__ == "__main__":
    unittest.main()
