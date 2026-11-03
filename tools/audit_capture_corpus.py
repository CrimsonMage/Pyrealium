"""Audit AC2 captures into sanitized, provenance-linked world observations.

Raw inputs and decoder intermediates stay local. No chat, login credentials,
player names, IP addresses or private inventory payloads are exported. This does
not infer spawn schedules, quest completion rules or loot probabilities.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import struct
import subprocess
from capture_packets import reassemble

MAGICS = {b'\xd4\xc3\xb2\xa1',b'\xa1\xb2\xc3\xd4',b'\x4d\x3c\xb2\xa1',b'\xa1\xb2\x3c\x4d',b'\x0a\x0d\x0d\x0a'}
OPCODES = {0x10058,0x1007f,0x10082,0x10083,0x10086,0x10087,0x100a0,0x100a2,0x100a8,0x100a9,0x100be,0x100c2,0x100cb,
           0x10066,0x10067,0x100c4,0x100ca}
AUDIT_VERSION = 5


def discover(roots):
    found = {}
    for root in roots:
        for path in sorted(root.rglob('*')):
            if not path.is_file() or '.svn' in path.parts: continue
            with path.open('rb') as stream:
                if stream.read(4) not in MAGICS: continue
                stream.seek(0); sha = hashlib.file_digest(stream,'sha256').hexdigest()
            entry = found.setdefault(sha,{'sha256':sha,'bytes':path.stat().st_size,'paths':[],'file':path})
            entry['paths'].append(path.relative_to(root).as_posix())
    return sorted(found.values(),key=lambda e:e['sha256'])


def audit(source, decoder):
    if source['bytes'] > 128*1024**2: raise ValueError('capture exceeds 128 MiB')
    raw = source['file'].read_bytes()
    counts, errors, scripts, issues = Counter(), Counter(), Counter(), Counter()
    actors, events, attached = {}, defaultdict(Counter), set()
    selected, duration = [], [None,None]
    def consume():
        if not selected: return
        encoded = b''.join(struct.pack('<I',len(m.data))+m.data for m in selected)
        run = subprocess.run([str(decoder)],input=encoded,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True,timeout=60)
        decoded = run.stdout.splitlines()
        if len(decoded) != len(selected): raise ValueError('decoder output count mismatch')
        for message, line in zip(selected,decoded):
            value = json.loads(line)
            if not value: continue
            if 'error' in value: errors[value['error']] += 1; continue
            if 'excluded' in value: errors[value['excluded']] += 1; continue
            kind, iid = value['type'], value.get('source_iid')
            if kind == 'create':
                if len(actors) >= 65536: raise ValueError('capture object limit')
                if iid not in actors:
                    value.update({'first_seen_seconds':None if message.time is None or duration[0] is None else round(message.time-duration[0],6),
                                  'message_sha256':hashlib.sha256(message.data).hexdigest()})
                    actors[iid] = value
                else:
                    initial=actors[iid]
                    if any(initial[k]!=value[k] for k in ('entity','visual','cell')) or \
                       sum((a-b)**2 for a,b in zip(initial['position'],value['position']))>0.0001:
                        events[iid][('moving',None,None,None)] += 1
            elif kind == 'attached': attached.add(iid)
            elif kind == 'position':
                initial=actors.get(iid)
                changed=initial and (initial['cell']!=value['cell'] or
                    sum((a-b)**2 for a,b in zip(initial['position'],value['position']))>0.0001)
                # A position update before CreateObject is conservatively mobile.
                # Native byte quantization maps zero to approximately -1/255.
                if not initial or changed or any(abs(v)>0.01 for v in value['motion']):events[iid][('moving',None,None,None)]+=1
            elif kind == 'script': scripts[f"{value['function']:08x}"] += 1
            else:
                key = (kind,value.get('value'),value.get('mode'),value.get('rate'))
                events[iid][key] += 1
        selected.clear()
    try:
        for message in reassemble(raw, issues):
            if message.time is not None:
                if duration[0] is None: duration[0] = message.time
                duration[1] = message.time
            if len(message.data) < 4: continue
            opcode = struct.unpack_from('<I',message.data)[0]
            counts[f'{opcode:08x}'] += 1
            if opcode in OPCODES and len(message.data) <= 65536:
                if opcode==0x10086:
                    if len(message.data)<12:continue
                    iid=struct.unpack_from('<Q',message.data,4)[0]
                    if iid>>32 not in (0x81200000,0x81300000) and iid>>56!=0x40:continue
                selected.append(message)
                if len(selected) >= 4096: consume()
        consume()
    except (ValueError,struct.error) as error:
        # Broken container framing invalidates a source; isolated malformed UDP
        # packets/conflicting assemblies are quarantined by the reader instead.
        return {'sha256':source['sha256'],'paths':source['paths'],'bytes':source['bytes'],
                'error':str(error),'opcodes':dict(counts),'actors':[]}
    output=[]
    for iid, actor in actors.items():
        if iid in attached: errors['later-attached-or-contained'] += 1; continue
        actor['events'] = [{'kind':k[0],'value':k[1],'mode':k[2],'rate':k[3],'count':count} for k,count in sorted(events[iid].items(),key=str)]
        output.append(actor)
    return {'sha256':source['sha256'],'paths':source['paths'],'bytes':source['bytes'],
            'duration_seconds':None if None in duration else round(duration[1]-duration[0],3),
            'opcodes':dict(sorted(counts.items())),'scripts':dict(sorted(scripts.items())),
            'decode_rejections':dict(errors),'packet_issues':dict(issues),'actors':output}


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root',type=Path,action='append',required=True)
    p.add_argument('--decoder',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    sources=discover(args.root);reports=[];opcodes=Counter();scripts=Counter();rejected=Counter()
    decoder_hash=hashlib.sha256(args.decoder.read_bytes()).hexdigest()
    print(f'{len(sources)} unique captures, {sum(s["bytes"] for s in sources)} bytes',flush=True)
    for i,source in enumerate(sources):
        dest=args.output/(source['sha256']+'.json')
        result=json.loads(dest.read_text(encoding='utf-8')) if dest.exists() else {}
        if result.get('audit_version') != AUDIT_VERSION or result.get('decoder_sha256') != decoder_hash:
            result=audit(source,args.decoder.resolve())
        result.update({'paths':source['paths'],'audit_version':AUDIT_VERSION,'decoder_sha256':decoder_hash})
        dest.write_text(json.dumps(result,indent=2),encoding='utf-8')
        summary={k:v for k,v in result.items() if k!='actors'};summary['observations']=len(result['actors']);reports.append(summary)
        opcodes.update(result.get('opcodes',{}));scripts.update(result.get('scripts',{}));rejected.update(result.get('decode_rejections',{}))
        if i%25==0 or i==len(sources)-1:print(i+1,'/',len(sources),'observations',sum(r['observations'] for r in reports),flush=True)
    report={'captures':len(sources),'copies':sum(len(s['paths']) for s in sources),'bytes':sum(s['bytes'] for s in sources),
            'observations':sum(r['observations'] for r in reports),'opcodes':dict(opcodes),'scripts':dict(scripts),
            'decode_rejections':dict(rejected),'sources':reports}
    (args.output/'summary.json').write_text(json.dumps(report,indent=2),encoding='utf-8')

if __name__=='__main__':main()
