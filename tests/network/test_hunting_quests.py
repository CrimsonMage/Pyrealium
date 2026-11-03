"""Real UDP hunting dialogue, kills, return rewards and restart persistence.

NPCs use production placements. Ten low-health copies of each native target at
one of its production ground positions keep every hunt bounded and repeatable;
these copies and test-map travel exist only in this disposable test world.
"""
import json
import math
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until
from prepare_content import prepare

PORT = 19536


def main():
    run = ROOT/'Saved/AC2NetworkTests'/('hunting-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    pack = {'quests': [q for version in (1,2) for q in json.loads(
        (ROOT/f'content/hunting-content-v{version}.json').read_text())['quests']]}
    prepare(DAT_DIRECTORY, run/'prepared.tsv')
    rows = [s.split('\t') for s in (run/'prepared.tsv').read_text(encoding='utf8').splitlines()
            if len(s.split('\t')) == 13]
    npcs, targets, selected = {}, {}, []
    for q in pack['quests']:
        npc = next(r for r in rows if int(r[1],16) == q['npc'])
        if q['npc'] not in npcs: selected.append(npc)
        npcs[q['npc']] = npc
        originals = {int(r[1],16):r for r in rows if int(r[1],16) in q['targets']}
        assert originals, q['title']
        targets[q['id']] = []
        for n in range(10):
            row = originals[sorted(originals)[n % len(originals)]].copy()
            row[0] = f'{0x8140000000034000+(q["id"] & 255)*16+n:016x}'
            selected.append(row); targets[q['id']].append(row)
    content = run/'world.tsv'
    content.write_text('pyrealium-world-v4\n'+''.join('\t'.join(r)+'\n' for r in selected),encoding='utf8')
    rules = run/'rules.tsv'
    rules.write_text('pyrealium-encounters-v1\nplayer\t1000\t1\t1000\t30\nregen\t1000\t1000\t1000\n'+
                    ''.join(f'npc\t{e}\t1\t1\t60000\t3\t300000\t0\t0\n'
                            for e in sorted({r[1] for v in targets.values() for r in v})))
    # Seed only this isolated character store, using the supported offline API.
    for command in [
        ['character-create-human','--account','local','--name','Hunt Tester'],
        ['player-state','--character-id','1','--event','x,113726634933'],
        ['character-create-human','--account','local','--name','Low Level Hunter']]:
        subprocess.run([str(SERVER),*command,'--data',str(run/'world')],check=True,capture_output=True)
    peers, result = [], {}
    process = None
    with (run/'server.log').open('w',encoding='utf8') as log:
        def stop():
            if process and process.poll() is None:
                (run/'world/stop.request').write_text('stop\n'); process.wait(timeout=15)

        def start():
            p = subprocess.Popen([str(SERVER),'serve','--bind',f'127.0.0.1:{PORT}',
                '--account','local','--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),
                '--content',str(content),'--encounters',str(rules),'--initialize-ui','true',
                '--training-kit','false','--portals','false','--lifestones','false',
                '--test-map-travel','true','--seconds','600'],stdout=log,stderr=log,creationflags=0x08000000,
                env=dict(os.environ,AC2_TEST_LOOT_SEED='17'))
            time.sleep(1)
            assert p.poll() is None,(run/'server.log').read_text()
            return p

        def login(identity='1'):
            p = Peer(); peers.append(p); p.id = identity
            p.send('connect',host='127.0.0.1',port=PORT,account='local'); p.wait('characters')
            p.send('enter',id=identity); p.wait('player'); p.send('ready')
            p.wait('status',lambda e:e['phase']=='world'); until(p,0,'inventory')
            return p

        def leave(p):
            mark=len(p.events); p.send('logout',id=p.id); until(p,mark,'selection'); p.close()

        def at(p,row,distance=.5):
            pos=list(map(float,row[4:7])); pos[0]+=distance
            cell=int(row[3],16)
            # Interior coordinates are room-local and can be negative. Never
            # reinterpret an indoor variant as an outdoor landscape cell.
            if 1 <= (cell & 65535) <= 64:
                dx,dy=math.floor(pos[0]/160),math.floor(pos[1]/160)
                pos[0]-=dx*160; pos[1]-=dy*160
                cell=(((cell>>24)+dx)<<24)|((((cell>>16)&255)+dy)<<16)|(int(pos[0]/20)*8+int(pos[1]/20)+1)
            p.send('move',cell=cell,position=pos,heading=0,motion=[0,0,0])
            until(p,0,'object',lambda e:e['id']==str(int(row[0],16)))
            time.sleep(.1)

        def use(p,npc):
            time.sleep(1.1)
            mark=len(p.events); p.send('use',item=str(int(npcs[npc][0],16)),target='0')
            return mark

        def journal(p,mark,q,count,completed=False):
            status=0x30000000 if completed else 0x10000000
            phase=0 if completed else count+1
            return until(p,mark,'quests',lambda e:any(i['id']==q and i['status']==status and i['phase']==phase for i in e['items']))

        def xp(p):
            return int(next(e['value'] for e in reversed(p.events)
                            if e.get('event')=='quality' and e.get('id') is None and e['key']==0x12c))

        try:
            process=start(); low=login('2')
            at(low,npcs[pack['quests'][0]['npc']]); mark=use(low,pack['quests'][0]['npc'])
            until(low,mark,'chat',lambda e:'level or prerequisite' in str(e)); leave(low)
            result['level_gate_enforced']=True
            hero=login()
            for i,q in enumerate(pack['quests']):
                at(hero,npcs[q['npc']]); mark=use(hero,q['npc']); journal(hero,mark,q['id'],0)
                until(hero,mark,'script',lambda e:e['function']==0x82120024)
                for n,row in enumerate(targets[q['id']]):
                    at(hero,row); mark=len(hero.events); iid=str(int(row[0],16))
                    hero.send('target',id=iid); hero.send('attack',id=iid)
                    until(hero,mark,'quality',lambda e:e.get('id')==iid and e['key']==0x102 and e['value']==0)
                    journal(hero,mark,q['id'],n+1)
                    hero.send('combat',enabled=False); until(hero,mark,'combat',lambda e:not e['enabled'])
                    changed_hunts=[item['id'] for e in hero.events[mark:] if e['event']=='quests'
                                   for item in e['items'] if 0x4000fc01<=item['id']<=0x4000fc23]
                    assert changed_hunts==[q['id']], ('Unchanged hunts retransmitted', changed_hunts)
                    if i==0 and n==3:
                        leave(hero); stop(); process=start(); hero=login()
                        journal(hero,0,q['id'],4); result['partial_progress_survives_restart']=True
                at(hero,npcs[q['npc']]); before=xp(hero); mark=use(hero,q['npc'])
                journal(hero,mark,q['id'],10,True)
                until(hero,mark,'quality',lambda e:e.get('id') is None and e['key']==0x12c and int(e['value'])==before+q['xp_cap'])
                result[q['title']]={'accepted':True,'kills':10,'returned':True,'xp':q['xp_cap']}
                print('PASS', q['title'], flush=True)
            # A standalone completed quest cannot pay twice before its repeat timer.
            before=xp(hero); mark=use(hero,pack['quests'][-1]['npc'])
            until(hero,mark,'script',lambda e:e['function']==0x82120024)
            time.sleep(.2); assert xp(hero)==before
            result['duplicate_turn_in_rejected']=True
            leave(hero); stop(); process=start(); hero=login()
            for q in pack['quests']: journal(hero,0,q['id'],10,True)
            result['all_completed_journals_survive_restart']=len(pack['quests'])
            result['kill_updates_exclude_unchanged_hunts']=True
            leave(hero)
            (run/'result.json').write_text(json.dumps(result,indent=2))
            print('PASS',run,json.dumps(result))
        finally:
            for i,p in enumerate(peers):
                (run/f'peer-{i}.json').write_text(json.dumps(p.events)); p.close()
            stop()


if __name__=='__main__': main()
