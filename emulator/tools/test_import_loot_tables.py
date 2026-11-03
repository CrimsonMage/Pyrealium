"""Evidence admission: complete observations, cross-version identity, quantities."""
import json, tempfile, unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from import_loot_tables import convert

def template(name, visual, ai=None):
    return dict(name=name,visual=visual,ai=ai,role='monster' if ai else 'scripted',
                props={0x4100018a:22,0x410001a6:411},signature=(1,2,3,4,5))

class LootImportTests(unittest.TestCase):
    def run_import(self, mutate=None, duplicate=False):
        source={1:template('Beetle',101,'BeetleAI'),2:template('Dagger',102)}
        target={11:template('Beetle',201,'BeetleAI'),12:template('Dagger',202)}
        corpse=dict(source_iid='corpse',corpse=True,container=None,visual=101,
                    name={'children':[{'key':1,'table':2}]},message_sha256='corpse-hash')
        item=dict(source_iid='item',corpse=False,container='corpse',entity=2,visual=102,
                  quantity=1,message_sha256='item-hash')
        if mutate:mutate(source,target,item)
        def templates(directory):
            return SimpleNamespace(close=lambda:None),SimpleNamespace(file=SimpleNamespace(close=lambda:None)),source if directory.name=='reference' else target
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);audit=root/'audit';audit.mkdir()
            for sha in ['a'*64]+(['b'*64] if duplicate else []):
                (audit/(sha+'.json')).write_text(json.dumps(dict(sha256=sha,objects=[corpse,item])))
            for label in ['reference','installed']:
                d=root/label;d.mkdir()
                for file in ['portal.dat','local_English.dat']:(d/file).write_bytes(b'synthetic test')
            with patch('import_loot_tables.templates',templates),patch('import_loot_tables.localized_name',lambda *_:'Beetle'):
                return convert(audit,root/'reference',root/'installed')

    def test_copied_capture_does_not_inflate_weight_and_uses_destination_visual(self):
        tables,items,report=self.run_import(duplicate=True)
        self.assertIn('0000000b\t411\t1\t0000000c',tables)
        self.assertIn('0000000c\t000000ca\tDagger',items)
        self.assertEqual(len(report['evidence']),1)

    def test_ambiguous_item_or_wrong_capture_visual_withholds_whole_bundle(self):
        for mutate in [lambda s,t,i:t.update({13:dict(t[12])}),
                       lambda s,t,i:i.update(visual=999),
                       lambda s,t,i:t[12].update(signature=(9,9,9,9,9))]:
            self.assertEqual(self.run_import(mutate)[2]['bundles'],0)

    def test_quantity_is_preserved_or_withheld_without_silent_truncation(self):
        tables,_,report=self.run_import(lambda s,t,i:i.update(quantity=3))
        self.assertIn('0000000c,0000000c,0000000c',tables)
        self.assertEqual(len(report['evidence'][0]['items']),3)
        self.assertEqual(self.run_import(lambda s,t,i:i.update(quantity=100000))[2]['bundles'],0)
        self.assertEqual(self.run_import(lambda s,t,i:i.update(quantity=0))[2]['bundles'],0)

    def test_friendly_scripted_actor_never_receives_generic_corpse_loot(self):
        self.assertEqual(self.run_import(lambda s,t,i:t[11]['props'].update({0x4100018a:24}))[2]['bundles'],0)

if __name__=='__main__':unittest.main()
