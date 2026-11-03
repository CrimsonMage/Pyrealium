"""Recover corpse item bundles with native identity and source provenance.

These are observed bundles, not original probabilities or randomized affixes.
Unknown/ambiguous/version-incompatible items are withheld, never substituted.
"""
import argparse, collections, hashlib, json, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT/'Tools/AC2'),str(ROOT/'Network')]
from population_probe import Reader, entity
from dat_probe import Archive
from content_profile import ContentProfile
from import_audited_population import localized_name
from import_versioned_population import runtime_role, semantic_key

def item_signature(portal,did):
    """Bounded native qualities prefix: equipment/category/race identity."""
    r=Reader(portal.read(0x81000000|(did&0xffffff)));mask=r.word();r.word();desc=r.word()
    if not mask&1:raise ValueError('missing descriptor')
    for bit in (1,0x2000000,2):
        if desc&bit:r.word()
    for bit in (4,8):
        if desc&bit:
            r.take(8)
            if r.word()!=0:raise ValueError('item default has substitutions')
    for bit in (0x10,0x20,0x40,0x100,0x200,0x20000000,0x40000000,0x4000000,0x80,0x800,0x1000,0x2000,0x200000,0x400000,0x8000,0x10000,0x20000,0x40000,0x80000,0x100000,0x8000000,0x10000000):
        if desc&bit:r.take(8 if bit in (0x20,0x40,0x100,0x200,0x20000000,0x40000000,0x4000000) else 4)
    ints={}
    if mask&2:
        count=r.word()&0xffffff
        if count>512:raise ValueError('quality count')
        for _ in range(count):key=r.word();ints[key]=r.word()
    return tuple(ints.get(k) for k in (0x113,0x114,0x115,0x11a,0x2d1))

def templates(directory):
    profile=ContentProfile(directory);local=Archive(directory/'local_English.dat');raw={};rows={}
    for did,*_ in profile.portal.entries():
        if did>>24!=0x47:continue
        try:
            r=Reader(profile.portal.read(did));v=entity(r,profile.types)
            if r.pos==len(r.data):raw[did]=v
        except (ValueError,KeyError,AssertionError):pass
    for did in raw:
        props={};cur=did;seen=set()
        while cur in raw and cur not in seen:
            seen.add(cur);r=raw[cur]
            for k,v in r.get('properties',{}).items():props.setdefault(int(k[9:],16),v)
            cur=r['data']
        name=props.get(0x41000143)
        if not isinstance(name,list) or len(name)!=2:
            try:
                q=Reader(profile.portal.read(0x81000000|(did&0xffffff)))
                mask=q.word();q.word();desc=q.word()
                if not mask&1 or not desc&4:continue
                for bit in (1,0x2000000,2):
                    if desc&bit:q.word()
                name=[q.word(),q.word()]
                if q.word()!=0:continue # No fabricated substitution/default name.
            except (KeyError,ValueError,AssertionError):continue
        name=localized_name(local,dict(key=name[0],table=name[1]))
        visual=profile.visual(did)
        if name=='-' or not visual:continue
        role=('vendor' if props.get(0x410002de) or props.get(0x410002ec) else
              'monster' if props.get(0x4100016e,0)>0 and props.get(0x41000184,0)>0 and props.get(0x4100018a,0)!=0 else 'scripted')
        try:signature=item_signature(profile.portal,did)
        except (KeyError,ValueError,AssertionError):signature=None
        rows[did]=dict(name=name,visual=visual,base=cur,props=props,ai=props.get(0x41000189),role=role,signature=signature)
    return profile,local,rows

def convert(audit,reference,installed):
    sp,sl,source=templates(reference);tp,tl,target=templates(installed)
    creatures=collections.defaultdict(list);items=collections.defaultdict(list);mapped={};skipped=collections.Counter()
    # Match localized identity and native equipment/type/race qualities, never
    # a shared numeric DID across versions. Each version supplies its own visual.
    for did,r in target.items():
        if r['ai']:creatures[semantic_key(r)].append(did)
        elif r['signature'] is not None:items[(r['name'],r['signature'])].append(did)
    for did,r in source.items():
        if r['ai']:
            options=creatures[semantic_key(r)]
        else:
            options=items[(r['name'],r['signature'])] if r['signature'] is not None else []
        if len(options)==1:mapped[did]=options[0]
    source_monsters=collections.defaultdict(list)
    for did,r in source.items():
        dest=mapped.get(did)
        if dest and r['ai'] and runtime_role(dest,target[dest])=='monster':
            source_monsters[(r['name'],r['visual'])].append(did)
    bundles=collections.defaultdict(dict);proof=[];definitions={};dedup=set()
    for file in sorted(audit.glob('*.json')):
        if len(file.stem)!=64:continue
        report=json.loads(file.read_text());objects=report.get('objects',[])
        corpses={o['source_iid']:o for o in objects if o['corpse']}
        contents=collections.defaultdict(dict)
        for o in objects:
            if o['container'] in corpses:contents[o['container']][o['source_iid']]=o
        for iid,entries in contents.items():
            corpse=corpses[iid];children=(corpse.get('name') or {}).get('children',[])
            if len(children)!=1:skipped['corpse name not one native template']+=1;continue
            name=localized_name(sl,children[0]);options=source_monsters[(name,corpse['visual'])]
            destinations={mapped[d] for d in options}
            if len(destinations)!=1:skipped['corpse has no unique compatible monster']+=1;continue
            monster=next(iter(destinations));resolved=[];failed=False
            # Do not overcount the same corpse copied between capture archives.
            fingerprint=(corpse['message_sha256'],tuple(sorted(o['message_sha256'] for o in entries.values())))
            if fingerprint in dedup:continue
            dedup.add(fingerprint)
            for o in entries.values():
                src=source.get(o['entity']);dest=mapped.get(o['entity']);row=target.get(dest)
                if not src or src['ai'] or not row or src['visual']!=o['visual']:
                    skipped['unverified item identity/visual']+=1;failed=True;continue
                # Only complete bundles can be used; silently truncating a bag
                # would turn an unverified observation into a different drop.
                quantity=o.get('quantity')
                if quantity is None:quantity=1
                if not 1<=quantity<=32:skipped['unsupported item quantity']+=1;failed=True;continue
                resolved.extend([dest]*quantity);definitions[dest]=row
            if failed or not 1<=len(resolved)<=32:continue
            key=tuple(sorted(resolved));bundles[monster][key]=bundles[monster].get(key,0)+1
            proof.append(dict(source_sha256=report['sha256'],corpse_message_sha256=corpse['message_sha256'],
                              monster=f'{monster:08x}',items=[f'{d:08x}' for d in key]))
    # Keep identifiers/data only; use runtime metadata validation before enabling.
    lines=['pyrealium-observed-loot-v1','# monster profile observed_count item_definitions (one complete observed bundle)']
    for monster,choices in sorted(bundles.items()):
        profile=target[monster]['props'].get(0x410001a6,0)
        for choice,count in sorted(choices.items()):
            lines.append(f'{monster:08x}\t{profile}\t{count}\t'+','.join(f'{did:08x}' for did in choice))
    used={did for choices in bundles.values() for choice in choices for did in choice}
    itemlines=['pyrealium-loot-items-v1','# definition visual name; native metadata is validated at server startup']
    for did in sorted(used):itemlines.append(f'{did:08x}\t{definitions[did]["visual"]:08x}\t{definitions[did]["name"]}')
    report=dict(monsters=len(bundles),bundles=sum(map(len,bundles.values())),items=len(used),skipped=dict(skipped),evidence=proof,
        limitation='Observed base items only. Original generation weights, affixes, quality rolls and unobserved tables are unavailable. Runtime rejects unsupported metadata; no potion replacement.',
        dat_sha256={label:{n:hashlib.sha256((d/n).read_bytes()).hexdigest() for n in ['portal.dat','local_English.dat']}
                    for label,d in [('reference',reference),('installed',installed)]})
    sp.close();sl.file.close();tp.close();tl.file.close()
    return '\n'.join(lines)+'\n','\n'.join(itemlines)+'\n',report

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--audit',type=Path,required=True)
    p.add_argument('--reference',type=Path,required=True);p.add_argument('--installed',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    tables,items,report=convert(a.audit,a.reference,a.installed)
    a.output.mkdir(parents=True,exist_ok=True)
    (a.output/'observed-loot.tsv').write_text(tables,encoding='utf-8')
    (a.output/'loot-items.tsv').write_text(items,encoding='utf-8')
    (a.output/'observed-loot.report.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k not in ('evidence','dat_sha256')},indent=2))
if __name__=='__main__':main()
