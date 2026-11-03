"""Build a conservative world observation pack from sanitized capture audits.

Positions are evidence of presence, not proof of a retail spawn schedule. Keep
existing cell snapshots; select one capture for each new cell. Never resuscitate
looted/dead observations or guess a model from a different client build.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import struct
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT/'Network'), str(ROOT/'Tools/AC2')]
from content_profile import ContentProfile
from dat_probe import Archive
from population_probe import Reader

PEACE, CLOSED = 0x40000001, 0x41000003
TRANSIENT = {0x41000004: 'looted', 0x41000005: 'dead'}


def identity(sha, iid):
    digest = hashlib.sha256(bytes.fromhex(sha)+bytes.fromhex(iid)).digest()
    return f'{0x8140800000000000 | (int.from_bytes(digest[:6],"big")&0x7fffffffffff):016x}'


def rejection(actor):
    if actor.get('visual_mask') != 2 or actor.get('visual_scale') is not None:
        return 'custom visual requires unsupported overrides'
    if actor.get('mode') not in (PEACE, CLOSED): return 'not a neutral or closed initial state'
    if actor.get('behaviors') or any(actor.get('velocity') or []): return 'active initial movement or behavior'
    if any(e['kind']=='moving' for e in actor.get('events',[])): return 'moving object in this capture'
    cell, entity, visual = (actor.get(k) or 0 for k in ('cell','entity','visual'))
    if entity >> 24 != 0x47 or visual >> 24 != 0x1f: return 'unsupported definition type'
    if not (1 <= cell&65535 <= 64 or 256 <= cell&65535 <= 65531): return 'reserved cell'
    p, q = actor['position'], actor['orientation']
    if len(p)!=3 or len(q)!=4 or any(not math.isfinite(v) or abs(v)>=1_000_000 for v in p+q):
        return 'invalid position'
    if abs(sum(v*v for v in q)-1)>0.01: return 'invalid orientation'
    scale=actor.get('scale')
    if scale is not None and (not math.isfinite(scale) or not 0<scale<=100): return 'invalid scale'
    return None


def select_cells(candidates, covered):
    """Most complete admissible snapshot per cell; never merge object lifetimes."""
    groups=defaultdict(lambda:defaultdict(list))
    for actor in candidates:
        if actor['cell'] not in covered:
            groups[actor['cell']][actor['source_sha256']].append(actor)
    selected=[]
    for cell, sources in sorted(groups.items()):
        unique={sha:{(a['entity'],*(round(v,2) for v in a['position'])):a for a in actors}
                for sha,actors in sources.items()}
        sha=min(unique,key=lambda s:(-len(unique[s]),s))
        selected.extend(sorted(unique[sha].values(),key=lambda a:a['source_iid'])[:64])
    return selected


def localized_name(local, info, depth=0):
    # Literal names can describe player-created items. Publish only installed
    # localization records with no substitutions, never captured literal text.
    if not info or info.get('variables') or info.get('literal') or depth>8: return '-'
    table, key = info.get('table') or 0, info.get('key') or 0
    if not table or not key: return '-'
    try:
        r=Reader(local.read(table))
        if r.word()!=table: return '-'
        r.take(12);r.align();count=r.word()
        if count>65536:return '-'
        for _ in range(count):
            k,private=r.word(),r.word();header=struct.unpack('<H',r.take(2))[0];segments=[]
            for _ in range(header&32767):
                present=struct.unpack('<H',r.take(2))[0];r.align()
                segments.append(r.string(2) if present else '')
            r.align();variables=struct.unpack('<H',r.take(2))[0];r.take(variables*4);r.align()
            if header&32768:
                n=r.word()
                if n>65536:return '-'
                for _ in range(n):r.string(2)
                r.align()
            if k==key and not variables:
                name=''.join(segments)
                if not name and private and private!=table:
                    return localized_name(local,{'table':private,'key':key},depth+1)
                name=re.sub(r'(\[[^\[\]]{1,6}\])+$','',name).strip()
                if not name or len(name.encode('utf-8'))>128 or any(ord(c)<32 for c in name):return '-'
                return name
    except (KeyError,ValueError,AssertionError,struct.error):pass
    return '-'


def convert(audit_directory, bases, game_data):
    covered, base_rows = set(), {}
    for path in bases:
        for line in path.read_text(encoding='utf-8').splitlines()[1:]:
            if not line or line.startswith('#'):continue
            f=line.split('\t');covered.add(int(f[3],16));base_rows.setdefault(f[0],(path.name,line,f))
    profile=ContentProfile(game_data);cells=Archive(game_data/'cell_1.dat');local=Archive(game_data/'local_English.dat')
    candidates=[];skipped=Counter();exclusions={};backing=set();sources=[]
    try:
        for path in sorted(audit_directory.glob('*.json')):
            if not re.fullmatch('[0-9a-f]{64}',path.stem):continue
            source=json.loads(path.read_text(encoding='utf-8'))
            if source.get('audit_version')!=5:raise ValueError('Rebuild source audit with current decoder')
            sha=source['sha256']
            if sha!=path.stem:raise ValueError('Audit source hash mismatch')
            sources.append({'sha256':sha,'bytes':source['bytes'],'observations':len(source['actors']),
                            'packet_issues':source.get('packet_issues',{}),'error':source.get('error')})
            for actor in source['actors']:
                iid=identity(sha,actor['source_iid'])
                old=base_rows.get(iid)
                if old and old[0].startswith('capture-spawns') and actor.get('mode') in TRANSIENT:
                    name,line,f=old
                    if [int(v,16) for v in f[1:4]]==[actor.get(k) for k in ('entity','visual','cell')] and \
                       all(abs(float(x)-y)<0.001 for x,y in zip(f[4:11],actor['position']+actor['orientation'])):
                        exclusions[iid]={'id':iid,'source':name,'row_sha256':hashlib.sha256(line.encode()).hexdigest(),
                            'reason':'initial captured state is '+TRANSIENT[actor['mode']],
                            'initial_mode':f"{actor['mode']:08x}",'capture_sha256':sha,
                            'source_iid':actor['source_iid'],'message_sha256':actor['message_sha256']}
                reason=rejection(actor)
                if not reason and actor['cell'] in covered:reason='existing cell snapshot'
                if not reason and profile.visual(actor['entity'])!=actor['visual']:reason='installed DAT visual mismatch'
                if reason:skipped[reason]+=1;continue
                cell=actor['cell'];record=cell|65535 if cell&65535<=64 else cell
                try:
                    for archive,did in ((profile.portal,actor['entity']),(profile.portal,actor['visual']),(cells,record)):
                        key=(id(archive),did)
                        if key not in backing:archive.read(did);backing.add(key)
                except (KeyError,ValueError):skipped['missing backing DAT record']+=1;continue
                actor.update({'source_sha256':sha,'id':iid,'display_name':localized_name(local,actor.get('name'))})
                candidates.append(actor)
        selected=select_cells(candidates,covered)
    finally:
        profile.close();cells.file.close();local.file.close()
    lines=['pyrealium-world-v4','# Capture observations validated against installed DATs; not retail spawn schedules.']
    evidence=[];ids=set(base_rows)
    for a in selected:
        if a['id'] in ids:raise ValueError('Placement identity collision')
        ids.add(a['id'])
        line='\t'.join([a['id'],*(f'{a[k]:08x}' for k in ('entity','visual','cell')),
                        *(format(v,'.9g') for v in a['position']+a['orientation']),
                        '-' if a['scale'] is None else format(a['scale'],'.9g'),a['display_name']])
        if len(line.encode())>512:raise ValueError('Manifest row too long')
        lines.append(line)
        evidence.append({k:a[k] for k in ('id','source_iid','source_sha256','message_sha256','first_seen_seconds','entity','visual','cell','mode')})
    report={'schema':1,'capture_build_verified':False,'checksums_verified':False,
            'policy':'Installed entity/visual match; neutral or closed, no active behavior/velocity or movement events; one capture per new cell.',
            'base_sha256':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in bases},
            'placed_actors':len(selected),'cells':len({a['cell'] for a in selected}),
            'rejected':dict(sorted(skipped.items())),'candidates_not_selected':len(candidates)-len(selected),
            'actors':evidence,'sources':sources,'excluded_existing':list(exclusions.values())}
    return '\n'.join(lines)+'\n',report


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--audit',type=Path,required=True)
    p.add_argument('--base',type=Path,action='append',required=True)
    p.add_argument('--game-data',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--report',type=Path,required=True)
    a=p.parse_args();text,report=convert(a.audit,a.base,a.game_data)
    for path,value in ((a.output,text),(a.report,json.dumps(report,indent=2)+'\n')):
        path.parent.mkdir(parents=True,exist_ok=True)
        temp=path.with_name(path.name+'.tmp');temp.write_text(value,encoding='utf-8',newline='\n');temp.replace(path)
    print(f"Imported {report['placed_actors']} observations in {report['cells']} new cells; {len(report['excluded_existing'])} old transient observations identified.")

if __name__=='__main__':main()
