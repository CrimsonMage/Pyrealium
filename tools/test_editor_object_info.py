"""Selection data panel and ACE-style spawn group chances."""
from pathlib import Path
from types import SimpleNamespace
import unittest

from editor_object_info import ObjectData, render
from editor_rules import HEADER, Bestiary, Rules, spawn_shares

CONTENT = Path(__file__).resolve().parents[1] / "content"
DATS = Path(__file__).resolve().parents[2] / "dataset-7230"
A, B, C = "8140000000020001", "8140000000020002", "8140000000020003"


class ChanceRulesTest(unittest.TestCase):
    def test_member_chances_round_trip_and_share_a_roll(self):
        rules = Rules.parse(f"{HEADER}\ngroup\t1\t1\t60000\t{A},{B}:0.75,{C}:0.25\n")
        group = rules.groups[0]
        self.assertEqual(group["chances"], {B: 0.75, C: 0.25})
        self.assertEqual(Rules.parse(rules.dumps()).groups[0]["chances"], group["chances"])
        self.assertEqual(spawn_shares(group), {A: None, B: 0.75, C: 0.25})
        group["chances"] = {B: 0.5, C: 0.5 / 3}
        self.assertAlmostEqual(spawn_shares(group)[B], 0.75)

    def test_bad_chances_are_rejected(self):
        for chance in ("0", "1.5", "nan"):
            with self.assertRaises(ValueError):
                Rules.parse(f"{HEADER}\ngroup\t1\t1\t-\t{A}:{chance}\n")

    def test_removing_a_placement_drops_its_chance(self):
        rules = Rules.parse(f"{HEADER}\ngroup\t1\t1\t-\t{A}:0.5,{B}:0.5\n")
        rules.remove_placement(B)
        self.assertEqual(rules.groups[0]["chances"], {A: 0.5})


class ObjectDataTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.data = ObjectData(CONTENT)
        cls.bestiary = Bestiary(CONTENT)

    def editor(self, entity, rules=None, routes=None):
        rows = {}
        population = SimpleNamespace(bestiary=self.bestiary, rules=rules or Rules.parse(HEADER + "\n"),
                                     find=lambda m: rows.get(m), label=lambda r: r["name"])
        for member in (A, B):
            rows[member] = dict(id=member, entity=entity, name=f"Spawn {member[-1]}")
        return SimpleNamespace(placements=[[A, entity, "1f000000", "7d640013", "1", "2", "3", "1", "0", "0", "0"]],
                               prepared_rows=lambda: {}, population=population, portal_routes=routes or {},
                               atlas=SimpleNamespace(place_name=lambda cell, position: "Somewhere"))

    def test_a_creature_shows_its_combat_values_respawn_and_generator(self):
        entity = "470007a4"  # creature-combat.tsv: health 1900, damage 276, xp 11000
        rules = Rules.parse(f"{HEADER}\ngroup\t1\t1\t90000\t{A}:0.75,{B}:0.25\n")
        text = render(self.data.describe(self.editor(entity, rules), int(A, 16)))
        self.assertIn("Health 1900 (DAT)", text)
        self.assertIn("Damage 276", text)
        self.assertIn("Experience 11,000", text)
        self.assertIn("Respawn 90 s (spawn group)", text)
        self.assertIn("75.0% of a roll", text)
        self.assertIn("25.0% of a roll", text)

    @unittest.skipUnless((DATS / "portal.dat").exists(), "7230 DATs not present")
    def test_dat_values_are_read_from_the_entity_record_and_labelled(self):
        editor = self.editor("47001089")  # Relentless Lasher
        editor.game = SimpleNamespace(get=lambda: str(DATS))
        text = render(self.data.describe(editor, int(A, 16)))
        self.assertIn("FROM THE DAT\nportal.dat entity record 47001089", text)
        self.assertIn("Natural damage 72 ±40%", text)
        self.assertIn("Death experience 45", text)
        self.assertIn("Health 355 (DAT)", text)
        self.assertIn("every 1.5 s (server)", text)

    def test_a_portal_shows_its_destination(self):
        routes = {A: dict(destination=(0x58740039, [147.74, 15.13, 161.75]))}
        text = render(self.data.describe(self.editor("47001454", routes=routes), int(A, 16)))
        self.assertIn("Destination: Somewhere", text)
        self.assertIn("Cell 58740039", text)

    def test_quest_ties_and_dialogue_come_from_the_content_database(self):
        # A hunt's target: its Kill set advances the quest.
        text = render(self.data.describe(self.editor("47001027"), int(A, 16)))  # Blighted Mucor
        self.assertIn("Death advances Cull the Blighted Mucor", text)
        guard = render(self.data.describe(self.editor("4700157d"), int(A, 16)))  # the Shelter Guard's emote sets
        self.assertIn("DIALOGUE", guard)
        self.assertIn("Rescue the Shelter Hold", guard)


if __name__ == "__main__":
    unittest.main()
