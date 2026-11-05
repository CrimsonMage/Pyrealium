"""Tk workflow integration using isolated files and mocked DAT reads."""
import os
import tkinter as tk
import unittest
from concurrent.futures import Future
from unittest.mock import patch, MagicMock

from content_editor import Editor
from editor_document import read_document, rows_from_text
import test_editor_document as fixtures

REAL_REFRESH = Editor.refresh_scene


class EditorWorkflowTests(unittest.TestCase):
    def setUp(self):
        fixtures.DocumentTests.setUp(self)
        self.args.output = self.path
        self.args.game_data = self.folder
        self.env = patch.dict(os.environ, {"LOCALAPPDATA": str(self.folder)})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.refresh = patch.object(Editor, "refresh_scene")
        self.refresh.start()
        self.addCleanup(self.refresh.stop)
        self.root = tk.Tk()
        self.root.withdraw()
        self.editor = Editor(self.root, self.args)
        self.addCleanup(self.cleanup_editor)
        self.dialog = patch("content_editor.messagebox.showerror", side_effect=AssertionError("Unexpected error dialog"))
        self.dialog.start()
        self.addCleanup(self.dialog.stop)

    def cleanup_editor(self):
        self.editor.closing = True
        self.editor.view3d.close()
        self.editor.executor.shutdown(wait=True)
        for callback in self.root.tk.call("after", "info"):
            self.root.after_cancel(callback)
        self.root.destroy()

    def working(self):
        """The editor's working copy: edits reach the file only through Save content file."""
        return self.editor.original

    def test_edits_reach_the_file_only_on_save(self):
        editor = self.editor
        self.select()
        editor.fields["X"].set("45")
        editor.save()
        self.assertEqual(read_document(self.path), self.original)
        self.assertTrue(editor.unsaved())
        self.assertEqual(str(editor.write_button.cget("state")), "normal")
        # The 3D toolbar's Save button shows the unsaved state and saves.
        self.assertIn("Save changes", editor.view3d.save_button.cget("text"))
        with patch.object(editor.live, "changed") as pushed:
            editor.save_file()
        pushed.assert_called_once()
        self.assertEqual(rows_from_text(read_document(self.path))[0][4], "45")
        self.assertFalse(editor.unsaved())
        self.assertEqual(editor.view3d.save_button.cget("text"), "Saved")
        editor.undo()
        editor.view3d.save_button.invoke()   # the toolbar button itself
        self.assertFalse(editor.unsaved())
        self.assertEqual(read_document(self.path), self.original)
        editor.redo()
        editor.save_file()
        # Undo is a working-copy edit again, saved by the next Save.
        editor.undo()
        self.assertEqual(rows_from_text(read_document(self.path))[0][4], "45")
        editor.save_file()
        self.assertEqual(read_document(self.path), self.original)
        # Closing with unsaved edits asks; Cancel keeps the editor open.
        editor.redo()
        with patch("content_editor.messagebox.askyesnocancel", return_value=None):
            editor.close()
        self.assertFalse(editor.closing)

    def select(self):
        self.editor.objects.selection_set(self.row[:16])
        self.editor.choose_position(None)

    def test_edit_save_undo_redo_workflow(self):
        self.select()
        self.assertEqual(self.editor.fields["Heading"].get(), "64")
        self.editor.fields["X"].set("45")
        self.editor.save()
        rows = rows_from_text(self.working())
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][0], self.row[:16])
        self.assertEqual(rows[0][4], "45")
        self.assertEqual(rows[0][7:11], self.row.split("\t")[7:11])
        self.editor.undo()
        self.assertEqual(self.working(), self.original)
        self.editor.redo()
        self.assertEqual(rows_from_text(self.working())[0][4], "45")

    def test_duplicate_then_delete(self):
        self.select()
        self.editor.new_placement()
        self.assertIsNone(self.editor.selected_id)
        self.editor.save()
        self.assertEqual(len(rows_from_text(self.working())), 2)
        # The new placement is selected with the move handles, ready to adjust.
        self.assertIsNotNone(self.editor.selected_id)
        self.assertEqual(self.editor.view3d.tool.get(), "move")
        with patch("content_editor.messagebox.askyesno", return_value=True):
            self.editor.delete_selected()
        self.assertEqual(len(rows_from_text(self.working())), 1)
        self.editor.undo()
        self.assertEqual(len(rows_from_text(self.working())), 2)

    def test_search_names_and_cell_filter(self):
        self.editor.placement_search.set("Test creature")
        self.assertEqual(len(self.editor.objects.get_children()), 1)
        self.editor.placement_search.set("no matches")
        self.assertEqual(len(self.editor.objects.get_children()), 0)
        self.editor.placement_search.set("")
        self.editor.cell_only.set(True)
        self.editor.fields["Cell"].set("02990110")
        self.assertEqual(len(self.editor.objects.get_children()), 0)

    def test_cancel_reload_keeps_draft(self):
        self.select()
        self.editor.fields["X"].set("45")
        with patch("content_editor.messagebox.askyesno", return_value=False):
            self.editor.reload()
        self.assertEqual(self.editor.fields["X"].get(), "45")
        self.assertEqual(self.editor.selected_id, int(self.row[:16], 16))

    def test_delete_and_selection_changes_never_open_a_dialog(self):
        editor = self.editor
        self.select()
        editor.fields["X"].set("45")      # a form edit not applied
        with patch("content_editor.messagebox.askyesno", side_effect=AssertionError("dialog")):
            editor.clear_selection()
            self.assertIsNone(editor.selected_id)
            self.select()
            editor.viewport_command("delete")
        self.assertEqual(rows_from_text(editor.original), [])
        self.assertEqual(read_document(self.path), self.original, "nothing written before Ctrl+S")
        editor.undo()
        self.assertEqual(len(rows_from_text(editor.original)), 1)

    def test_world_load_without_selected_entity(self):
        self.editor.fields["Entity"].set("")
        self.editor.explore_cell = int(self.editor.fields["Cell"].get(), 16)
        REAL_REFRESH(self.editor)
        self.assertIsNotNone(self.editor.wanted)
        self.assertEqual(self.editor.wanted[0][2], 0)

    def test_startup_does_not_load_an_unrequested_dungeon(self):
        REAL_REFRESH(self.editor)
        self.assertIsNone(self.editor.wanted)

    def test_outdoor_arrival_keeps_eye_above_nearby_steep_ground(self):
        self.editor.frame_arrival([dict(terrain=True, mesh=dict(vertices=[(0, 0, 500), (30, 0, 600)]), position=(0, 0, 0))], (0, 0, 490))
        eye = self.editor.view3d.camera()[0]
        self.assertGreaterEqual(eye[2], 625)

    def test_world_map_is_initial_workspace_and_inspector_is_hidden(self):
        self.assertEqual(self.editor.workspace.select(), str(self.editor.atlas))
        self.assertFalse(self.editor.inspector_visible)

    def test_outdoor_cross_cell_keeps_reference_frame(self):
        editor = self.editor
        editor.view3d.set_tool("place")
        editor.outdoor_grid = {"block_side": 160, "cell_side": 20, "cells": 8}
        editor.active_scene_cell = 0x74ba0001
        editor.scene_ready = (editor.game.get(), editor.active_scene_cell, 0, (0, None, 0))
        editor.set_fields(dict(Cell="75ba0002", X="10", Y="15", Z="20"))
        with patch.object(editor.view3d, "update_placement") as moved:
            REAL_REFRESH(editor)
        self.assertEqual(editor.wanted[0][1], 0x74ba0001)
        self.assertEqual(editor.wanted[1], [170, 15, 20])
        moved.assert_called_once_with([170, 15, 20], 0, None)

    def test_scene_hit_maps_back_to_canonical_outdoor_cell(self):
        editor = self.editor
        editor.outdoor_grid = {"block_side": 160, "cell_side": 20, "cells": 8}
        editor.active_scene_cell = 0x74ba0001
        editor.pick_3d([170, 15, 20])
        self.assertEqual(editor.fields["Cell"].get(), "75ba0001")
        self.assertEqual(editor.fields["X"].get(), "10")
        self.assertEqual(editor.fields["Y"].get(), "15")

    def test_area_navigation_preserves_authored_placement(self):
        editor = self.editor
        self.select()
        before = editor.field_snapshot()
        future = Future()
        future.set_result([10, 20, 30])
        editor.navigation_future = future
        editor.navigation_loading = (editor.game.get(), 0x74ba0010, None, None)
        editor.navigation_wanted = editor.navigation_loading
        editor.finish_navigation()
        self.assertEqual(editor.field_snapshot(), before)
        self.assertEqual(editor.explore_cell, 0x74ba0010)
        self.assertEqual(editor.view3d.tool.get(), "explore")

    def test_camera_streaming_preserves_placement_and_translates_view(self):
        editor = self.editor
        before = editor.field_snapshot()
        editor.outdoor_grid = {"block_side": 160, "cell_side": 20, "cells": 8}
        editor.active_scene_cell = 0x74ba0001
        editor.view3d.focus = (165, 15, 30)
        with patch.object(editor.view3d, "get_navigation_point", return_value=(170, 15, 30)):
            editor.stream_camera_area()
        self.assertEqual(editor.field_snapshot(), before)
        self.assertEqual(editor.explore_cell, 0x75ba0001)
        self.assertEqual(editor.stream_camera[1], (5, 15, 30))
        REAL_REFRESH(editor)
        self.assertEqual(editor.wanted[0][1], 0x75ba0001)
        self.assertEqual(editor.wanted[0][2], 0)

    def test_position_map_follows_the_camera_over_terrain(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        editor = self.editor
        editor.outdoor_grid = {"block_side": 160, "cell_side": 20, "cells": 8}
        editor.active_scene_cell = 0x74ba0001
        editor.view3d.set_tool("explore")
        folder = Path(tempfile.mkdtemp(dir=self.folder))
        (folder / "blocks").mkdir()
        tile = tk.PhotoImage(master=self.root, width=64, height=64)
        tile.put("#79a039", to=(0, 0, 64, 64))
        tile.write(str(folder / "blocks" / "75ba.png"), format="png")
        editor.atlas.export = dict(folder=folder, blocks={"75ba"}, notes=[])
        canvas = editor.canvas
        with patch.object(editor.view3d, "get_navigation_point", return_value=(170, 15, 30)), \
                patch.object(canvas, "winfo_width", return_value=600), patch.object(canvas, "winfo_height", return_value=400):
            # The camera has crossed east into the next landblock's first cell.
            self.assertEqual(editor.camera_cell(), (0x75ba0001, (10, 15)))
            editor.preview()
            texts = [canvas.itemcget(item, "text") for item in canvas.find_all() if canvas.type(item) == "text"]
            self.assertTrue(any(text.startswith("Landblock 75BA · cell 75BA0001") and text.endswith("camera") for text in texts), texts)
            self.assertTrue(any(canvas.type(item) == "image" for item in canvas.find_all()), "terrain tile drawn")
            self.assertTrue(editor.map_tiles)
            # Zooming keeps the camera's block; a click places the draft in canonical cell terms.
            span = editor.map_view[3]
            editor.zoom_step(1)
            self.assertLess(editor.map_view[3], span)
            x, y, scale, width, height = editor.transform
            editor.pick_position(SimpleNamespace(x=width / 2 + (25 - x) * scale, y=height / 2 - (35 - y) * scale))
        self.assertEqual(editor.fields["Cell"].get(), "75ba000a")
        self.assertAlmostEqual(float(editor.fields["X"].get()), 25, places=3)
        self.assertAlmostEqual(float(editor.fields["Y"].get()), 35, places=3)

    def test_prepared_world_objects_are_moved_through_overrides(self):
        from editor_population import read_reference
        editor = self.editor
        base_file = self.folder / "prepared.tsv"
        base_file.write_text("pyrealium-world-v4\n"
                             "8140000000000009\t47000001\t1f000001\t02990109\t5\t6\t7\t1\t0\t0\t0\t-\tGuard\n"
                             "814000000000000a\t47000222\t1f000333\t02990109\t1\t1\t1\t1\t0\t0\t0\t-\t-\n", encoding="utf-8")
        editor.population.reference = read_reference(base_file)
        # Selecting the prepared object copies it into the working copy under its own id.
        editor.select_in_view(0x8140000000000009)
        self.assertEqual(editor.selected_id, 0x8140000000000009)
        self.assertEqual(rows_from_text(editor.original)[-1][:7], ["8140000000000009", "47000001", "1f000001", "02990109", "5", "6", "7"])
        self.assertEqual(editor.delete_button.cget("text"), "Revert to prepared world")
        # Saving an unchanged override writes nothing for it.
        editor.save_file()
        self.assertNotIn("8140000000000009", read_document(self.path))
        # Moved, it is saved as an override with the prepared identity.
        editor.select_in_view(0x8140000000000009)
        editor.fields["X"].set("9")
        editor.save()
        editor.save_file()
        saved = [row for row in rows_from_text(read_document(self.path)) if row[0] == "8140000000000009"]
        self.assertEqual(saved[0][4], "9")
        # Clearing the selection keeps the working copy.
        editor.clear_selection()
        self.assertIsNone(editor.selected_id)
        self.assertEqual(editor.view3d.tool.get(), "explore")
        editor.select_in_view(0x8140000000000009)
        # Deleting the override reverts the object to the prepared world.
        with patch("content_editor.messagebox.askyesno", return_value=True):
            editor.delete_selected()
        editor.save_file()
        self.assertNotIn("8140000000000009", read_document(self.path))
        # A prepared object whose entity the catalog lacks keeps its own visual.
        editor.select_in_view(0x814000000000000a)
        editor.fields["Y"].set("4")
        editor.save()
        editor.save_file()
        saved = [row for row in rows_from_text(read_document(self.path)) if row[0] == "814000000000000a"]
        self.assertEqual(saved[0][1:3], ["47000222", "1f000333"])

    def test_selecting_and_moving_keep_the_loaded_scene(self):
        from editor_population import read_reference
        editor = self.editor
        base_file = self.folder / "prepared.tsv"
        base_file.write_text("pyrealium-world-v4\n"
                             "8140000000000009\t47000001\t1f000001\t02990109\t5\t6\t7\t1\t0\t0\t0\t-\tGuard\n", encoding="utf-8")
        editor.population.reference = read_reference(base_file)
        editor.active_scene_cell = 0x02990109
        revision = editor.content_revision
        with patch.object(editor.view3d, "place_rows") as placed:
            editor.select_in_view(0x8140000000000009)   # the override copy changes nothing drawn
            self.assertEqual(editor.content_revision, revision)
            placed.assert_not_called()
            editor.fields["X"].set("9")
            editor.save()                               # a move: in place
            self.assertEqual(editor.content_revision, revision)
            moved = placed.call_args.args
            self.assertEqual((moved[0][0][4], moved[1][0][4]), ("9", "5"))
            editor.undo()                               # back: in place again
            self.assertEqual(editor.content_revision, revision)
            self.assertEqual(placed.call_args.args[0][0][4], "5")
        # Adding a new object loads its model: a rebuild.
        editor.new_placement()
        editor.fields["X"].set("20")
        editor.save()
        self.assertGreater(editor.content_revision, revision)

    def test_navigation_bar_names_the_place_and_facing(self):
        from editor_hero_map import map_units, world_point
        editor = self.editor
        self.assertEqual(editor.navigation_text(), ("Open an area from the World map.", None))
        editor.outdoor_grid = {"block_side": 160, "cell_side": 20, "cells": 8}
        editor.active_scene_cell = 0x74ba0001
        # The camera stands 10 m into the next block east, facing east.
        with patch.object(editor.view3d, "camera", return_value=((170, 15, 30), (1, 0, 0), (0, -1, 0), (0, 0, 1))):
            text, bearing = editor.navigation_text()
        self.assertEqual(round(bearing), 90)
        self.assertIn("landblock 75BA  ·  cell 75BA0001", text)
        self.assertIn(map_units(*world_point(0x75ba0001, 10, 15)), text)
        self.assertTrue(text.endswith("facing  90° E"), text)
        with patch.object(editor.view3d, "camera", return_value=((5, 5, 1), (-0.7071, -0.7071, 0), (0, 0, 0), (0, 0, 1))):
            self.assertTrue(editor.navigation_text()[0].endswith("facing 225° SW"))
        with patch.object(editor.view3d, "camera", return_value=((5, 5, 1), (-0.004, 1, 0), (0, 0, 0), (0, 0, 1))):
            self.assertTrue(editor.navigation_text()[0].endswith("facing   0° N"))
        editor.active_scene_cell = 0x02990109
        with patch.object(editor.view3d, "camera", return_value=((1, 2, 3), (0, 1, 0), (1, 0, 0), (0, 0, 1))):
            self.assertTrue(editor.navigation_text()[0].startswith("Dungeon room 02990109"))
        editor.view3d.set_navigation("here", 90)
        self.assertEqual(editor.view3d.nav_text.get(), "here")
        self.assertTrue(editor.view3d.compass.find_all())

    def test_a_portal_with_a_route_takes_the_camera_through(self):
        editor = self.editor
        editor.portal_routes = {"8140000000030031": dict(id="8140000000030031", destination=(0x58740039, [147.74, 15.13, 161.75]))}
        with patch.object(editor, "navigate_to") as travel:
            editor.use_object(0x8140000000030031)
            travel.assert_called_once_with(0x58740039, [147.74, 15.13, 161.75])
            editor.use_object(0x8140000000099999)
            travel.assert_called_once()
        self.assertIn("no route", editor.status.get())

    def test_entity_search_takes_ids_as_copied(self):
        editor = self.editor
        for typed in ("47000001", "0x47000001", "0X47000001", "test creature"):
            editor.search.set(typed)
            values = [editor.entities.item(iid, "values") for iid in editor.entities.get_children()]
            self.assertEqual([v[0] for v in values], ["47000001"], typed)
        # An entity the DATs hold but the catalog lacks offers to choose its visual.
        with patch("ac2_dat.Dat", MagicMock()):
            editor.search.set("0x470012f5")
        self.assertEqual(editor.entities.get_children(), ("uncatalogued",))
        with patch("ac2_dat.Dat", MagicMock(side_effect=KeyError)):
            editor.search.set("470012f6")
        self.assertEqual(editor.entities.get_children(), ())

    def test_uncatalogued_entity_places_with_its_chosen_visual(self):
        editor = self.editor
        editor.extra_definitions["470012f5"] = ["1f000627", "Ancestor Head (Darkstone)"]
        editor.load_catalog()
        self.select()
        editor.new_placement()
        editor.fields["Entity"].set("470012f5")
        self.assertEqual(editor.selection_name.get(), "Ancestor Head (Darkstone)")
        editor.save()
        rows = rows_from_text(self.working())
        self.assertEqual(rows[-1][1:3], ["470012f5", "1f000627"])

    def test_keep_adding_allocates_distinct_objects(self):
        self.select()
        self.editor.new_placement()
        self.editor.keep_adding.set(True)
        self.editor.save()
        self.assertIsNone(self.editor.selected_id)
        self.editor.fields["X"].set("5")
        self.editor.save()
        rows = rows_from_text(self.working())
        self.assertEqual(len(rows), 3)
        self.assertEqual(len({row[0] for row in rows}), 3)

    def test_exploring_linked_room_streams_without_moving_object(self):
        editor = self.editor
        before = editor.field_snapshot()
        editor.active_scene_cell = 0x02990109
        editor.nearby_locations = [dict(cell=0x0299011b)]
        with patch("editor_locations.room_below", return_value=0x0299011b):
            editor.stream_camera_area()
        self.assertEqual(editor.field_snapshot(), before)
        self.assertEqual(editor.explore_cell, 0x0299011b)
        self.assertEqual(editor.stream_camera[0], 0x0299011b)

    def seed_group(self):
        second = self.row.split("\t")
        second[0], second[4] = "8140000000020001", "3"
        import editor_content
        editor_content.write_world(self.path, self.original + "\t".join(second) + "\n")
        self.editor.reload(force=True)
        self.editor.select_many({0x8140000000020000, 0x8140000000020001})

    def test_multi_selection_disables_single_object_fields(self):
        self.seed_group()
        self.assertEqual(len(self.editor.group_ids), 2)
        self.assertEqual(self.editor.view3d.tool.get(), "select")
        self.assertEqual(str(self.editor.transform_entries["Entity"].cget("state")), "disabled")
        self.assertEqual(self.editor.save_button.cget("text"), "Arrange selected objects")

    def test_group_arrangement_commits_one_undoable_edit(self):
        self.seed_group()
        before = self.working()
        with patch.object(tk.Toplevel, "grab_set"):
            self.editor.open_arrange()
        dialog = self.editor.arrange_dialog
        self.assertIsNotNone(dialog)
        dialog.fields["Offset X"].set("5")
        dialog.preview()
        self.assertEqual(self.working(), before)
        with patch("editor_arrange.Dat", MagicMock()):
            dialog.apply()
        self.assertIsNone(self.editor.arrange_dialog)
        rows = rows_from_text(self.working())
        self.assertEqual([r[4] for r in rows], ["6", "8"])
        self.assertEqual(len(self.editor.history), 1)
        self.editor.undo()
        self.assertEqual(self.working(), before)

    def test_group_delete_undo_restores_entire_selection(self):
        self.seed_group()
        before = self.working()
        with patch("content_editor.messagebox.askyesno", return_value=True):
            self.editor.delete_selected()
        self.assertEqual(rows_from_text(self.working()), [])
        self.editor.undo()
        self.assertEqual(self.working(), before)

    def test_direct_drag_commits_once_and_undo_restores_object(self):
        self.select()
        editor = self.editor
        editor.view3d.set_tool("move")
        self.assertTrue(editor.object_tools.transform("move", {}, "begin"))
        editor.object_tools.transform("move", dict(translation=(5, 0, 0)), "preview")
        self.assertEqual(self.working(), self.original)
        with patch("editor_arrange.Dat", MagicMock()):
            editor.object_tools.transform("move", {}, "commit")
        self.assertEqual(rows_from_text(self.working())[0][4], "6")
        self.assertEqual(len(editor.history), 1)
        editor.undo()
        self.assertEqual(self.working(), self.original)

    def test_cancel_drag_restores_fields_and_leaves_file_untouched(self):
        self.select()
        editor = self.editor
        before = editor.field_snapshot()
        editor.object_tools.transform("rotate", {}, "begin")
        editor.object_tools.transform("rotate", dict(degrees=90), "preview")
        editor.object_tools.transform("rotate", {}, "cancel")
        self.assertEqual(editor.field_snapshot(), before)
        self.assertEqual(self.working(), self.original)
        self.assertEqual(editor.history, [])
