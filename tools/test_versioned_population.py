import unittest
from import_versioned_population import mappings, cohorts, manifest_row, runtime_role

def row(name='Beetle',ai='NPCTable_Beetle',role='monster'):
    return dict(name=name,ai=ai,role=role)

def actor(iid,sha='11'*32,cell=0x77b80001,time=0,x=5):
    return dict(source_sha256=sha,source_iid=iid,cell=cell,first_seen_seconds=time,entity=0x4700088c,
                position=[x,5,100],orientation=[1,0,0,0],scale=None,id='8140000000000001',
                visual=0x1f000031,name='Edge Beetle')

class VersionedPopulationTests(unittest.TestCase):
    def test_scripted_friendly_actors_are_not_generic_loot_monsters(self):
        for love in (24, 79):
            self.assertEqual(runtime_role(123, dict(role='monster', props={0x4100018a:love})), 'scripted')
        self.assertEqual(runtime_role(123, dict(role='monster', props={0x4100018a:22})), 'monster')
        self.assertEqual(runtime_role(123, dict(role='vendor', props={0x4100018a:24})), 'vendor')

    def test_cross_version_ids_require_unambiguous_name_ai_and_role(self):
        source={1:row(),2:row(ai='QuestBeetle'),3:row(role='scripted'),4:row(name='NPC')}
        target={5:row(),6:row(name='NPC'),7:row(name='NPC')}
        self.assertEqual(mappings(source,target),{1:5})
        target[8]=row()
        self.assertEqual(mappings(source,target),{})

    def test_one_capture_owns_a_whole_outdoor_block(self):
        a=actor('a');b=actor('b',sha='22'*32,cell=0x77b80002)
        c=actor('c',sha='22'*32,cell=0x77b80003)
        self.assertEqual(cohorts([a,b,c]),[b,c])
        a['cell']=0x77b80101;b['cell']=0x77b80102
        self.assertEqual(len(cohorts([a,b])),2)

    def test_later_respawns_and_duplicate_positions_are_not_stacked(self):
        a=actor('a');b=actor('b',time=200,x=20);c=actor('c',time=1)
        d=actor('d',time=2,x=10)
        self.assertEqual(cohorts([d,c,b,a]),[a,d])
        self.assertEqual(cohorts([a,b,c,d]),[a,d])

    def test_manifest_preserves_coordinates_and_native_scale(self):
        a=actor('a');a['position']=[1.25,8.75,-100.5];a['scale']=.8
        fields=manifest_row(a).split('\t')
        self.assertEqual(fields[4:7],['1.25','8.75','-100.5'])
        self.assertEqual(fields[11:13],['0.8','Edge Beetle'])

if __name__=='__main__':unittest.main()
