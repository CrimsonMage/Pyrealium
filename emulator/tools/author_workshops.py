"""Authored workshop yards near known settlements; positions are not retail captures."""
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
out=ROOT/'emulator/content/workshops-content-v1.json'
templates={str(d):dict(visual=v,name=n) for d,v,n in [
 (0x4700082c,0x1f00065a,'Crafting Workshop'),(0x4700082d,0x1f00056a,'Refining Workshop'),
 (0x4700082e,0x1f000657,'Spellbinding Workshop')]}
# Native visuals are resolved afresh below, never trusted from old captures.
import sys
sys.path.insert(0,str(ROOT/'Tools/AC2'))
sys.path.insert(0,str(ROOT/'Network'))
from content_profile import ContentProfile
p=ContentProfile(ROOT/"Asheron's Call 2 - Legions")
for d,row in templates.items():row['visual']=p.visual(int(d))
p.close()
towns=[('Arwic',44.0,-6.1),('Cragstone',33.9,-11.8)]
# The existing refining workshop is an observed Omishan anchor.
anchors=[dict(name=n,land=[1020+e*10.2,1020+north*10.2]) for n,north,e in towns]
anchors.append(dict(name='Observed Omishan workshop',land=[(0xa3*160+35.8999863)/20,(0x94*160+30.3000069)/20]))
npcs=[]
for index,a in enumerate(anchors):
 for slot,d in enumerate(templates):
  if index==2 and int(d)==0x4700082d:continue
  npcs.append(dict(id=0x8140000000200000+index*16+slot,entity=int(d),land=[a['land'][0]+(slot+1)*.75,a['land'][1]+.5],evidence=a))
out.write_text(json.dumps(dict(version=1,policy='Authored workshop yards. Native ground validation required; approximate settlement anchors, not captured retail placements.',npcs=npcs,camps=[],templates=templates),indent=2)+'\n')
