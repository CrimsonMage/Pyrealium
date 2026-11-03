"""Real UDP acceptance of inferred camps, hunt journals and delivery exchanges.

Only combat health/damage and the disposable test character's XP are overridden.
Camp transforms, capacity, quest bindings and rewards are the installed pack.
"""
import json, os, subprocess, time, uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until

def main():
    run=ROOT/'Saved/AC2NetworkTests'/('inferred-'+uuid.uuid4().hex);run.mkdir(parents=True)
    base=ROOT/'content'
    pack=json.loads((base/'inferred-content-v1.json').read_text())
    camps=[c for c in pack['camps'] if c['evidence']['quest']==0x40000097]
    ids={c['id']+i for c in camps for i in range(len(c['roster']))}
    rows=[l.split('\t') for l in (base/'inferred-world-v1.tsv').read_text().splitlines() if l.startswith('81') and int(l.split('\t')[0],16) in ids]
    world=[l.split('\t') for l in (ROOT/'data/world/legions-world.tsv').read_text().splitlines() if l.startswith('81')]
    kanten=next(f for f in world if f[1]=='47001a3c')
    orven=next(f for f in world if f[1]=='47001c27' and f[3].startswith('77b8'))
    rows += [kanten,orven]
    content=run/'world.tsv';content.write_text('pyrealium-world-v4\n'+'\n'.join('\t'.join(f) for f in rows)+'\n')
    rules=run/'rules.tsv';rules.write_text('pyrealium-encounters-v1\nplayer\t1000\t1\t500\t30\nregen\t10\t10\t1000\n'+
        ''.join(f'npc\t{entity}\t1\t1\t60000\t3\t300000\t1\t0\n' for entity in sorted({f[1] for f in rows if int(f[0],16) in ids})))
    env=dict(os.environ,AC2_TEST_LOOT_SEED='3');peers=[];server=None;character=None;report={}
    def stop():
        if server and server.poll() is None:(run/'world/stop.request').write_text('stop\n');server.wait(timeout=15)
    def start():
        process=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19520','--account','local','--data',str(run/'world'),
            '--game-data',str(DAT_DIRECTORY),'--content',str(content),'--encounters',str(rules),'--initialize-ui','true',
            '--training-kit','false','--portals','false','--lifestones','false','--test-map-travel','true','--seconds','180'],
            env=env,stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(1);assert process.poll() is None,(run/'server.log').read_text();return process
    def connect(create=False):
        p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19520,account='local');p.wait('characters')
        if create:
            p.send('create',name='Inferred Hunter',race=1,sex=4096,start_area=0,physical=[[1,1],[2,1]])
            p.id=p.wait('created')['character']['id']
        else:p.id=character
        p.send('enter',id=p.id);p.wait('player');p.send('ready');p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory');return p
    def leave(p):
        mark=len(p.events);p.send('logout',id=p.id);until(p,mark,'selection');p.close()
    def at(p,row):
        pos=list(map(float,row[4:7]));pos[0]+=.5
        p.send('move',cell=int(row[3],16),position=pos,heading=0,motion=[0,0,0])
        return until(p,0,'object',lambda e:e['id']==str(int(row[0],16)))
    def quest(p,mark,q,status):
        e=until(p,mark,'quests',lambda e:any(i['id']==q and i['status']==status for i in e['items']))
        return next(i for i in e['items'] if i['id']==q)
    with (run/'server.log').open('w') as log:
        try:
            server=start();p=connect(True);character=p.id;leave(p);stop()
            for args in [('player-state','--event','x,100000'),('inventory-grant','--id','47000f43','--visual-did','1f000e52')]:
                subprocess.run([str(SERVER),*args,'--account','local','--data',str(run/'world'),'--character-id',character,
                    '--game-data',str(DAT_DIRECTORY)],env=env,stdout=log,stderr=log,check=True,creationflags=0x08000000)
            server=start();p=connect()
            at(p,kanten);mark=len(p.events);p.send('use',item=str(int(kanten[0],16)),target='0')
            quest(p,mark,0x4000fe01,0x10000000)
            bag=until(p,mark,'object',lambda e:e.get('container')==p.id and e['entity']==0x47001c1e)['id']
            at(p,orven);mark=len(p.events);p.send('use',item=bag,target=str(int(orven[0],16)))
            quest(p,mark,0x4000fe01,0x30000000)
            until(p,mark,'chat',lambda e:'1,000 experience' in str(e))
            time.sleep(.3);mark=len(p.events);p.send('use',item=str(int(orven[0],16)),target='0')
            until(p,mark,'chat',lambda e:'Speak to Kanten' in str(e))
            report['delivery_bag_exchange_and_duplicate_rejection']=True
            elixir=until(p,0,'object',lambda e:e.get('container')==p.id and e['entity']==0x47000f43)['id']
            mark=len(p.events);p.send('use',item=elixir,target='0');quest(p,mark,0x4000000a,0x10000000)
            report['owned_elixir_starts_hunt']=True
            q=next(q for q in pack['quests'] if q['id']==0x40000097)
            roster={int(f[0],16):f for f in rows};dead=[];count=0;loot=[];claimed=[]
            for camp in camps:
                home=roster[camp['id']];at(p,home);time.sleep(.5)
                seen={int(e['id']) for e in p.events if e.get('event')=='object' and e.get('id') and int(e['id']) in {camp['id']+i for i in range(len(camp['roster']))}}
                assert len(seen)==camp['capacity'],(camp,seen)
                # Kill the starter once, then the three initially active targets.
                children=sorted(seen) if count==0 else sorted(seen)[1:]
                for iid in children:
                    if count>=q['goal']:break
                    row=roster[iid];at(p,row);mark=len(p.events);p.send('target',id=str(iid));p.send('attack',id=str(iid))
                    until(p,mark,'quality',lambda e:e.get('id')==str(iid) and e.get('key')==0x102 and e['value']==0)
                    p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
                    if int(row[1],16) in q['targets']:count+=1
                    state=quest(p,mark,q['id'],0x30000000 if count==q['goal'] else 0x10000000)
                    if count<q['goal']:assert f'Progress: {count} / {q["goal"]}' in state['journal']['literal'],state
                    mark=len(p.events);p.send('open_container',id=str(iid));opened=until(p,mark,'container_state',lambda e:e['open'])
                    for item in opened['items']:loot.append(until(p,mark,'object',lambda e:e['id']==item)['entity'])
                    p.send('close_container',id=str(iid));until(p,mark,'container_state',lambda e:not e['open'])
                    mark=len(p.events);p.send('open_container',id=str(iid));reopened=until(p,mark,'container_state',lambda e:e['open'])
                    assert reopened['items']==opened['items'],'Opening the corpse must not reroll its loot'
                    mark=len(p.events)
                    p.send('loot' if reopened['items'] else 'close_container',id=str(iid))
                    until(p,mark,'container_state',lambda e:not e['open'])
                    grants=[e['id'] for e in p.events[mark:] if e.get('event')=='object' and e.get('container')==p.id]
                    assert len(grants)==len(reopened['items']);claimed.extend(grants)
                    dead.append(iid)
            assert count==q['goal'];report.update(group_capacity=4,hunt_kills=count,hunt_reward=q['xp'],loot_definitions=[hex(d) for d in loot])
            leave(p);stop();server=start();p=connect()
            quest(p,0,0x40000097,0x30000000);quest(p,0,0x4000fe01,0x30000000);quest(p,0,0x4000000a,0x10000000)
            inv=until(p,0,'inventory');assert bag not in str(inv) and elixir not in str(inv),inv
            assert set(claimed)<=set(inv['items']),inv
            report['claimed_loot_survives_restart']=len(claimed)
            report['completion_and_consumed_items_survive_server_restart']=True
            leave(p);(run/'result.json').write_text(json.dumps(report,indent=2));print('PASS',run,json.dumps(report))
        finally:
            for i,p in enumerate(peers):(run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            stop()

if __name__=='__main__':main()
