"""Read-only Legions entity/template placement audit. No inferred spawn coordinates."""
import argparse
import collections
import json
import struct
from pathlib import Path
from dat_probe import Archive

KEYS = [0xE5849238,0xEB2D767D,0xFC0FDD03,0xCB45A6A6,0x7F21E6BB,0xF5A6CDC8,0x22D9AD12,0xB2A3D4DE,
        0x3B834DB7,0xC406C91C,0x8392305E,0x0A9F0099,0x0943B90C,0xA66D953C,0x0D929E1F,0x0E2E9AD4]

class Reader:
    def __init__(self, data): self.data, self.pos = data, 0
    def take(self, n):
        assert 0 <= n <= len(self.data)-self.pos
        b=self.data[self.pos:self.pos+n]; self.pos+=n; return b
    def word(self): return struct.unpack('<I',self.take(4))[0]
    def align(self): self.take((-self.pos)&3)
    def string(self, width=1):
        n=struct.unpack('<H',self.take(2))[0]
        if n==65535: n=self.word()
        b=bytearray(self.take(n*width)); self.align()
        for i in range(len(b)//4):
            struct.pack_into('<I',b,i*4,struct.unpack_from('<I',b,i*4)[0]^KEYS[i&15])
        b=bytes(((x<<4)|(x>>4))&255 for x in b)
        return b.decode('utf-16-le' if width==2 else 'utf-8',errors='replace')

def properties(r, types):
    values={}
    for _ in range(r.word()):
        group=r.word()
        for _ in range(r.word()):
            key=r.word(); t=types[key]&65535
            if t in (1,2,3,5,7,8,11): value=r.word()
            elif t==6: value=r.string()
            elif t in (4,9,13,14): value=r.take({4:12,9:44,13:8,14:32}[t]).hex()
            elif t==10:
                value=[r.word(),r.word()]; n=r.word()
                if n>>16: value.append(r.string(2))
                assert not n&65535
            else: raise ValueError(t)
            values[f'{group:08X}:{key:08X}']=value
    return values

def entity(r, types):
    bits=r.word(); assert not bits&~511
    e={'type':0,'data':0,'origin':[0,0,0],'rotation':[1,0,0,0],'scale':[1,1,1]}
    if bits&1: e['id']=r.word()
    if bits&2: e['type']=r.word()
    if bits&4: e['instance']=struct.unpack('<Q',r.take(8))[0]
    if bits&8: e['data']=r.word()
    if bits&16:
        e['origin']=struct.unpack('<3f',r.take(12)); e['rotation']=struct.unpack('<4f',r.take(16)); r.take(36)
    if bits&32: e['scale']=struct.unpack('<3f',r.take(12))
    if bits&128: e['properties']=properties(r,types)
    if bits&64: e['version']=r.word()
    if bits&256: e['name']=r.string()
    return e

def main():
    p=argparse.ArgumentParser(); p.add_argument('directory',type=Path); p.add_argument('--output',type=Path,required=True); a=p.parse_args()
    portal=Archive(a.directory/'portal.dat'); cell=Archive(a.directory/'cell_1.dat')
    r=Reader(portal.read(0x34000000)); assert r.word()==0x34000000; r.take(8)
    names={}
    for _ in range(r.word()):
        k=r.word(); names[k]=r.string()
    types={}
    for _ in range(r.word()&65535):
        k=r.word(); types[k]=r.word(); r.take(16)
    templates={}; rejected=[]; maps=[]
    for did,addr,size,_ in portal.entries():
        if did>>24==0x47:
            try:
                r=Reader(portal.chain(addr,size)); e=entity(r,types); assert r.pos==size
                templates[f'{did:08X}']=e
            except (AssertionError,KeyError,ValueError) as ex: rejected.append(f'{did:08X}:{ex}')
        if did>>24==0x28:
            try:
                r=Reader(portal.chain(addr,size)); r.word(); ids={}; labels={}
                for section in range(4):
                    for _ in range(r.word()&65535):
                        k=r.word(); v=r.string() if section%2 else r.word()
                        (labels if section%2 else ids)[k]=v
                for k,v in ids.items(): maps.append({'map':f'{did:08X}','key':k,'id':f'{v:08X}','name':labels.get(k,'')})
            except (AssertionError,KeyError,ValueError) as ex: rejected.append(f'map {did:08X}:{ex}')
    placed=[]; blocks=0; interior_ids=[]
    for did,addr,size,_ in cell.entries():
        if did&65535!=65534: continue
        blocks+=1
        try:
            r=Reader(cell.chain(addr,size)); bits=r.word(); assert r.word()==did
            if bits&1: r.word()
            cells=r.word(); r.take(8)
            interior_ids.extend((did&0xffff0000)| (0x100+i) for i in range(cells))
            for i in range(r.word()):
                e=entity(r,types); e.update(block=f'{did>>16:04X}',ordinal=i); placed.append(e)
        except (AssertionError,KeyError,ValueError) as ex: rejected.append(f'block {did:08X}:{ex}')
    interiors=[]
    for did in interior_ids:
        try:
            r=Reader(cell.read(did)); assert r.word()==did
            r.take(28); flags=r.word(); r.word()
            for _ in range(r.word()):
                bits=struct.unpack('<H',r.take(2))[0]; r.take(6)
                if bits&4:
                    r.take(struct.unpack('<H',r.take(2))[0]*2);r.align()
            r.align()
            for _ in range(2): r.take(r.word()*2);r.align()
            if flags&2:
                r.word()
                for i in range(r.word()):
                    e=entity(r,types);e.update(cell=f'{did:08X}',ordinal=i);interiors.append(e)
        except (AssertionError,KeyError,ValueError) as ex:rejected.append(f'cell {did:08X}:{ex}')
    def chain(e):
        seen=set(); result=[]
        while e:
            result.append(e); v=e['data']
            if v in seen: break
            seen.add(v);e=templates.get(f'{v:08X}')
        return result
    creature_placements=[e for e in placed+interiors if any(z['type']==0x40000002 for z in chain(e))]
    catalogue=[]
    for did,e in templates.items():
        props={}
        for z in reversed(chain(e)): props.update(z.get('properties',{}))
        byname={names.get(int(k[9:],16),k):v for k,v in props.items()}
        phys=byname.get('PhysObj',0); visual=chain(templates.get(f'{phys:08X}',{'data':phys}))[-1]['data']
        if visual>>24!=0x1f: continue
        ai=byname.get('AI_NPCTable','')
        if ai:
            catalogue.append({'template':did,'visual':f'{visual:08X}','ai_table':ai,'physics_entity':f'{phys:08X}'})
    encounter=Reader(portal.read(0x14000000)); encounter_version=encounter.word()
    assert encounter.word()==0x14000000
    encounter_map,encounter_rows=encounter.word(),encounter.word()
    report={'template_count':len(templates),'block_count':blocks,'placement_count':len(placed),
            'encounter_descriptor':{'id':'14000000','version':encounter_version,'map':encounter_map,'rows':encounter_rows},
            'interior_cells':len(interior_ids),'interior_placement_count':len(interiors),
            'known_creature_placements':creature_placements,'creature_catalogue':catalogue,
            'template_types':dict(collections.Counter(f'{e["type"]:08X}' for e in templates.values())),
            'placement_types':dict(collections.Counter(f'{e["type"]:08X}' for e in placed)),
            'templates':templates,'placements':placed,'interior_placements':interiors,'id_maps':maps,'property_names':names,'rejected':rejected}
    a.output.parent.mkdir(parents=True,exist_ok=True); a.output.write_text(json.dumps(report,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k not in ('templates','placements','interior_placements','creature_catalogue','id_maps','property_names')},indent=2))
    for item in maps:
        if any(s in item['name'].lower() for s in ('drudge','shreth','reedshark','banderling','beetle')): print(item)
    print('Named templates:',[(k,e.get('name')) for k,e in templates.items() if e.get('name')][:10])

if __name__=='__main__': main()
