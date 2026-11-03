import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from recover_quest_text import resolve_missing
from recover_reference_tables import (article_title, parse_article, read_wiki,
    location_hints, read_sql_corpus, extract_incursions, duration_minutes, native_quest_links)


class QuestTextRecovery(unittest.TestCase):
    def setUp(self):
        self.field={'table':0x25000001,'key':7,'text':None,'error':'MissingPrivateTable','variables':'[]'}
        self.installed={0x25000001:b'parent'}
        self.archived={0x25000001:b'parent',0x25020001:b'private'}
        self.decoded={0x25000001:{'entries':{'00000007':{'private_table':0x25020001,'text':'','variables':[]}}},
                      0x25020001:{'entries':{'00000007':{'private_table':0,'text':'Native journal[E]','variables':[]}}}}

    def resolve(self):
        return resolve_missing(self.field,self.installed.get,self.archived.get,self.decoded.get)

    def test_missing_private_text_keeps_exact_grammar_and_provenance(self):
        result,reason=self.resolve()
        self.assertIsNone(reason)
        self.assertEqual(result['text_raw'],'Native journal[E]')
        self.assertEqual([r['installed_record_identical'] for r in result['records']],[True,False])
        self.assertEqual(len(result['records'][1]['sha256']),64)

    def test_existing_text_is_never_replaced(self):
        self.field['text']='Current translation'
        self.assertEqual(self.resolve(),(None,'not_missing_private_table'))

    def test_changed_parent_is_rejected(self):
        self.installed[0x25000001]=b'different version'
        self.assertEqual(self.resolve(),(None,'record_version_mismatch'))

    def test_redirect_cycle_and_key_mismatch_fail_closed(self):
        self.decoded[0x25020001]['entries']['00000007']['private_table']=0x25000001
        self.assertEqual(self.resolve(),(None,'redirect_cycle'))
        self.decoded[0x25020001]['entries']={}
        self.assertEqual(self.resolve(),(None,'key_missing'))

    def test_variable_strings_need_formatter(self):
        self.decoded[0x25020001]['entries']['00000007']['variables']=[1]
        self.assertEqual(self.resolve(),(None,'requires_string_variables'))

    def test_missing_archive_record_and_empty_text_are_not_invented(self):
        self.archived.pop(0x25020001)
        self.assertEqual(self.resolve(),(None,'archived_record_missing'))
        self.archived[0x25020001]=b'private'
        self.decoded[0x25020001]['entries']['00000007']['text']=''
        self.assertEqual(self.resolve(),(None,'empty_archived_text'))


class ReferenceTables(unittest.TestCase):
    def test_guide_coordinates_reject_zero_placeholders_and_wrong_axes(self):
        self.assertEqual(location_hints({'locN':'0.0N','locE':'0.0W'}),[])
        self.assertEqual(location_hints({'locN':'2.5W','locE':'3.0N'}),[])
        hints=location_hints({'loc1N':'36.0N','loc1E':'3.7E','loc2N':'0.0N','loc2E':'0.0W'})
        self.assertEqual(len(hints),1)
        self.assertEqual(hints[0]['north_south'],'36.0N')
        self.assertNotIn('position',hints[0])

    def test_deduplicate_sql_bytes_not_names_and_exclude_private_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            text="INSERT INTO npc (`name`, ` locN`, ` locE`) VALUES ('Orven','44.9N','7.0W'); INSERT INTO useraccount VALUES ('secret');"
            (root/'a.sql').write_text(text)
            (root/'b.sql').write_text(text)
            sources,errors,count=read_sql_corpus([root])
            self.assertEqual((len(sources),count,errors),(1,2,[]))
            self.assertEqual(len(sources[0]['paths']),2)
            self.assertNotIn('secret',json.dumps(sources))

    def test_namespace_filter_preserves_incursions_but_ignores_talk_and_revisions(self):
        self.assertEqual(article_title('http://ac2wiki.info/index.php/Incursion:_Green_Phyntos_Wasp'),'Incursion: Green Phyntos Wasp')
        for url in ['http://ac2wiki.info/index.php/File_talk:X','http://ac2wiki.info/index.php/User:Test',
                    'http://ac2wiki.info/index.php/Broth?oldid=1','http://other.invalid/index.php/Broth']:
            self.assertIsNone(article_title(url))

    def test_html_text_tables_and_links_ignore_script_and_navigation(self):
        raw=b'''<html><nav>wrong</nav><div id="mw-content-text">
        <script>do_not_execute()</script><b>Quest Starter</b>: <a href="/index.php/Kanten">Kanten</a><br>
        <b>Repeat Time</b>: 2 hours<h2>Walkthrough</h2><p>Give the bag to Orven.</p>
        <table><tr><th>Item</th><th>Count</th></tr><tr><td>A<br>B</td><td>2</td></tr></table></div></html>'''
        a=parse_article('http://ac2wiki.info/index.php/Test',raw)
        self.assertEqual(a['facts'],{'Quest Starter':'Kanten','Repeat Time':'2 hours'})
        self.assertEqual(a['tables'][0][1][0]['text'],'A B')
        self.assertEqual(a['links'][0]['url'],'http://ac2wiki.info/index.php/Kanten')
        self.assertNotIn('do_not_execute',a['text'])
        self.assertNotIn('wrong',a['text'])

    def test_missing_wiki_article_is_not_evidence(self):
        self.assertIsNone(parse_article('http://ac2wiki.info/index.php/Missing',b'<div id="mw-content-text"><div class="noarticletext">missing</div></div>'))

    def test_cache_reads_url_named_members_without_extracting_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'cache.zip'
            with zipfile.ZipFile(path,'w') as z:
                z.writestr('http://ac2wiki.info/index.php/Test','<div id="mw-content-text">quest text</div>')
                z.writestr('../../outside','ignored')
            articles,errors=read_wiki(path)
            self.assertEqual((len(articles),errors),(1,[]))
            self.assertEqual(list(Path(tmp).iterdir()),[path])

    def test_conflicting_repeat_timers_are_reported_not_chosen(self):
        def cell(text,span='1'):return {'text':text,'rowspan':'1','colspan':span}
        index={'title':'Incursion','url':'index','html_sha256':'a','tables':[
            [list(map(cell,['Quest Name','Quest Starter','Quest Target','Repeat'])),
             list(map(cell,['Incursion: Wasps','Striker','5 Wasps','20 hours'])),
             [cell('bad','4')]]]}
        detail={'title':'Incursion: Wasps','url':'detail','html_sha256':'b','facts':{'Repeat Time':'5 minutes'}}
        rows,conflicts,rejected=extract_incursions([index,detail])
        self.assertEqual(len(rows),1)
        self.assertEqual(rows[0]['values']['Quest Target'],'5 Wasps')
        self.assertNotIn('spawn_count',rows[0])
        self.assertEqual((conflicts[0]['index']['value'],conflicts[0]['detail']['value']),(1200,5))
        self.assertEqual(len(rejected),1)
        self.assertIsNone(duration_minutes('Unknown'))

    def test_recovered_quest_title_is_only_a_candidate_and_generic_names_do_not_join(self):
        quests={'quests':[{'id':1,'package':'OldQuest','name':{'text':None}},
                          {'id':2,'package':'GenericQuest','name':{'text':'Incursion'}}]}
        recovered={'candidates':[{'quest':'00000001','field':'name','text_raw':'Broth of Tanacha[E]'}]}
        articles=[{'title':'Broth of Tanacha','url':'broth','html_sha256':'a'},
                  {'title':'Incursion','url':'incursion','html_sha256':'b'}]
        links=native_quest_links(quests,recovered,articles)
        self.assertEqual(len(links),1)
        self.assertEqual(links[0]['name_source'],'archived_private_string')
        self.assertEqual(links[0]['name_raw'],'Broth of Tanacha[E]')
        self.assertEqual(links[0]['article_candidates'][0]['url'],'broth')


if __name__=='__main__':unittest.main()
