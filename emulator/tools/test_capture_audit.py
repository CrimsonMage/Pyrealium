import copy
import json
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from audit_capture_corpus import audit
from test_capture_packets import pcap,frame,fragment
from test_import_audited_population import actor


class AuditTest(unittest.TestCase):
    def run_events(self, values):
        initial=actor();initial['type']='create'
        iid=int(initial['source_iid'],16)
        items=[]
        for i,v in enumerate(values):
            opcode=0x10086 if v['type']=='position' else 0x10058
            items.append(frame(fragment(struct.pack('<IQ',opcode,iid),blob=i+1)))
        raw=pcap(items)
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp)/'test.pcap';p.write_bytes(raw)
            source={'file':p,'sha256':'0'*64,'paths':['test.pcap'],'bytes':len(raw)}
            result=SimpleNamespace(stdout=b'\n'.join(json.dumps(v).encode() for v in values))
            with patch('audit_capture_corpus.subprocess.run',return_value=result):
                return audit(source,Path('decoder'))

    def test_quantized_idle_is_not_movement_but_changed_location_is(self):
        initial=actor();initial['type']='create'
        position=dict(type='position',source_iid=initial['source_iid'],cell=initial['cell'],
                      position=initial['position'],motion=[-1/255]*3)
        self.assertEqual(self.run_events([initial,position])['actors'][0]['events'],[])
        position['position']=[6,5,0]
        self.assertEqual(self.run_events([initial,position])['actors'][0]['events'][0]['kind'],'moving')

    def test_recreated_identity_at_another_location_is_not_a_stationary_spawn(self):
        initial=actor();initial['type']='create';later=copy.deepcopy(initial);later['position']=[8,5,0]
        result=self.run_events([initial,later])
        self.assertEqual(len(result['actors']),1)
        self.assertEqual(result['actors'][0]['events'][0]['kind'],'moving')

if __name__=='__main__':unittest.main()
