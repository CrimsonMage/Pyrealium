"""Creature tools workflow: list, inspector rules, patrols, spawn groups, overlays."""
import os
import tkinter as tk
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from content_editor import Editor
from editor_document import read_document, rows_from_text
from editor_rules import Rules, rules_path
import test_editor_document as fixtures

COMBAT = ("pyrealium-creature-combat-v3\n# entity health damage interval armor xp level variance attack defense perception homesick type vigor\n"
          "47000001\t460\t73\t0\t10\t48\t15\t0.5\t251\t176\t30.0\t0\t0\t460\n")
AI = "pyrealium-creature-ai-v1\n47000001\t30\t-\t1\t12\t0.3\t6\t-\t-\t497\t-\n"
ITEMS = "pyrealium-loot-items-v1\n47000ed7\t1f0005a5\tPlatemail Hauberk\n"


class PopulationWorkflowTests(unittest.TestCase):
    def setUp(self):
        fixtures.DocumentTests.setUp(self)
        for name, text in (("creature-combat.tsv", COMBAT), ("creature-ai.tsv", AI), ("loot-items.tsv", ITEMS)):
            (self.folder / name).write_text(text, encoding="utf-8")
        self.args.output, self.args.game_data, self.args.no_settings = self.path, self.folder, True
        for target in (patch.dict(os.environ, {"LOCALAPPDATA": str(self.folder)}), patch.object(Editor, "refresh_scene"),
                       patch("content_editor.messagebox.showerror", side_effect=AssertionError("error dialog")),
                       patch("editor_population.messagebox.showerror", side_effect=AssertionError("error dialog"))):
            target.start()
            self.addCleanup(target.stop)
        self.root = tk.Tk()
        self.root.withdraw()
        self.editor = Editor(self.root, self.args)
        self.addCleanup(self.cleanup)
        self.population = self.editor.population
        self.identity = self.row[:16]

    def cleanup(self):
        self.editor.closing = True
        self.editor.view3d.close()
        self.editor.executor.shutdown(wait=True)
        for callback in self.root.tk.call("after", "info"):
            self.root.after_cancel(callback)
        self.root.destroy()

    def rules(self):
        from editor_rules import load_rules
        return load_rules(self.path)[0]

    def inspect(self):
        self.editor.show_creatures()
        self.assertEqual(self.population.tree.get_children(), (self.identity,))
        self.population.select(self.identity)
        self.population.open_inspector()
        return self.population.inspector

    def test_stats_behaviour_and_loot_are_saved_per_placement(self):
        inspector = self.inspect()
        self.assertIn("DAT: 460", inspector.hints["health"].cget("text"))
        inspector.values["health"][1].set("999")
        inspector.values["aggressive"][1].set("no")
        inspector.loot_rows = [(3, ["47000ed7"]), (1, [])]
        inspector.apply()
        rules = self.rules()
        self.assertEqual(rules.stats[self.identity], {"health": "999"})
        self.assertEqual(rules.behaviour[self.identity], {"aggressive": "0"})
        self.assertEqual(rules.loot[self.identity], [(3, ["47000ed7"]), (1, [])])
        self.assertEqual(self.population.tree.item(self.identity, "values")[2], "999")
        inspector.scope.set("type")
        inspector.load_scope()
        self.assertEqual(inspector.values["health"][1].get(), "")
        inspector.values["level"][1].set("20")
        inspector.apply()
        self.assertEqual(self.rules().stats["47000001"], {"level": "20"})
        self.assertIn("999", self.rules().dumps())

    def test_patrol_capture_and_overlays(self):
        inspector = self.inspect()
        self.editor.preview()
        inspector.toggle_capture()
        self.population.patrol_capture(11, 2)
        self.population.patrol_capture(11, 12)
        inspector.finish_patrol()
        inspector.apply()
        self.assertEqual(self.rules().patrols[self.identity], (2000, [(10.0, 0.0), (10.0, 10.0)]))
        kinds = {shape[0] for shape in self.population.shapes(0x02990109)}
        self.assertEqual(kinds, {"circle", "path", "dot"})
        self.assertTrue(self.population.lines_3d(0x02990109))

    def test_add_spawn_point_groups_copy_and_delete_cleans_rules(self):
        inspector = self.inspect()
        inspector.add_spawn()
        rows = rows_from_text(read_document(self.path))
        self.assertEqual(len(rows), 2)
        group = self.rules().group_of(self.identity)
        self.assertEqual(group["members"], [self.identity, rows[1][0]])
        self.assertEqual(float(rows[1][4]), 4.0)
        inspector.capacity.set("2")
        inspector.group_respawn.set("45")
        inspector.save_group()
        self.assertEqual((self.rules().groups[0]["capacity"], self.rules().groups[0]["regeneration_ms"]), (2, 45000))
        inspector.close()
        self.editor.objects.selection_set(rows[1][0])
        self.editor.choose_position(None)
        with patch("content_editor.messagebox.askyesno", return_value=True):
            self.editor.delete_selected()
        self.assertEqual(self.rules().groups[0]["members"], [self.identity])
        self.assertEqual(self.rules().groups[0]["capacity"], 1)


    def base_world(self):
        base = self.folder / "base.tsv"
        base.write_text("pyrealium-world-v4\n"
                        "8140aaaa00000001\t47000001\t1f000001\t02990109\t5\t6\t3\t1\t0\t0\t0\t-\tOld Guard\n"
                        "8140aaaa00000002\t47000001\t1f000001\t7f7f0001\t5\t6\t3\t1\t0\t0\t0\t-\tFar Away\n")
        self.population.base_path.set(str(base))
        self.population.load_reference()
        return "8140aaaa00000001"

    def test_base_world_creatures_join_the_zone_scene_and_click_in_3d(self):
        identity = self.base_world()
        rows = self.population.scene_rows(0x02990109)
        self.assertEqual([row[0] for row in rows], [identity])
        self.assertEqual(self.population.scene_rows(0x02990109, enabled=False), [])
        self.editor.library.select(self.editor.page_creatures)
        hit = lambda object_id: (lambda event, roles: (5.0, 0x02990109, {"object_id": object_id}, (5, 6, 3)))
        view = SimpleNamespace(surface_hit=hit(int(identity, 16)))
        self.assertTrue(self.population.click_3d(view, SimpleNamespace(state=0)))
        self.assertEqual(self.population.selected, identity)
        self.assertIsNone(self.population.inspector)
        self.assertTrue(self.population.click_3d(view, SimpleNamespace(state=4)))
        self.assertEqual(self.population.inspector.row["name"], "Old Guard")
        ground = SimpleNamespace(surface_hit=lambda event, roles: (5.0, 0x02990109, {}, (1, 1, 1)))
        self.assertFalse(self.population.click_3d(ground, SimpleNamespace(state=0)), "empty ground keeps camera control")

    def test_patrol_waypoints_from_the_3d_ground_and_base_selection_in_view(self):
        identity = self.base_world()
        self.editor.active_scene_cell = 0x02990109
        # On the Creatures page a prepared creature opens its rules inspector.
        self.editor.library.select(self.editor.page_creatures)
        self.editor.select_in_view(int(identity, 16))
        inspector = self.population.inspector
        self.assertEqual(inspector.row["id"], identity)
        inspector.toggle_capture()
        view = SimpleNamespace(surface_hit=lambda event, roles: (5.0, 0x02990109, {}, (15, 6, 3)))
        self.assertTrue(self.population.click_3d(view, SimpleNamespace(state=0)))
        inspector.finish_patrol()
        inspector.apply()
        self.assertEqual(self.rules().patrols[identity], (2000, [(10.0, 0.0)]))


if __name__ == "__main__":
    unittest.main()
