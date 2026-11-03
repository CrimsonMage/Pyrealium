"""Previously looted armor is repaired through real UDP use, then survives replay.

Only this isolated test store receives development grants. No player save is edited.
"""
import json, os, subprocess, time, uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until
from prepare_content import prepare

def main():
    run=ROOT/'Saved/AC2NetworkTests'/('recovered-equipment-'+uuid.uuid4().hex)
    run.mkdir(parents=True);content=run/'world.tsv';prepare(DAT_DIRECTORY,content)
    env=dict(os.environ)
    peers=[];server=None;report={};character=None
    def stop():
        if server and server.poll() is None:
            (run/'world/stop.request').write_text('stop\n');server.wait(timeout=15)
    def start():
        return subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19516','--account','local',
            '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
            '--initialize-ui','true','--training-kit','false','--test-map-travel','true','--seconds','150'],
            env=env,stdout=log,stderr=log,creationflags=0x08000000)
    def connect(create=False):
        p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19516,account='local');p.wait('characters')
        if create:
            p.send('create',name='Recovered Armor',race=1,sex=4096,start_area=0,physical=[[1,1],[2,1]])
            p.id=p.wait('created')['character']['id']
        else:p.id=character
        p.send('enter',id=p.id);p.wait('player');p.send('ready');p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory')
        return p
    def leave(p):
        mark=len(p.events);p.send('logout',id=p.id);until(p,mark,'selection');p.close()
    with (run/'server.log').open('w') as log:
        try:
            server=start();time.sleep(1);p=connect(True);character=p.id;leave(p);stop()
            for definition,visual,mask in [('47000fdc','1f00058c',0),('47001009','1f0005b6',0),
                                           ('47000fdb','1f00059e',32),('47000fd7','1f00059d',32)]:
                subprocess.run([str(SERVER),'inventory-grant','--account','local','--data',str(run/'world'),
                    '--character-id',character,'--game-data',str(DAT_DIRECTORY),'--id',definition,
                    '--visual-did',visual,'--equipment-mask',f'{mask:x}'],env=env,check=True,stdout=log,stderr=log)
            server=start();time.sleep(1);p=connect()
            boots=until(p,0,'object',lambda e:e.get('entity')==0x47000fdc and e.get('container')==p.id)['id']
            moccasins=until(p,0,'object',lambda e:e.get('entity')==0x47001009 and e.get('container')==p.id)['id']
            mark=len(p.events);p.send('use',item=boots,target='0')
            ack=until(p,mark,'inventory_result',lambda e:e['id']==boots)
            assert ack['status']==0 and ack['equipped']==16,ack
            bare=until(p,mark,'visual',lambda e:e['id']==p.id)['visual']
            mark=len(p.events);p.send('examine',id=boots)
            view=until(p,mark,'examination',lambda e:e['id']==boots)
            text='\n'.join(n.get('text',{}).get('literal') or '' for n in view['nodes'])
            assert 'Feet' in text and 'Human' in text and 'Armor rating unavailable.' in text,text
            time.sleep(.3);mark=len(p.events);p.send('use',item=moccasins,target='0')
            until(p,mark,'chat',lambda e:'requires a Tumerok' in str(e))
            report['old_loot_equips_with_native_appearance']=True;report['race_restriction_and_inspection']=text
            # Gloves already have the correct mask, but the native equipment
            # reply used to reject Hands (0x20) after all item checks succeeded.
            gloves=until(p,0,'object',lambda e:e.get('entity')==0x47000fdb and e.get('container')==p.id)['id']
            leather=until(p,0,'object',lambda e:e.get('entity')==0x47000fd7 and e.get('container')==p.id)['id']
            inventory=until(p,0,'inventory');bag=list(inventory['items'])
            bag[bag.index(boots)]=next(i['id'] for i in inventory['equipment'] if i['slot']==16)
            def equip(item):
                time.sleep(.15);mark=len(p.events)
                p.send('equip',id=item,location=32,slot=bag.index(item))
                ack=until(p,mark,'inventory_result',lambda e:e['id']==item)
                assert ack['status']==0 and ack['equipped']==32,ack
                return mark,until(p,mark,'visual',lambda e:e['id']==p.id)['visual']
            mark,worn=equip(gloves);bag.remove(gloves)
            assert worn!=bare,worn
            assert any(a['table']==0x2000004e and a['key']==16 and a['value']==1 for a in worn['aspects']),worn
            time.sleep(.15);mark=len(p.events)
            p.send('unequip',id=gloves,location=32,slot=len(bag))
            ack=until(p,mark,'inventory_result',lambda e:e['id']==gloves)
            assert ack['status']==0 and ack['equipped']==0,ack
            bag.insert(ack['slot'],gloves)
            assert until(p,mark,'visual',lambda e:e['id']==p.id)['visual']==bare
            mark,_=equip(leather);bag.remove(leather)
            source=bag.index(gloves);mark,restored=equip(gloves)
            displaced=until(p,mark,'inventory_result',lambda e:e['id']==leather)
            assert displaced['status']==0 and displaced['equipped']==0 and displaced['slot']==source,displaced
            assert restored==worn,(restored,worn)
            bag[source]=leather
            report['padded_gloves_equip_remove_and_replace']=True
            # The same cave in the user report has one recovered interior monster.
            rows=[l.split('\t') for l in content.read_text().splitlines() if l.startswith('8140f0f91f22d2f0\t')]
            assert len(rows)==1;f=rows[0];point=list(map(float,f[4:7]));point[0]+=2
            mark=len(p.events);p.send('move',cell=int(f[3],16),position=point,heading=0,motion=[0,0,0])
            mob=until(p,mark,'object',lambda e:e['id']==str(int(f[0],16)))
            assert mob['entity']==0x470016af and mob['placement']['cell']==0x7cb9013a,mob
            mark=len(p.events);p.send('examine',id=mob['id']);view=until(p,mark,'examination',lambda e:e['id']==mob['id'])
            assert 'Health:' in str(view),view
            report['cave_creature_received']=mob
            leave(p);stop();server=start();time.sleep(1);p=connect()
            bag=until(p,0,'inventory');assert {'id':boots,'slot':16} in bag['equipment'],bag
            assert {'id':gloves,'slot':32} in bag['equipment'] and leather in bag['items'],bag
            report['equipment_survives_server_restart']=True
            leave(p);(run/'result.json').write_text(json.dumps(report,indent=2))
            print('PASS: recovered footwear, gloves equip/remove/replace, race restriction, inspection, journal replay and cave population.',run)
        finally:
            for i,p in enumerate(peers):
                (run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            stop()

if __name__=='__main__':main()
