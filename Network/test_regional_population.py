"""Real UDP visibility, combat and loot for the full regional world.

Three sampled templates use low-health encounter overrides to bound test time;
native stats/AI, all ground positions and group regeneration are checked by the
Rust regional_population acceptance tests. Never touches the user's store.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until
from prepare_content import prepare


def main():
    run=ROOT/'Saved/AC2NetworkTests'/('regional-'+uuid.uuid4().hex);run.mkdir(parents=True)
    content=run/'world.tsv';prepare(DAT_DIRECTORY,content)
    rows={int(f[0],16):f for l in content.read_text(encoding='utf8').splitlines()[1:]
          for f in [l.split('\t')] if len(f)==13}
    base=ROOT/'emulator/content'
    proof=json.loads((base/'regional-world-v1.report.json').read_text())
    targets=[]
    for region in ['Esper Reaches','Kydi Delta','Rings of Oriad']:
        groups=[g for g in proof['groups'] if g['region']==region]
        # Mid-region, away from the sparse extremes of the regional envelope.
        target=groups[len(groups)//2]['members'][0]
        targets.append((region,target,rows[target]))
    rules=run/'rules.tsv';rules.write_text('pyrealium-encounters-v1\nplayer\t1000\t1\t500\t30\nregen\t1000\t1000\t1000\n'+
        ''.join(f'npc\t{entity}\t1\t1\t60000\t3\t300000\t1\t0\n' for entity in sorted({f[1] for _,_,f in targets})))
    env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1',AC2_TEST_LOOT_SEED='3')
    peers=[];results=[]
    with (run/'server.log').open('w') as log:
        server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19524','--account','local',
            '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
            '--initialize-ui','true','--training-kit','false','--encounters',str(rules),
            '--lifestones','false','--portals','false','--test-map-travel','true','--seconds','180'],
            env=env,stdout=log,stderr=log,creationflags=0x08000000)
        try:
            time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()
            for name in ['Regional Hunter','Regional Observer']:
                p=Peer();peers.append(p)
                p.send('connect',host='127.0.0.1',port=19524,account='local');p.wait('characters')
                p.send('create',name=name,race=1,sex=4096,start_area=0,physical=[[1,1],[2,1]])
                p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player')
                p.send('ready');p.wait('status',lambda e:e['phase']=='world')
            for region,target,row in targets:
                point=list(map(float,row[4:7]));point[0]+=.5
                marks=[]
                for p in peers:
                    mark=len(p.events);marks.append(mark)
                    p.send('move',cell=int(row[3],16),position=point,heading=0,motion=[0,0,0])
                    obj=until(p,mark,'object',lambda e:e['id']==str(target))
                    assert obj['entity']==int(row[1],16),obj
                    until(p,mark,'quality',lambda e:e.get('id')==str(target) and e.get('key')==0x107 and e['value']==1)
                a,b=peers;mark=len(a.events);observed=len(b.events)
                a.send('target',id=str(target));a.send('attack',id=str(target))
                until(a,mark,'quality',lambda e:e.get('id')==str(target) and e.get('key')==0x102 and e['value']==0)
                until(b,observed,'quality',lambda e:e.get('id')==str(target) and e.get('key')==0x102 and e['value']==0)
                a.send('combat',enabled=False);until(a,mark,'combat',lambda e:not e['enabled'])
                mark=len(a.events);a.send('open_container',id=str(target))
                opened=until(a,mark,'container_state',lambda e:e['open'])
                items=[until(a,mark,'object',lambda e:e['id']==item)['entity'] for item in opened['items']]
                mark=len(a.events);a.send('loot' if items else 'close_container',id=str(target))
                until(a,mark,'container_state',lambda e:not e['open'])
                if items:
                    grants=[e for e in a.events[mark:] if e.get('event')=='object' and e.get('container')==a.id]
                    assert len(grants)==len(items),(items,grants)
                cell=int(row[3],16)
                x=(cell>>24)*160+float(row[4]);y=((cell>>16)&255)*160+float(row[5])
                results.append({'region':region,'creature':row[12],'id':row[0],'cell':row[3],
                    'north':round((y/20-1020)/10.2,2),'east':round((x/20-1020)/10.2,2),
                    'seen_by_two_clients':True,'killed':True,'claimed_loot':[f'{d:08x}' for d in items]})
            assert any(r['claimed_loot'] for r in results)
            result={'world_placements':len(rows),'regional_placements':proof['included'],
                    'spawner_groups':len(proof['groups']),'samples':results}
            (run/'result.json').write_text(json.dumps(result,indent=2))
            print('PASS',run,json.dumps(result))
        finally:
            for i,p in enumerate(peers):
                (run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            if server.poll() is None:
                (run/'world/stop.request').write_text('stop\n');server.wait(timeout=15)


if __name__=='__main__':main()
