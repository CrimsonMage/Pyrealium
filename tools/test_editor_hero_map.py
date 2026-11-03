"""AC2Hero map export: coordinates, portal routes and the map widget."""
import json
import tempfile
import tkinter as tk
import unittest
from pathlib import Path
from types import SimpleNamespace

from editor_hero_map import HeroMap, cell_at, load_export, map_units, parse_destination, read_routes, snap_tile_scale, world_point

CONTENT = Path(__file__).resolve().parent.parent / "content"


class CoordinateTests(unittest.TestCase):
    def test_block_local_points_round_trip_through_world_pixels(self):
        for cell, x, y in ((0x76b80022, 85.5, 29.5), (0x00000001, 0, 0), (0xfefe0040, 159.9, 159.9)):
            gx, gy = world_point(cell, x, y)
            back, bx, by = cell_at(gx, gy)
            self.assertEqual(back >> 16, cell >> 16)
            self.assertAlmostEqual(bx, x, places=3)
            self.assertAlmostEqual(by, y, places=3)
        self.assertEqual(cell_at(*world_point(0x76b80022, 85.5, 29.5))[0], 0x76b80022)
        with self.assertRaises(ValueError):
            cell_at(-1, 5)

    def test_map_units_match_the_game_compass(self):
        # 1020 cells from the west/south edges is 0, 0; 10.2 cells per unit.
        self.assertEqual(map_units(1020, 1020), "0.0N, 0.0E")
        self.assertEqual(map_units(1020 + 102, 1020 + 51), "5.0S, 10.0E")

    def test_typed_destinations(self):
        # A coordinate lands on the cell and block-local point under it (the map's own frame).
        cell, x, y = parse_destination("31.2N, 93.1E")
        self.assertEqual(cell, 0xf6a7000b)
        self.assertEqual(map_units(*world_point(cell, x, y)), "31.2N, 93.1E")
        self.assertEqual(parse_destination("8.6s 52.6w")[0], parse_destination("8.6S, 52.6W")[0])
        # A landblock opens at its middle, a cell at its own middle; a room has no outdoor point.
        self.assertEqual(parse_destination("0xCF8C"), (0xcf8c0025, 80.0, 80.0))
        self.assertEqual(parse_destination("cf8c0005"), (0xcf8c0005, 10.0, 90.0))
        self.assertEqual(parse_destination("02990109"), (0x02990109, None, None))
        # Place names stay searches, even ones spelled in hex letters.
        self.assertIsNone(parse_destination("Arwic"))
        self.assertIsNone(parse_destination("cafe"))
        with self.assertRaises(ValueError):
            parse_destination("ff00")

    def test_tile_scale_snaps_to_whole_zoom_ratios(self):
        for wanted in (0.05, 0.3, 0.4, 1.0, 2.0, 6.5):
            scale, (up, down) = snap_tile_scale(wanted, 160)
            self.assertAlmostEqual(scale * 160, 64 * up / down)
            self.assertLess(abs(scale / wanted - 1), 0.2)

    def test_bundled_portal_routes_parse(self):
        routes = read_routes(CONTENT / "portal-routes.tsv")
        self.assertGreater(len(routes), 100)
        route = routes["8140000000030022"]
        self.assertEqual(route["destination"][0], 0x02990109)


class WidgetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        folder = Path(self.temp.name)
        (folder / "blocks").mkdir()
        (folder / "icons").mkdir()
        self.root = tk.Tk()  # mapped: the map needs a real canvas size
        self.addCleanup(self.root.destroy)
        world = tk.PhotoImage(master=self.root, width=2040, height=2040)
        world.put("#2c588a", to=(0, 0, 2040, 2040))
        world.write(str(folder / "world.png"), format="png")
        tile = tk.PhotoImage(master=self.root, width=64, height=64)
        tile.put("#79a039", to=(0, 0, 64, 64))
        tile.write(str(folder / "blocks" / "7fa0.png"), format="png")
        tile.write(str(folder / "icons" / "4100512c.png"), format="png")
        (folder / "manifest.json").write_text(json.dumps(dict(format="ac2hero-map-export-v1", world_cells=2040)))
        (folder / "notes.json").write_text(json.dumps([dict(name="Testtown", kind="Towns", north=1.0, east=2.0, icon="4100512c")]))
        self.trips = []
        self.map = HeroMap(self.root, lambda cell, position=None, target_xy=None: self.trips.append((cell, position, target_xy)))
        self.map.pack(fill="both", expand=True)
        self.map.canvas.configure(width=600, height=400)
        self.root.geometry("640x520")
        self.root.update()
        self.map.set_export(load_export(folder))

    def test_draws_notes_tiles_and_travels(self):
        note = self.map.export["notes"][0]
        self.map.show(note["gx"], note["gy"], level=5)  # 8 px per cell: block tiles
        self.root.update()
        self.map.show(127 * 8 + 4, 2040 - (0xa0 * 8 + 4), level=5)
        self.map.load_tiles()
        self.assertIn(("7fa0", 8), self.map.tiles)
        self.map.show(note["gx"], note["gy"], level=5)
        kinds = {kind for _, _, kind, _ in self.map.hits}
        self.assertEqual(kinds, {"note"})
        width, height = self.map.canvas.winfo_width(), self.map.canvas.winfo_height()
        self.map.travel(SimpleNamespace(x=width / 2 + 40, y=height / 2 + 40, x_root=0, y_root=0))
        cell, _, target = self.trips[-1]
        clicked = cell_at(*self.map.to_world(width / 2 + 40, height / 2 + 40))
        self.assertEqual(cell, clicked[0])
        self.assertEqual(target, clicked[1:])
        # A typed coordinate travels to its point and centres the map there.
        self.map.query.set("1.0N, 2.0E")
        self.map.find()
        cell, _, (x, y) = self.trips[-1]
        self.assertEqual(map_units(*world_point(cell, x, y)), "1.0N, 2.0E")
        self.assertEqual(map_units(*self.map.center), "1.0N, 2.0E")
        # A place name still finds the note.
        self.map.query.set("testtown")
        trips = len(self.trips)
        self.map.find()
        self.assertEqual(len(self.trips), trips)
        self.assertEqual(self.map.center, [note["gx"], note["gy"]])

    def test_portal_destinations_are_named_and_drawn(self):
        self.map.editor = SimpleNamespace(portal_routes=read_routes(CONTENT / "portal-routes.tsv"), locations=[
            dict(cell=0x02990109, name="Training room")], population=SimpleNamespace(mine=lambda: [], reference=[]))
        outdoor = [p for p in self.map.portals() if p["destination"]]
        self.assertTrue(outdoor)
        portal = next(p for p in outdoor if p["destination"][0] == 0x02990109)
        self.assertEqual(self.map.place_name(*portal["destination"]), "Training room")
        self.map.layers["Creatures"].set(False)
        self.map.show(*world_point(portal["cell"], portal["x"], portal["y"]), level=4)
        self.assertIn("portal", {kind for _, _, kind, _ in self.map.hits})


if __name__ == "__main__":
    unittest.main()
