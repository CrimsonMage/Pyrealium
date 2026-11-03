import copy
import struct
import unittest
from import_audited_population import identity, rejection, select_cells, localized_name
from import_observed_population import object_description, Unsupported


def actor(sha='00'*32,iid='8130000000000001',cell=0x01020001,x=5):
    return dict(source_sha256=sha,source_iid=iid,cell=cell,entity=0x47000001,visual=0x1f000001,
                visual_mask=2,visual_scale=None,mode=0x40000001,behaviors=[],velocity=None,
                events=[],position=[x,5,0],orientation=[1,0,0,0],scale=None)


class ImportTest(unittest.TestCase):
    def test_legacy_importer_also_rejects_initial_looted_or_combat_state(self):
        def packet(mode):
            return (struct.pack('<IQ6I',0x10058,0x8130000000000001,2,0x1f000023,0x8080,0x10001,mode,0x02990109)
                    +struct.pack('<7f',59,240,-44.5,1,0,0,0)+bytes(8)+struct.pack('<II',0x2000000,0x4700110f))
        self.assertEqual(object_description(packet(0x40000001))['entity'],'4700110f')
        for mode in (0x41000004,0x41000005,0x41000012):
            with self.assertRaises(Unsupported):object_description(packet(mode))

    def test_transient_or_moving_observations_do_not_become_spawns(self):
        self.assertIsNone(rejection(actor()))
        for change in [dict(mode=0x41000004),dict(mode=0x41000005),dict(mode=0x41000012),
                       dict(velocity=[0,1,0]),dict(behaviors=[{'id':1}]),dict(events=[{'kind':'moving'}]),
                       dict(position=[float('nan'),0,0]),dict(orientation=[0,0,0,0]),dict(visual_mask=3)]:
            a=actor();a.update(change);self.assertIsNotNone(rejection(a))
        a=actor();a['mode']=0x41000003;self.assertIsNone(rejection(a))

    def test_selects_one_snapshot_and_never_stacks_on_existing_cells(self):
        first=actor();second=actor('11'*32);third=actor('11'*32,'8130000000000002',x=10)
        self.assertEqual(select_cells([first,second,third],set()),[second,third])
        self.assertEqual(select_cells([first,second,third],{first['cell']}),[])
        self.assertEqual(select_cells([third,second,first],set()),[second,third])
        self.assertEqual(len(select_cells([first,copy.deepcopy(first)],set())),1)

    def test_ids_match_old_importer_and_do_not_publish_literal_names(self):
        a=actor();iid=identity(a['source_sha256'],a['source_iid'])
        self.assertEqual(int(iid,16)>>48,0x8140)
        self.assertNotEqual(iid,identity('01'*32,a['source_iid']))
        self.assertEqual(localized_name(None,{'literal':'Private named item','variables':0}),'-')

if __name__=='__main__':unittest.main()
