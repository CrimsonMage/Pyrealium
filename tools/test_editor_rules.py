import tempfile
import unittest
from pathlib import Path

from editor_rules import HEADER, Bestiary, Rules, load_rules, rules_path, save_rules

CONTENT = Path(__file__).resolve().parent.parent / "content"
SAMPLE = (HEADER + "\n# comment\n"
          "stats\t470007d1\t500\t40\t-\t20\t-\t-\t-\t-\t-\t-\n"
          "stats\t8140000000020001\t-\t60\t-\t-\t900\t-\t-\t-\t40\t5000\n"
          "ai\t8140000000020001\t25\t-\t1\t15\t0.5\t1\t-\t-\n"
          "loot\t8140000000020001\t70\t47000ed7,47000ee1\n"
          "loot\t8140000000020001\t30\t-\n"
          "patrol\t8140000000020001\t2000\t10,0;10,10;0,10\n"
          "group\t1\t2\t60000\t8140000000020001,8140000000020002,8140000000020003\n")


class RulesTests(unittest.TestCase):
    def test_round_trip_matches_server_sample(self):
        rules = Rules.parse(SAMPLE)
        again = Rules.parse(rules.dumps())
        self.assertEqual(again.stats, rules.stats)
        self.assertEqual(again.loot["8140000000020001"], [(70, ["47000ed7", "47000ee1"]), (30, [])])
        self.assertEqual(again.patrols["8140000000020001"], (2000, [(10, 0), (10, 10), (0, 10)]))
        self.assertEqual(again.group_of("8140000000020002")["capacity"], 2)

    def test_invalid_values_are_refused_before_saving(self):
        rules = Rules()
        for call in (lambda: rules.set_stats("470007d1", {"health": "0"}),
                     lambda: rules.set_loot("470007d1", [(0, [])]),
                     lambda: rules.set_loot("470007d1", [(5, ["1f000001"])]),
                     lambda: rules.set_patrol("470007d1", 0, [(1, 1)]),
                     lambda: rules.set_patrol("8140000000020001", 0, [(900, 1)])):
            with self.assertRaises(ValueError):
                call()
        for row in ("stats\t470007d1\t1\t1\t-\t-\t-\t-\t10\t-\t-\t-", "group\t1\t1\t-\t470007d1", "bogus\tx"):
            with self.assertRaises(ValueError):
                Rules.parse(HEADER + "\n" + row + "\n")

    def test_deleting_a_placement_removes_its_rules_and_membership(self):
        rules = Rules.parse(SAMPLE)
        rules.remove_placement("8140000000020001")
        self.assertNotIn("8140000000020001", rules.loot)
        self.assertNotIn("8140000000020001", rules.patrols)
        self.assertEqual(rules.groups[0]["members"], ["8140000000020002", "8140000000020003"])

    def test_save_is_conflict_checked_and_keeps_backup(self):
        import editor_content
        with tempfile.TemporaryDirectory() as folder:
            world = Path(folder) / "my-world.tsv"
            rules, text = load_rules(world)
            self.assertEqual(text, "pyrealium-world-rules-v1\n", "a new world has no rules")
            rules.set_stats("470007d1", {"health": "77"})
            text = save_rules(world, text, rules)
            with self.assertRaises(ValueError):
                save_rules(world, "pyrealium-world-rules-v1\n", rules)  # changed since it was loaded
            rules.set_stats("470007d1", {"health": "78"})
            save_rules(world, text, rules)
            # The rules are rows of the world's content database.
            self.assertIn("'0x470007d1',78", editor_content.content_path(world).read_text().replace(", ", ","))

    def test_bestiary_layers_native_type_and_placement_values(self):
        bestiary = Bestiary(CONTENT)
        entity = "470007a4"
        self.assertTrue(bestiary.is_creature(entity))
        rules = Rules()
        rules.set_stats(entity, {"health": "5"})
        rules.set_stats("8140000000020001", {"damage": "9"})
        stats, behaviour = bestiary.effective(rules, entity, "8140000000020001")
        self.assertEqual(stats["health"], ("5", "type"))
        self.assertEqual(stats["damage"], ("9", "placement"))
        self.assertEqual(stats["armor"][1], "native")
        self.assertEqual(bestiary.roaming(rules, entity, None)["aggro"], 30.0)
        rules.set_behaviour(entity, {"homesick": "12"})
        self.assertEqual(bestiary.roaming(rules, entity, None)["leash"], 12.0)


if __name__ == "__main__":
    unittest.main()
