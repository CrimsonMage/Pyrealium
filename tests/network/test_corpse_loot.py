"""Real UDP: owned encounter corpse, native container UI messages, take/all, replay.

Uses captured Cutter Beetle loot with isolated test placements/combat balance.
"""
import json,os,subprocess,time,uuid
from test_network import Peer,ROOT,SERVER,DAT_DIRECTORY,prepare_test_content
from test_gameplay import until

def main():
    data=ROOT/'Saved/AC2NetworkTests'/('corpse-loot-'+uuid.uuid4().hex);data.mkdir(parents=True)
    content=prepare_test_content(data);log=(data/'server.log').open('w')
    # Replace only test Idols with a native creature whose corpse loot was
    # actually observed. Production starter placements remain unchanged.
    content.write_text(content.read_text().replace('470007d1\t1f0003d1','470008bf\t1f000031'))
    # A deterministic encounter in the real starter cell keeps this test bounded.
    rules=data/'encounters.tsv'
    rules.write_text('pyrealium-encounters-v1\nplayer\t100\t1\t1000\t30\nregen\t0\t0\t1000\nnpc\t470008bf\t10\t1\t3000\t3\t60000\t5\t1\n')
    args=[str(SERVER),'serve','--bind','127.0.0.1:19394','--account','local','--data',str(data/'world'),
        '--game-data',str(DAT_DIRECTORY),'--content',str(content),'--initialize-ui','true','--training-kit','false',
        '--encounters',str(rules),'--seconds','120']
    peers=[]
    def start():
        s=subprocess.Popen(args,env=dict(os.environ,AC2_TEST_LOOT_SEED='5'),stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(1);assert s.poll() is None;return s
    def login(name=None,identity=None):
        p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19394,account='local');p.wait('characters')
        if name:
            p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
            identity=p.wait('created')['character']['id']
        p.id=identity;p.send('enter',id=identity);p.wait('player')
        p.spawn=p.wait('object',lambda e:e['id']==identity);p.send('ready');p.wait('status',lambda e:e['phase']=='world')
        until(p,0,'inventory');return p
    def stop(s):
        for p in peers:p.close()
        peers.clear();(data/'world/stop.request').write_text('stop\n');s.wait(timeout=10)
    server=start()
    try:
        a,b=login('Loot Owner'),login('Loot Observer');aid=a.id
        mobs=[e for e in a.events if e.get('event')=='object' and e.get('entity')==0x470008bf]
        owned=[]
        for n,mob in enumerate(mobs[:2]):
            point=mob['placement'];xyz=point['position'][:];xyz[0]-=1.5
            for p in [a,b]:p.send('move',cell=point['cell'],position=xyz,heading=0,motion=[0,0,0])
            time.sleep(.2);mark=len(a.events);a.send('attack',id=mob['id'])
            until(a,mark,'quality',lambda e:e.get('id')==mob['id'] and e.get('key')==0x102 and e['value']==0)
            renamed=until(a,mark,'quality',lambda e:e.get('id')==mob['id'] and e.get('table')==10 and e.get('key')==0x100)
            assert renamed['value']['key']==0x509a8ff6 and renamed['value']['variables'][0]['key']==0xe4ea98e6,renamed
            until(b,0,'quality',lambda e:e.get('id')==mob['id'] and e.get('table')==10 and e.get('key')==0x100)
            if n==0:
                # A player entering after death receives the same composed name,
                # without relying on having observed the killing blow.
                bm=len(b.events);b.send('logout',id=b.id);until(b,bm,'selection')
                bm=len(b.events);b.send('enter',id=b.id);until(b,bm,'player');b.send('ready')
                late=until(b,bm,'quality',lambda e:e.get('id')==mob['id'] and e.get('table')==10 and e.get('key')==0x100)
                assert late['value']==renamed['value'],(late,renamed)
            a.send('examine',id=mob['id'])
            corpse=until(a,mark,'examination',lambda e:e['id']==mob['id'])
            assert any(node.get('text',{}).get('key')==0x509a8ff6 for node in corpse['nodes']),corpse
            a.send('combat',enabled=False)
            until(a,mark,'combat',lambda e:not e['enabled'])
            # Other players cannot open or steal this reward, even beside it.
            bm=len(b.events);b.send('open_container',id=mob['id']);until(b,bm,'chat',lambda e:'Loot rejected' in str(e))
            # Range is rechecked on take, even after a successful open.
            far=xyz[:];far[0]-=10
            a.send('move',cell=point['cell'],position=far,heading=0,motion=[0,0,0]);time.sleep(.2)
            fm=len(a.events);a.send('loot',id=mob['id'])
            until(a,fm,'chat',lambda e:'four meters' in str(e))
            a.send('move',cell=point['cell'],position=xyz,heading=0,motion=[0,0,0]);time.sleep(.2)
            a.send('use',item=mob['id'],target='0')
            opened=until(a,mark,'container_state',lambda e:e['id']==mob['id'] and e['open'])
            assert opened['status']==0 and len(opened['items'])>=1,opened
            temp=opened['items'][0]
            item=until(a,mark,'object',lambda e:e['id']==temp)
            assert item['container']==mob['id'] and item['entity']!=0
            time.sleep(.3);im=len(a.events);a.send('examine',id=temp)
            inspected=until(a,im,'examination',lambda e:e['id']==temp)
            assert any('In the loot container' in (node.get('text',{}).get('literal') or '') for node in inspected['nodes']),inspected
            bm=len(b.events);b.send('examine',id=temp)
            until(b,bm,'chat',lambda e:'not available to examine' in str(e))
            # Closing and reopening keeps the pending item, without granting it.
            mark=len(a.events);a.send('close_container',id=mob['id'])
            until(a,mark,'container_state',lambda e:not e['open'])
            mark=len(a.events);a.send('open_container',id=mob['id'])
            assert until(a,mark,'container_state',lambda e:e['open'])['items']==opened['items']
            # Native take and transmute must temporarily enter peace, then
            # restore combat without restarting attacks on the corpse.
            a.send('combat',enabled=True);until(a,mark,'combat',lambda e:e['enabled'])
            time.sleep(.15);mark=len(a.events)
            if n==0:a.send('take_item',id=temp,container=mob['id'],**{'from':0})
            else:
                bm=len(b.events);b.send('transmute_loot',id=mob['id']);until(b,bm,'chat',lambda e:'Loot rejected' in str(e))
                a.send('transmute_loot',id=mob['id'])
                until(a,mark,'chat',lambda e:'Loot transmuted for' in str(e))
                until(a,mark,'combat',lambda e:e['enabled'])
                states=[e['enabled'] for e in a.events[mark:] if e.get('event')=='combat']
                assert states==[False,True],states
                assert not any(e.get('event')=='object' and e.get('container')==a.id for e in a.events[mark:])
                mark=len(a.events);a.send('transmute_loot',id=mob['id']);until(a,mark,'chat',lambda e:'Loot rejected' in str(e))
                assert not any('Loot transmuted for' in str(e) for e in a.events[mark:])
                continue
            grant=until(a,mark,'object',lambda e:e.get('container')==a.id and e.get('entity')==item['entity'])
            owned.append(grant['id']);until(a,mark,'chat',lambda e:'Loot collected' in str(e))
            until(a,mark,'combat',lambda e:e['enabled'])
            assert [e['enabled'] for e in a.events[mark:] if e.get('event')=='combat']==[False,True]
            if n==0 and len(opened['items'])>1:a.send('loot',id=mob['id'])
            until(a,mark,'container_state',lambda e:not e['open'])
            mark=len(a.events);a.send('loot',id=mob['id']);until(a,mark,'chat',lambda e:'Loot rejected' in str(e))
            assert not any(e.get('event')=='object' and e.get('container')==a.id for e in a.events[mark:])
        assert len(owned)==1
        stop(server);server=start();a=login(identity=aid)
        bag=until(a,0,'inventory')['items'];assert set(owned)<=set(bag)
        result={'result':'PASS','checks':['corpse rename on both clients and late entry','nested corpse inspection','owned loot inspection','single take from combat restores stance','transmute from combat','transmute ownership and replay','loot all','close/reopen','owner/range validation','no duplicate claim','inventory survives restart'],'directory':str(data)}
        (data/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
    finally:
        for i,p in enumerate(peers):
            (data/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
        if server.poll() is None:(data/'world/stop.request').write_text('stop\n');server.wait(timeout=10)
        log.close()

if __name__=='__main__':main()
