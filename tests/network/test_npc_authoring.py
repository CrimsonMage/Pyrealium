"""Identity binding regressions for wiki NPC conversation authoring."""
import importlib.util
from pathlib import Path
import unittest

path = Path(__file__).resolve().parents[2] / 'tools/author_npc_dialogue.py'
spec = importlib.util.spec_from_file_location('npc_authoring', path)
authoring = importlib.util.module_from_spec(spec)
spec.loader.exec_module(authoring)


class StarterNames(unittest.TestCase):
    def test_both_civil_war_contacts_receive_the_quest_topic(self):
        names = {'Gurog High Shaman', 'Gurog Supreme Warlord'}
        self.assertEqual(authoring.starter_names('Gurog Supreme Warlord, Gurog High Shaman', names), sorted(names))

    def test_generic_ancestor_does_not_absorb_named_ancestor(self):
        self.assertEqual(authoring.starter_names('Ancestor Juna', {'Ancestor', 'Ancestor Juna'}), ['Ancestor Juna'])

    def test_person_named_in_location_is_not_a_second_starter(self):
        self.assertEqual(authoring.starter_names("Leftenant Dell (32.8S, 52.3E) in Jethric's Outpost",
                         {'Leftenant Dell', 'Jethric'}), ['Leftenant Dell'])

    def test_matching_is_case_insensitive_and_boundary_checked(self):
        self.assertEqual(authoring.starter_names('ORVEN and Orvenish', {'Orven'}), ['Orven'])


if __name__ == '__main__':
    unittest.main()
