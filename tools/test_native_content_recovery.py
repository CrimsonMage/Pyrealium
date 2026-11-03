"""Installed-DAT regression vectors for the read-only quest recovery tool."""
import json
import shutil
import os
from pathlib import Path
import subprocess
import unittest

ROOT=Path(__file__).resolve().parents[1]

@unittest.skipUnless(os.environ.get('AC2_TEST_GAME_DATA'),'requires installed retail DATs')
class NativeQuestRecovery(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        probe=os.environ.get('AC2_RETAIL_PROBE') or shutil.which('ac2-retail-probe') or 'ac2-retail-probe'
        cls.data=json.loads(subprocess.check_output([str(probe),os.environ['AC2_TEST_GAME_DATA'],'--content'],encoding='utf-8'))
        cls.quests={q['package']:q for q in cls.data['quests']}

    def test_native_incursion_phase_order_and_effect_identity(self):
        q=self.quests['QuestKillGreenWaspStriker']
        self.assertEqual(q['id'],0x40000097)
        self.assertEqual(q['name']['text'],'Incursion')
        self.assertEqual([p['phase'] for p in q['phases']],[1,2,3,4,5])
        self.assertIn('Kill 5 Green Phyntos Wasps.',q['phases'][0]['journal']['text'])
        self.assertIn('Kill 1 Green Phyntos Wasp.',q['phases'][-1]['journal']['text'])
        effect=next(e for e in self.data['quest_effects'] if e['package']=='Eff_IncrementQuestKillGreenWaspStriker')
        self.assertEqual(effect['quest'],q['id'])
        self.assertEqual(effect['quest_status'],0x60000000)

    def test_stripped_names_are_not_treated_as_absent_bytecode(self):
        script=next(s for s in self.data['scripts'] if s['name']=='QuestMucorMushrooms')
        self.assertEqual(script['functions'],[])
        self.assertTrue(any(b['kind']=='retained' for b in script['own_bodies']))
        self.assertTrue(any(b['kind']=='trap' for b in script['own_bodies']))
        # This old event's local table is empty. Do not fabricate player text.
        self.assertIsNone(self.quests['QuestMucorMushrooms']['name']['text'])

    def test_phase_links_are_quest_ids_not_executable_callbacks(self):
        phase=self.quests['QuestBrothOfTanacha']['phases'][0]
        self.assertEqual(phase['related_quests'],[0x40000024,0x40000027,0x40000025,0x40000023,0x40000026])
        self.assertEqual(self.quests['QuestBrothOfTanacha']['phases'][1]['related_quests'],[])

if __name__=='__main__':unittest.main()
