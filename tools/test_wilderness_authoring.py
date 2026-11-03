"""Regression cases from the archived guides, independent of native DAT access."""
import unittest
from author_wilderness_expansion import roster, habitat, compass, SPECIAL


class GuideParsing(unittest.TestCase):
    def test_prose_and_split_level_rosters_stop_before_unique_encounters(self):
        text='Monsters\nGolem\nCoral Golem, 50\nSilver Colossi, 60 group rare\nSkeleton Lieutenant\n- Lv 50\nSkeleton Lancer\nLv 55 - group\nUnique Creatures\nBaron Colier, 65 group'
        self.assertEqual(roster({'text':text}),[
            ('Coral Golem',50,False,False),('Silver Colossi',60,True,True),
            ('Skeleton Lieutenant',50,False,False),('Skeleton Lancer',55,True,False)])

    def test_specific_harvester_hunt_overrides_general_northern_olthoi_habitat(self):
        creature=dict(name='Olthoi Harvester',elite=False,level=51)
        self.assertTrue(habitat('Arramora',creature,compass(12,-51)))
        self.assertFalse(habitat('Arramora',creature,compass(24,-51)))
        creature['name']='Olthoi Mutilator'
        self.assertFalse(habitat('Arramora',creature,compass(12,-51)))
        self.assertTrue(habitat('Arramora',creature,compass(24,-51)))

    def test_common_kill_quest_monsters_are_not_unique_bosses(self):
        self.assertNotIn('Drudge Robber',SPECIAL)
        self.assertNotIn('Burun Keerik',SPECIAL)
        self.assertIn('Olthoi Soldier Grub',SPECIAL)


if __name__=='__main__': unittest.main()
