"""Extract public corpse/container evidence; never export player inventories.

Capture observations prove an item was present, not its probability, affixes,
generator membership, or availability in a different client version.
"""
import argparse, hashlib, json, struct, subprocess
from collections import Counter
from pathlib import Path
from capture_packets import reassemble

def audit(path, decoder):
    data=path.read_bytes(); selected=[]; objects=[]
    def consume():
        if not selected:return
        wire=b''.join(struct.pack('<I',len(m.data))+m.data for m in selected)
        result=subprocess.run([str(decoder),'--loot'],input=wire,capture_output=True,check=True,timeout=60)
        rows=result.stdout.splitlines()
        if len(rows)!=len(selected):raise ValueError('decoder row count')
        for m,row in zip(selected,rows):
            value=json.loads(row)
            if value:
                value.update(time=m.time,message_sha256=hashlib.sha256(m.data).hexdigest());objects.append(value)
        selected.clear()
    issues=Counter()
    for m in reassemble(data,issues):
        if m.data[:4]==struct.pack('<I',0x10058) and len(m.data)<=65536:
            selected.append(m)
            if len(selected)>=2048:consume()
    consume()
    return dict(sha256=hashlib.sha256(data).hexdigest(),objects=objects,issues=dict(issues))

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--manifest',type=Path,required=True);p.add_argument('--root',type=Path,required=True)
    p.add_argument('--decoder',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
    a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
    decoder_sha=hashlib.sha256(a.decoder.read_bytes()).hexdigest()
    files={v['sha256']:a.root/v['path'] for v in json.loads(a.manifest.read_text())}
    summaries=[]
    for index,(sha,path) in enumerate(files.items()):
        dest=a.output/(sha+'.json')
        try:
            report=json.loads(dest.read_text()) if dest.exists() else {}
            if report.get('format')!=2 or report.get('decoder_sha256')!=decoder_sha:
                report=audit(path,a.decoder.resolve())
                report.update(format=2,decoder_sha256=decoder_sha)
            if report['sha256']!=sha:raise ValueError('source hash mismatch')
            dest.write_text(json.dumps(report,separators=(',',':')))
            summaries.append(dict(sha256=sha,path=str(path),items=sum(bool(o['container']) for o in report['objects']),corpses=sum(o['corpse'] for o in report['objects'])))
        except Exception as e:summaries.append(dict(sha256=sha,path=str(path),error=str(e)))
        if index%25==0:print(index+1,'/',len(files),'items',sum(s.get('items',0) for s in summaries),flush=True)
    (a.output/'summary.json').write_text(json.dumps(summaries,indent=2))
    print('Done',len(files),'captures',sum(s.get('items',0) for s in summaries),'contained objects',flush=True)
if __name__=='__main__':main()
