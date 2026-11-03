"""Real UDP: native Lightning Bolt vigor ticks and bounded boss treasure.

Locations, training XP and weakened boss health are isolated test fixtures.
The Outcast's vigor and the boss's loot bounds come from the installed DAT.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY
from test_gameplay import until

PORT = 19526
IDOL, TARGET, BOSS = [str(0x8140000000030D00+n) for n in range(3)]


def main():
    run = ROOT/'Saved/AC2NetworkTests'/('vigor-loot-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    content = run/'world.tsv'
    content.write_text('pyrealium-world-v4\n'+''.join(
        f'{int(iid):016x}\t{entity:08x}\t{visual:08x}\t02990109\t{x}\t240.199\t-44.606205\t1\t0\t0\t0\t-\t-\n'
        for iid,entity,visual,x in [(IDOL,0x470007d1,0x1f0003d1,60),
                                   (TARGET,0x470015ac,0x1f000036,62),
                                   (BOSS,0x4700390b,0x1f001a01,60.5)]))
    encounters = run/'encounters.tsv'
    encounters.write_text('pyrealium-encounters-v1\nplayer\t20\t1\t1000\t3\nregen\t0\t0\t1000\n'
                          'npc\t470007d1\t20\t1\t60000\t3\t300000\t1000\t0\n'
                          'npc\t4700390b\t20\t1\t60000\t3\t300000\t0\t0\n')
    peers = []
    with (run/'server.log').open('w', encoding='utf-8') as log:
        process = subprocess.Popen([str(SERVER),'serve','--bind',f'127.0.0.1:{PORT}',
            '--account','local','--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),
            '--content',str(content),'--encounters',str(encounters),'--initialize-ui','true',
            '--training-kit','false','--lifestones','false','--portals','false','--seconds','110'],
            stdout=log,stderr=log,creationflags=0x08000000,
            env=dict(os.environ,AC2_TEST_LOOT_SEED='42'))
        try:
            time.sleep(1)
            assert process.poll() is None, (run/'server.log').read_text()
            for name in ['Vigor Tester','Vigor Observer']:
                p=Peer(); peers.append(p)
                p.send('connect',host='127.0.0.1',port=PORT,account='local');p.wait('characters')
                p.send('create',name=name,start_area=0,race=4,sex=4096,physical=[[1,1],[2,1]])
                p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player')
                p.send('ready');p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory')
                p.send('move',cell=0x02990109,position=[58.5,240.199,-44.606205],heading=0,motion=[0,0,0])
            hero,observer=peers
            until(hero,0,'object',lambda e:e['id']==IDOL)
            mark=len(hero.events);hero.send('attack',id=IDOL)
            until(hero,mark,'quality',lambda e:e['id']==IDOL and e['key']==0x102 and e['value']==0)
            hero.send('combat',enabled=False);until(hero,mark,'combat',lambda e:not e['enabled'])
            for skill in [0x2a1,0x2a6]:
                hero.send('skill_train',skill=skill);time.sleep(.3)
            # The native drum is required even though the skill is learned.
            mark=len(hero.events);hero.send('skill',skill=0x2a6,target=TARGET)
            until(hero,mark,'chat',lambda e:'requires a drum' in str(e));until(hero,mark,'skill_finished')
            drum=next(e for e in hero.events if e.get('event')=='object' and e.get('container')==hero.id and e.get('entity')==0x470007f7)
            mark=len(hero.events);hero.send('use',item=drum['id'],target='0')
            until(hero,mark,'inventory_result',lambda e:e['id']==drum['id'] and e['status']==0)
            for p in peers:p.send('move',cell=0x02990109,position=[58.5,240.199,-44.606205],heading=0,motion=[0,0,0])
            until(hero,0,'quality',lambda e:e['id']==TARGET and e['key']==0x108 and e['value']==164)
            for attempt in range(5):
                mark,remote=len(hero.events),len(observer.events)
                hero.send('skill',skill=0x2a6,target=TARGET);until(hero,mark,'skill_finished')
                try:
                    first=until(hero,mark,'quality',lambda e:e['id']==TARGET and e['key']==0x103 and e['value']==162,timeout=3)
                    break
                except AssertionError:
                    if attempt==4:raise
            until(observer,remote,'quality',lambda e:e['id']==TARGET and e['key']==0x103 and e['value']==first['value'])
            until(hero,mark,'quality',lambda e:e['id']==TARGET and e['key']==0x103 and e['value']==160)
            mark=len(hero.events);hero.send('examine',id=TARGET)
            panel=until(hero,mark,'examination')
            assert any(f'Vigor: {n} / 164' in str(panel) for n in range(150,161)),panel
            # Kill the weakened boss and verify its native 3..6 generic slots.
            hero.send('combat',enabled=False)
            for p in peers:p.send('move',cell=0x02990109,position=[61,240.199,-44.606205],heading=0,motion=[0,0,0])
            mark=len(hero.events);hero.send('attack',id=BOSS)
            until(hero,mark,'quality',lambda e:e['id']==BOSS and e['key']==0x102 and e['value']==0)
            hero.send('combat',enabled=False);until(hero,mark,'combat',lambda e:not e['enabled'])
            mark=len(hero.events);hero.send('open_container',id=BOSS)
            opened=until(hero,mark,'container_state',lambda e:e['open']);assert 3<=len(opened['items'])<=6,opened
            drops=[until(hero,mark,'object',lambda e:e['id']==iid) for iid in opened['items']]
            # Seed 42 includes equipment. Inspect its durable rating/level before
            # and after the temporary corpse IID becomes an owned inventory IID.
            mark=len(hero.events);hero.send('examine',id=drops[0]['id'])
            before=until(hero,mark,'examination')
            def stats(panel):
                values = [n.get('text', {}).get('literal') or '' for n in panel['nodes']]
                return [value for value in values if value.startswith(('Armor:', 'Weapon damage:', 'Treasure level:'))]
            expected=stats(before)
            assert len(expected)==2 and 'Treasure level: 56' in expected,before
            remote=len(observer.events);observer.send('open_container',id=BOSS)
            until(observer,remote,'chat',lambda e:'Loot rejected' in str(e))
            mark=len(hero.events);hero.send('close_container',id=BOSS);until(hero,mark,'container_state',lambda e:not e['open'])
            mark=len(hero.events);hero.send('open_container',id=BOSS)
            assert until(hero,mark,'container_state',lambda e:e['open'])['items']==opened['items']
            mark=len(hero.events);hero.send('loot',id=BOSS);until(hero,mark,'container_state',lambda e:not e['open'])
            owned=[e for e in hero.events[mark:] if e.get('event')=='object' and e.get('container')==hero.id]
            assert sorted(e['entity'] for e in owned)==sorted(e['entity'] for e in drops),(owned,drops)
            received=next(e for e in owned if e['entity']==drops[0]['entity'])
            time.sleep(.15)  # Respect the server's 100 ms examination cooldown.
            mark=len(hero.events);hero.send('examine',id=received['id'])
            assert stats(until(hero,mark,'examination'))==expected
            result=dict(lightning_bolt_vigor_ticks=True,observer_receives_vigor=True,
                        examination_uses_current_vigor=True,native_boss_treasure_count=len(drops),
                        no_reroll=True,owner_only=True,loot_claims=True,rolled_stats_preserved=True)
            (run/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result));print(run)
        finally:
            for index,p in enumerate(peers):
                (run/f'peer-{index}.json').write_text(json.dumps(p.events,indent=2));p.close()
            if process.poll() is None:
                (run/'world/stop.request').write_text('stop\n');process.wait(timeout=10)


if __name__=='__main__':main()
