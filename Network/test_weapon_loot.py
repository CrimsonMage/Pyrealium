"""Real UDP: rolled weapon pickup, level gate, combat, and restart persistence.

Boss position/health/XP and the loot seed are isolated fixtures. Weapon identities
and minimum levels are DAT evidence; generated damage remains Pyrealium policy.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY
from test_gameplay import until

PORT = 19532
BOSS, TARGET = [str(0x8140000000030E00+n) for n in range(2)]
STAFF, NEST = 0x47000F0B, 0x470010DA


def main():
    run = ROOT/'Saved/AC2NetworkTests'/('weapon-loot-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    content = run/'world.tsv'
    content.write_text('pyrealium-world-v4\n'+''.join(
        f'{int(iid):016x}\t{entity:08x}\t{visual:08x}\t02990109\t{x}\t240.199\t-44.606205\t1\t0\t0\t0\t-\t-\n'
        for iid,entity,visual,x in [(BOSS,0x4700390b,0x1f001a01,60),
                                   (TARGET,0x470015ac,0x1f000036,62)]))
    encounters = run/'encounters.tsv'
    encounters.write_text('pyrealium-encounters-v1\nplayer\t20\t1\t1000\t3\nregen\t0\t0\t1000\n'
                          'npc\t4700390b\t20\t1\t60000\t3\t300000\t1000\t0\n'
                          'npc\t470015ac\t1000\t1\t60000\t3\t300000\t0\t0\n')
    peers, process = [], None
    with (run/'server.log').open('w', encoding='utf-8') as log:
        def start():
            p = subprocess.Popen([str(SERVER),'serve','--bind',f'127.0.0.1:{PORT}',
                '--account','local','--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),
                '--content',str(content),'--encounters',str(encounters),'--initialize-ui','true',
                '--training-kit','false','--lifestones','false','--portals','false','--seconds','120'],
                stdout=log,stderr=log,creationflags=0x08000000,
                env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1',AC2_TEST_LOOT_SEED='5'))
            time.sleep(1)
            assert p.poll() is None, (run/'server.log').read_text()
            return p

        def login(name=None, identity=None):
            p=Peer(); peers.append(p)
            p.send('connect',host='127.0.0.1',port=PORT,account='local');p.wait('characters')
            if name:
                p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
                identity=p.wait('created')['character']['id']
            p.id=identity;p.send('enter',id=identity);p.wait('player')
            p.send('ready');p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory')
            p.send('move',cell=0x02990109,position=[58.5,240.199,-44.606205],heading=0,motion=[0,0,0])
            return p

        def inspect(p, iid):
            time.sleep(.15)  # Server examination cooldown.
            mark=len(p.events);p.send('examine',id=iid)
            panel=until(p,mark,'examination',lambda e:e['id']==iid)
            return [n.get('text',{}).get('literal') or '' for n in panel['nodes']]

        try:
            process=start();hero,observer=login('Weapon Hunter'),login('Weapon Observer')
            identity=hero.id
            until(hero,0,'object',lambda e:e['id']==BOSS)
            mark=len(hero.events);hero.send('attack',id=BOSS)
            until(hero,mark,'quality',lambda e:e['id']==BOSS and e['key']==0x102 and e['value']==0)
            hero.send('combat',enabled=False);until(hero,mark,'combat',lambda e:not e['enabled'])
            mark=len(hero.events);hero.send('open_container',id=BOSS)
            opened=until(hero,mark,'container_state',lambda e:e['open'])
            drops=[until(hero,mark,'object',lambda e:e['id']==iid) for iid in opened['items']]
            assert len(drops)==4,drops
            staff=next(e for e in drops if e['entity']==STAFF)
            expected=['Weapon damage: 115','Treasure level: 56','Required level: 1']
            before=inspect(hero,staff['id']);assert all(s in before for s in expected),before
            mark=len(hero.events);hero.send('loot',id=BOSS)
            until(hero,mark,'container_state',lambda e:not e['open'])
            owned=[e for e in hero.events[mark:] if e.get('event')=='object' and e.get('container')==hero.id]
            assert sorted(e['entity'] for e in owned)==sorted(e['entity'] for e in drops),(owned,drops)
            staff=next(e for e in owned if e['entity']==STAFF)
            nest=next(e for e in owned if e['entity']==NEST)
            after=inspect(hero,staff['id']);assert all(s in after for s in expected),after
            # Level 56 describes the generated roll, not its native equip gate.
            # The staff requires 1; Stinging Nest requires 15 and must reject us.
            mark=len(hero.events);hero.send('use',item=nest['id'],target='0')
            until(hero,mark,'chat',lambda e:'requires level 15' in str(e))
            time.sleep(.3)
            mark,remote=len(hero.events),len(observer.events)
            hero.send('use',item=staff['id'],target='0')
            until(hero,mark,'inventory_result',lambda e:e['id']==staff['id'] and e['status']==0 and e['equipped']==0x40)
            hero.send('skill_train',skill=0x59);time.sleep(.3)
            for attempt in range(5):
                mark,remote=len(hero.events),len(observer.events)
                hero.send('skill',skill=0x59,target=TARGET)
                until(hero,mark,'skill_finished')
                try:
                    hit=until(hero,mark,'hit',lambda e:e['id']==TARGET and e['damage']>80,timeout=2)
                    break
                except AssertionError:
                    if attempt==4:raise
            until(observer,remote,'fx',lambda e:e['id']==TARGET and e['fx']==0x41000241)
            time.sleep(2.2)
            hits=[e for e in hero.events[mark:] if e.get('event')=='hit' and e['id']==TARGET]
            assert len(hits)>=3 and all(e['damage']==1 for e in hits[1:]),hits
            hero.send('combat',enabled=False)
            # Persist the equipped generated item through a full server restart.
            for index,p in enumerate(peers):
                (run/f'before-peer-{index}.json').write_text(json.dumps(p.events,indent=2));p.close()
            peers.clear();(run/'world/stop.request').write_text('stop\n');process.wait(timeout=10)
            process=start();hero=login(identity=identity)
            restored=until(hero,0,'object',lambda e:e['id']==staff['id'])
            assert restored['entity']==STAFF,restored
            panel=inspect(hero,staff['id'])
            assert all(s in panel for s in expected) and 'Equipped in main hand.' in panel,panel
            result=dict(weapon_drops=len(drops),damage=115,treasure_level=56,native_minimum_level=1,
                        native_level_15_rejected=True,flame_bolt_damage=hit['damage'],
                        periodic_damage=1,remote_spell_effect=True,stats_and_equipment_survive_restart=True)
            (run/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result));print(run)
        finally:
            for index,p in enumerate(peers):
                (run/f'peer-{index}.json').write_text(json.dumps(p.events,indent=2));p.close()
            if process and process.poll() is None:
                (run/'world/stop.request').write_text('stop\n');process.wait(timeout=10)


if __name__=='__main__':main()
