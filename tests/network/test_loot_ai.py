"""Real UDP regression: captured multi-item loot, idle AI, retained target/stance.

Isolated actual capture anchors; low combat health/damage are test-only rules.
"""
import json,os,subprocess,time,uuid
from test_network import ROOT,SERVER,DAT_DIRECTORY,Peer
from test_gameplay import until

def main():
    run=ROOT/'Saved/AC2NetworkTests'/('loot-ai-'+uuid.uuid4().hex);run.mkdir(parents=True)
    allrows=[l for l in (ROOT/'content/recovered-population.tsv').read_text().splitlines() if l.startswith('81')]
    wasp=next(l for l in allrows if l.startswith('8140f9422bd7f183'))
    vermin=next(l for l in allrows if l.startswith('8140f4fee9cdfd75'))
    # Test explicit two-child capacity using two real captured anchors.
    # This fixture does not assert that retail grouped these two placements.
    second=next(l for l in allrows if l.split('\t')[1]=='47000fa2' and l!=wasp)
    content=run/'world.tsv';content.write_text('pyrealium-world-v4\n'+'\n'.join([wasp,second,vermin])+'\n')
    rules=run/'encounters.tsv';rules.write_text('pyrealium-encounters-v1\nplayer\t10\t1\t1500\t3\nregen\t0\t0\t1000\n'
        'npc\t47000fa2\t25\t1\t3000\t3\t5000\t0\t0\n'
        'npc\t47001640\t100\t1\t3000\t3\t5000\t0\t0\n'
        +'spawn-group\t1\t2\t'+','.join(row.split('\t')[0] for row in [wasp,second])+'\n')
    peers=[];results={}
    def point(row,offset=0):
        f=row.split('\t');p=list(map(float,f[4:7]));p[0]+=offset
        return str(int(f[0],16)),int(f[3],16),p
    with (run/'server.log').open('w') as log:
        server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19510','--account','local','--data',str(run/'world'),
            '--game-data',str(DAT_DIRECTORY),'--content',str(content),'--encounters',str(rules),'--initialize-ui','true',
            '--training-kit','false','--portals','false','--lifestones','false','--seconds','130'],
            env=dict(os.environ,AC2_TEST_LOOT_SEED='5'),stdout=log,stderr=log,creationflags=0x08000000)
        try:
            time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()
            for name in ['Loot AI Hunter','Loot AI Observer']:
                p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19510,account='local');p.wait('characters')
                p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
                p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player');p.send('ready')
                p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory')
            a,b=peers
            # Native Scamp: 15m perception, 10m wandering radius. Observers stay
            # 22m away, within streaming range but outside initial perception.
            vid,cell,pos=point(vermin,22);home=point(vermin)[2]
            for p in peers:p.send('move',cell=cell,position=pos,heading=0,motion=[0,0,0])
            until(a,0,'object',lambda e:e['id']==vid)
            am,bm=len(a.events),len(b.events)
            moved=until(a,am,'position',lambda e:e['id']==vid and sum((x-y)**2 for x,y in zip(e['position'],home))>.01,timeout=40)
            mirror=until(b,bm,'position',lambda e:e['id']==vid and e['position']==moved['position'])
            assert not any(e.get('event')=='hit' and e.get('id')==vid for e in a.events[:a.events.index(moved)])
            assert sum((x-y)**2 for x,y in zip(mirror['position'][:2],home[:2]))<=100.1
            results['idle_roaming_without_being_hit']=True;results['movement_replicated']=True
            # Enter native perception at the creature's current location.
            pos=moved['position'][:];pos[0]+=1
            am=len(a.events);a.send('move',cell=moved['cell'],position=pos,heading=0,motion=[0,0,0])
            until(a,am,'hit',lambda e:e['id']==a.id,timeout=12)
            results['autonomous_aggro']=True
            tid,cell,pos=point(wasp,1.5)
            for p in peers:p.send('move',cell=cell,position=pos,heading=0,motion=[0,0,0])
            until(a,0,'object',lambda e:e['id']==tid)
            am=len(a.events);a.send('target',id=tid);a.send('attack',id=tid)
            until(a,am,'quality',lambda e:e.get('id')==tid and e.get('key')==0x102 and e['value']==15)
            # Stop and resume with no new target message. The selected monster
            # must survive stance changes and the previous completed swing.
            a.send('combat',enabled=False);until(a,am,'combat',lambda e:not e['enabled'])
            am=len(a.events);a.send('combat',enabled=True)
            until(a,am,'quality',lambda e:e.get('id')==tid and e.get('key')==0x102 and e['value']==0,timeout=12)
            until(a,am,'combat',lambda e:e['enabled'])
            assert not any(e.get('event')=='combat' and not e['enabled'] for e in a.events[am:])
            results['target_and_stance_retained']=True
            a.send('combat',enabled=False);until(a,am,'combat',lambda e:not e['enabled'])
            bm=len(b.events);b.send('open_container',id=tid);until(b,bm,'chat',lambda e:'Loot rejected' in str(e))
            am=len(a.events);a.send('open_container',id=tid);opened=until(a,am,'container_state',lambda e:e['open'])
            assert len(opened['items'])>=2,opened
            original=opened['items'][:];drops=[]
            for iid in original:drops.append(until(a,am,'object',lambda e:e['id']==iid)['entity'])
            assert drops!=[0x47001745]*len(drops)
            am=len(a.events);a.send('take_item',id=original[0],container=tid,**{'from':0})
            remaining=until(a,am,'container_state',lambda e:e['open'])
            assert remaining['items']==original[1:]
            am=len(a.events);a.send('close_container',id=tid);until(a,am,'container_state',lambda e:not e['open'])
            a.send('open_container',id=tid);assert until(a,am,'container_state',lambda e:e['open'])['items']==original[1:]
            am=len(a.events);a.send('loot',id=tid);until(a,am,'container_state',lambda e:not e['open'])
            am=len(a.events);a.send('loot',id=tid);until(a,am,'chat',lambda e:'Loot rejected' in str(e))
            sid,cell,pos=point(second,2)
            bm=len(b.events);b.send('move',cell=cell,position=pos,heading=0,motion=[0,0,0])
            # Nearby child may already be visible; moving closer must not
            # require another CreateObject or generate a duplicate child.
            other=until(b,0,'object',lambda e:e['id']==sid)
            assert other['entity']==0x47000fa2
            assert all(abs(x-y)<.001 for x,y in zip(other['placement']['position'],point(second)[2]))
            results['explicit_group_has_two_independent_children']=True
            assert not any(e.get('event')=='object' and e.get('container')==a.id for e in a.events[am:])
            results.update(multi_item_drop=[hex(d) for d in drops],single_take_keeps_remaining=True,reopen_does_not_reroll=True,loot_all=True,ownership_and_replay_protected=True)
            (run/'result.json').write_text(json.dumps(dict(result='PASS',checks=results),indent=2));print('PASS',run,json.dumps(results))
        finally:
            for i,p in enumerate(peers):(run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            if server.poll() is None:(run/'world/stop.request').write_text('stop\n');server.wait(timeout=10)
if __name__=='__main__':main()
