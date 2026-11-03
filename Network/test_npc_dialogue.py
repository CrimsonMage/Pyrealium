"""All wiki NPC reply identities over native UDP, plus real placement interactions.

Synthetic nearby copies exercise the entire catalog, including unplaced event
variants; they exist only in this isolated test. Production placement safety is
covered by the DAT-backed npc_population Rust test.
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


def main():
    run=ROOT/'Saved/AC2NetworkTests'/('npc-dialogue-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    catalog=json.loads((ROOT/'emulator/content/npc-dialogue-v1.json').read_text(encoding='utf8'))
    prepare(DAT_DIRECTORY,run/'prepared.tsv')
    production=[line.split('\t') for line in (run/'prepared.tsv').read_text(encoding='utf8').splitlines() if line.startswith('8140')]
    control=next(row for row in production if row[1]=='470007d1')
    profiles={p['entity']:p for p in catalog['profiles']}
    rows={}
    for i,p in enumerate(profiles.values()):
        x=21+(i//60)*20
        cell=0x77b80000+(x//20)*8+1
        rows[p['entity']]=[f"{0x8140d50000000000+p['entity']:016x}",f"{p['entity']:08x}",
            f"{p['visual']:08x}",f'{cell:08x}',str(x),'1','180','1','0','0','0','-',p['name']]
    # Newly authored outdoor actors must also work at their actual placements.
    actual=[row for row in production if row[0].startswith('8140d400')]
    content=run/'world.tsv'
    content.write_text('pyrealium-world-v4\n'+'\n'.join('\t'.join(r) for r in [*rows.values(),*actual,control])+'\n',encoding='utf8')
    for command in [['character-create-human','--account','local','--name','NPC Reader']]:
        subprocess.run([str(SERVER),*command,'--data',str(run/'world')],check=True,capture_output=True)
    peers=[];server=None;report={}
    with (run/'server.log').open('w',encoding='utf8') as log:
        try:
            server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19540','--account','local',
                '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
                '--initialize-ui','true','--training-kit','false','--portals','false','--lifestones','false',
                '--test-map-travel','true','--seconds','600'],stdout=log,stderr=log,creationflags=0x08000000,
                env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1'))
            time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()

            def login(identity):
                p=Peer();p.id=identity;peers.append(p)
                p.send('connect',host='127.0.0.1',port=19540,account='local');p.wait('characters')
                p.send('enter',id=identity);p.wait('player');p.send('ready')
                p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory')
                return p

            def at(p,row,distance=.5):
                pos=list(map(float,row[4:7]));pos[0]+=distance;cell=int(row[3],16)
                if 1<=cell&65535<=64:
                    dx,dy=math.floor(pos[0]/160),math.floor(pos[1]/160)
                    pos[0]-=160*dx;pos[1]-=160*dy
                    cell=(((cell>>24)+dx)<<24)|((((cell>>16)&255)+dy)<<16)|(int(pos[0]/20)*8+int(pos[1]/20)+1)
                p.send('move',cell=cell,position=pos,heading=0,motion=[0,0,0])
                until(p,0,'object',lambda e:e['id']==str(int(row[0],16)));time.sleep(.1)
                p.send('target',id=str(int(row[0],16)))

            def speak(p,row,command='talk',delay=1.05):
                time.sleep(delay);mark=len(p.events)
                if command=='talk':p.send('chat',text='@talk',channel='say')
                else:p.send('use',item=str(int(row[0],16)),target='0')
                return mark

            def notice(p,mark,text):
                until(p,mark,'chat',lambda e:text in str(e))
                until(p,mark,'script',lambda e:e['function']==0x82120024)
                # Native use completion precedes speech and the readable notice.
                scripts=[e['function'] for e in p.events[mark:] if e['event']=='script']
                assert scripts[:2]==[0x82160021,0x82120024],scripts
                assert any(e['event']=='chat' and e.get('kind')==32 and e.get('speaker') for e in p.events[mark:])

            p=login('1');warder=rows[0x470015c0]
            at(p,warder,8);mark=speak(p,warder,'use')
            until(p,mark,'chat',lambda e:'four metres' in str(e))
            assert not any(e['event']=='script' and e['function']==0x82120024 for e in p.events[mark:])
            at(p,warder);mark=len(p.events);p.send('combat',enabled=True)
            until(p,mark,'combat',lambda e:e['enabled'])
            mark=speak(p,warder,'use');until(p,mark,'chat',lambda e:'out of combat' in str(e))
            p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
            report['distance_and_combat_guards']=True
            mark=speak(p,warder,'use');notice(p,mark,profiles[0x470015c0]['topics'][0]['text'])
            mark=speak(p,warder,'use',delay=0)
            until(p,mark,'chat',lambda e:'Wait briefly' in str(e))
            report['native_double_click_and_throttle']=True
            start=len(p.events)
            for i,(entity,profile) in enumerate(profiles.items()):
                row=rows[entity];at(p,row);mark=speak(p,row)
                notice(p,mark,profile['name']+':')
                if i%25==0:print('NPC catalog',i+1,'/',len(profiles),flush=True)
            assert not any(e['event'] in ('quests','inventory_add','inventory_remove','shop') for e in p.events[start:])
            assert not any(e['event']=='quality' and e.get('id') is None and e.get('key') in (0x12c,0x12d) for e in p.events[start:])
            report['native_catalog_replies_without_rewards_or_quest_mutation']=len(profiles)
            # A complete multi-topic cycle must return to its beginning.
            row=rows[0x470022fe];at(p,row);profile=profiles[0x470022fe]
            for page in range(len(profile['topics'])+1):
                mark=speak(p,row,'use');notice(p,mark,profile['topics'][page%len(profile['topics'])]['text'])
            report['topic_cycle']=len(profile['topics'])
            # Real outdoor placements, including the new Brisin shop identity,
            # can be selected and conversed with in the prepared world.
            for row in actual:
                at(p,row);mark=speak(p,row);notice(p,mark,row[12]+':')
            report['production_placements_replied']=len(actual)
            # Conversation lookup never turns hostile variants into speakers.
            at(p,control);mark=speak(p,control)
            until(p,mark,'chat',lambda e:'cannot hold a conversation' in str(e))
            report['hostile_target_rejected']=True
            # Existing explorer action takes precedence over information fallback.
            row=rows[0x4700159f];at(p,row);mark=speak(p,row,'use')
            until(p,mark,'quests');report['existing_explorer_quest_preserved']=True
            mark=len(p.events);p.send('logout',id='1');until(p,mark,'selection');p.close()
            (run/'result.json').write_text(json.dumps(report,indent=2))
            print('PASS',run,json.dumps(report),flush=True)
        finally:
            for i,p in enumerate(peers):
                (run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            if server and server.poll() is None:
                (run/'world/stop.request').write_text('stop\n');server.wait(timeout=15)


if __name__=='__main__':main()
