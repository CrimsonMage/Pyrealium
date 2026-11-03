"""Export inherited native perception, home, wandering and loot bindings.

Absent properties stay absent. A label is not a server-side relationship or
generator definition. The runtime documents its scheduling/hostility defaults.
"""
import argparse,json,struct,math
from pathlib import Path

FIELDS=[('perception',0x4100017a,True),('homesick',0x41000178,True),
        ('wandering',0x4100019f,False),('wander_range',0x410001a0,True),('wander_probability',0x410001a1,True),
        ('friend_table',0x4100018a,False),('free_attacking',0x4100017f,False),('idle_only',0x41000281,False),
        ('loot_profile',0x410001a6,False),('loot_proof',0x410000d6,False)]
def export(population,entities):
    rows=population['templates'];lines=['pyrealium-creature-ai-v1','# entity '+' '.join(f[0] for f in FIELDS)]
    for did in sorted(entities):
        props={};seen=set();r=rows.get(f'{did:08X}')
        while r:
            for k,v in r.get('properties',{}).items():props.setdefault(int(k[9:],16),v)
            parent=r['data']
            if parent in seen:raise ValueError('cyclic inheritance')
            seen.add(parent);r=rows.get(f'{parent:08X}')
        values=[]
        for _,key,isfloat in FIELDS:
            value=props.get(key)
            if value is not None and isfloat:
                value=struct.unpack('<f',struct.pack('<I',value))[0]
                if not math.isfinite(value):raise ValueError('nonfinite native field')
            values.append('-' if value is None else format(value,'.9g'))
        lines.append(f'{did:08x}\t'+'\t'.join(values))
    return '\n'.join(lines)+'\n'
def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('population',type=Path);p.add_argument('combat',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    entities={int(l.split('\t')[0],16) for l in a.combat.read_text().splitlines() if l.startswith('47')}
    a.output.write_text(export(json.loads(a.population.read_text()),entities));print('Native AI rows',len(entities))
if __name__=='__main__':main()
