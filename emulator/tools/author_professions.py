"""Recover profession catalog from the installed DAT, with explicit exclusions.

No recipe is inferred from its name. Indexed production recipes, native refining
classes, dyeing and current survey maps have explicit handlers. Unsupported
extraction/mutation and quest entitlements remain in the exclusion audit.
"""
import argparse
import hashlib
import json
import struct
from pathlib import Path
from craft_catalog import graph
from ac2_dat import Dat
from import_audited_population import ContentProfile, Archive, localized_name
from population_probe import Reader, entity


def recover(directory):
    profile = ContentProfile(directory)
    local = Archive(directory / 'local_English.dat')
    graphs, fingerprints = {}, {}
    names, templates = {}, {}
    reader = Reader(profile.portal.read(0x34000000)); reader.take(12)
    for _ in range(reader.word()):
        key = reader.word(); names[key] = reader.string()
    for did, *_ in profile.portal.entries():
        if did >> 24 != 0x47: continue
        try:
            reader = Reader(profile.portal.read(did)); row = entity(reader, profile.types)
            if reader.pos == len(reader.data): templates[did] = row
        except (ValueError, KeyError, AssertionError): pass
    def properties(did):
        result, seen = {}, set()
        while did in templates and did not in seen:
            seen.add(did); row = templates[did]
            for key, value in row.get('properties', {}).items():
                result.setdefault(names.get(int(key[9:],16),key),value)
            did = row['data']
        return result
    with Dat(directory / 'portal.dat') as dat:
        def get(did):
            if did not in graphs:
                raw = dat.read(did)
                graphs[did] = graph(raw, did)
                fingerprints[f'{did:08x}'] = hashlib.sha256(raw).hexdigest()
            return graphs[did]
        def text(g, ref):
            node = g[ref]
            if 'string' not in node: return node.get('text', '')
            key, table = node['string']
            value = localized_name(local, {'key': key, 'table': table})
            return '' if value == '-' else value
        def curve(did):
            g = get(did); f = g[0]['slots']
            if g[0]['package'] == 0x1e5: return g[f[0]]['list']
            if g[0]['package'] != 0x331f: raise ValueError('unsupported curve')
            return [0] * f[0] + g[f[2]]['list']
        def values(g, ref, default):
            if ref == 0xffffffff: return [default]
            return curve(g[ref]['did'])
        master = get(0x6f002ec3)
        skills = []
        for _, did in master[master[0]['slots'][1]]['pairs']:
            g = get(did); f = g[0]['slots']
            skills.append(dict(id=did, name=text(g,f[2]), description=text(g,f[4]),
                               cap=f[1], icon=f[6], xp=curve(f[5]), limit=curve(f[7]),
                               use_xp=curve(f[11]) if f[11] else []))
        index = get(0x6f002647); ids = set(index[index[0]['slots'][1]]['list'])
        # The old index predates Legions. The installed ordering table includes
        # Empyrean recipes and the variable-quantity refining replacements.
        ordering = get(0x6f002ab8)
        ids.update(k for node in ordering for k,v in node.get('pairs',[]) if k >> 24 == 0x6f)
        recipes, excluded, items = [], [], {}
        def item(did):
            if did not in items:
                visual = profile.visual(did)
                if not visual or visual >> 24 != 0x1f: raise ValueError('missing item visual')
                props = properties(did)
                items[did] = dict(id=did, visual=visual, name=localized_name(local,dict(zip(('key','table'),props['Name']))) if isinstance(props.get('Name'),list) else '',
                                  damage=props.get('Weapon_Damage',0), armor=props.get('Item_ArmorLevel',0),
                                  stack=min(65535,props.get('Item_MaxStackSize',1)),
                                  dye=struct.unpack('<f',struct.pack('<I',props['Craft_DyePlant_Mod']))[0] if 'Craft_DyePlant_Mod' in props else None)
            return did
        for did in sorted(set(ids) | {0x6f001f17, 0x6f001f18,0x6f005249,0x6f001f0e}):
            name = f'{did:08x}'
            try:
                g = get(did); f = g[0]['slots']; name = text(g,f[8])
                kind = 'produce'
                refining = {0x36a4:0x47002ad5,0x36a5:0x470027a4,0x36a7:0x470027a5,0x36a8:0x470027a6,0x36a9:0x470027a8}
                if g[0]['package'] in refining: kind = 'refine'
                elif g[0]['package'] == 0x3738: kind = 'dye'
                elif g[0]['package'] != 0x1d7: raise ValueError('specialized mutation handler')
                if f[11] == 0x6f00264a:
                    if not 0x6f002aef <= did <= 0x6f002afd: raise ValueError('historical or specialized survey')
                    kind = 'survey'
                if f[11] == 0x6f0028d7 and did not in (0x6f001f17,0x6f001f18,0x6f001f0e):
                    raise ValueError('quest entitlement required')
                if f[11] not in {s['id'] for s in skills}: raise ValueError('unknown profession')
                ingredients = []
                for ref in g[f[7]]['list']:
                    fields = g[ref]['slots']
                    choices = sorted(set(k for k,v in g[fields[6]]['pairs'] if k == v))
                    if kind == 'dye':
                        race = {0x6f0023a8:1,0x6f0023aa:2,0x6f0023a9:4,0x6f005242:16}[f[11]]
                        choices = sorted(d for d in templates if (p:=properties(d)).get('Craft_DyePlant_Mod') is not None) if fields[0] == 2 else sorted(d for d in templates if (p:=properties(d)).get('Item_WornAppearanceRace1') == race and p.get('Item_PreferredInventoryLocation') in (1,2,4,8,16,32) and p.get('Item_PileAppearance'))
                    if not choices or any(d >> 24 != 0x47 for d in choices): raise ValueError('abstract ingredient predicate')
                    for d in choices: item(d)
                    quantities = values(g,fields[18],fields[12])
                    if max(quantities) > 65535: raise ValueError('ingredient exceeds inventory stack limit')
                    ingredient_name = text(g,fields[1])
                    if did == 0x6f001f0e and not ingredient_name:
                        ingredient_name = items[choices[0]]['name']
                    ingredients.append(dict(ordinal=fields[0], name=ingredient_name, choices=choices,
                                            minimum=fields[4], quantities=quantities))
                if not (kind == 'survey' and not ingredients) and not 1 <= len(ingredients) <= 16: raise ValueError('harvesting or empty ingredients')
                branch = dict(g[f[5]]['pairs']).get(0x40000000)
                if branch is None: raise ValueError('no normal success branch')
                outcome = g[branch]['slots']; outputs = []
                for threshold, ref in sorted(g[outcome[2]]['pairs'], key=lambda kv: kv[0]):
                    if kind == 'dye': break
                    actions = [g[i] for i in g[g[ref]['slots'][0]]['list']]
                    produce = [a for a in actions if a.get('package') == 0x144]
                    if kind == 'refine': definition = item(refining[g[0]['package']])
                    else:
                        if len(produce) != 1: raise ValueError('non-item or multiple-item outcome')
                        definition = item(produce[0]['slots'][2])
                    outputs.append(dict(threshold=struct.unpack('<f',struct.pack('<I',threshold))[0],item=definition))
                if not outputs and kind != 'dye': raise ValueError('empty outcomes')
                workshop = g[f[31]].get('did',0) if f[31] != 0xffffffff else 0
                recipes.append(dict(id=did, skill=f[11], name=name, description=text(g,f[9]), kind=kind,workshop=workshop,
                                    minimum=f[13], maximum=f[21], difficulty=values(g,f[3],f[29]),
                                    cost=values(g,f[1],f[34]), ingredients=ingredients, outputs=outputs,
                                    icon=f[32]))
            except (ValueError, KeyError, IndexError, AssertionError) as e:
                excluded.append(dict(id=did, name=name, reason=str(e)))
        workshops = [dict(id=d,effect=p['ForgeEffect'],radius=struct.unpack('<f',struct.pack('<I',p['ForgeEffectRadius']))[0])
                     for d in templates if (p:=properties(d)).get('ForgeEffect') and p.get('ForgeEffectRadius')]
        chance = get(0x6f002fb9)
        success = [struct.unpack('<f',struct.pack('<I',w))[0] for w in chance[chance[0]['slots'][0]]['list']]
        # Native mines retain material, per-continent yield, variance and charges.
        mines=[]
        for d in templates:
            p=properties(d)
            if p.get('Mine_Object') and p.get('Mine_MaxUses') and profile.visual(d):
                mines.append(dict(id=d,visual=profile.visual(d),material=item(p['Mine_Object']),
                                  quantity=p['Mine_ObjectQuantity'],uses=p['Mine_MaxUses'],
                                  variance=struct.unpack('<f',struct.pack('<I',p['Mine_ObjectQuantityVariance']))[0]))
        maps=[]
        def xy(cell):
            c=(cell&65535)-1
            return [(cell>>24)*160+(c>>3)*20+10,((cell>>16)&255)*160+(c&7)*20+10]
        materials={'Crystal':0x470010f2,'Iron':0x470010f3,'Silver':0x470010fe,'Stone':0x470010f4,'Wood':0x470010f5}
        for r in recipes:
            if r['kind']!='survey':continue
            for out in r['outputs']:
                d=out['item'];p=properties(d);effect=p['Usage_Effect2'].split('_')
                region=['Osteth','Omishan','Linvak'].index(effect[-1]);material=materials[effect[-2]]
                lo=xy(struct.unpack('<I',bytes.fromhex(p['Usage_RequiredLocationTsysMin'])[:4])[0]);hi=xy(struct.unpack('<I',bytes.fromhex(p['Usage_RequiredLocationTsysMax'])[:4])[0])
                mine=next(m for m in mines if m['material']==material and m['quantity']==25+5*region and m['id']<0x47003000)
                maps.append(dict(id=d,region=region,mine=mine['id'],bounds=[min(lo[0],hi[0]),min(lo[1],hi[1]),max(lo[0],hi[0]),max(lo[1],hi[1])],radius=40))
        tools=[]
        for d,skill in [(0x47002de7,0x6f0025a9),(0x47002de8,0x6f002560),(0x47002de4,0x6f0025a8),(0x47002dea,0x6f0025a6),(0x47002deb,0x6f002579),(0x47002dec,0x6f0025a7)]:
            p=properties(d);item(d)
            tools.append(dict(id=d,skill=skill,durability=p.get('Item_Durability_MaxLevel',20),animation=p.get('Usage_UserAnimation',0)))
        # Butchery profile identities survive, but the server yield tables do not.
        # Keep the set of eligible monsters, never treat NPCs as carcasses.
        butcher=[dict(id=d,profile=p['ButcheryProfile'],material=0x470027a0 if any(s in p['AI_NPCTable'] for s in ('Beetle','Wasp','Grut','Rachis')) else 0x470027a3)
                 for d in templates if (p:=properties(d)).get('ButcheryProfile') and p.get('AI_NPCTable')]
        for d in [0x470027a0,0x470027a3,0x4700141f,0x47001400,0x47002ad4,0x470010fa,0x470027a2]:item(d)
        catalog = dict(version=1, skills=skills, recipes=recipes, items=list(items.values()),workshops=workshops,success=success,mines=mines,maps=maps,tools=tools,butcher=butcher)
        audit = dict(version=1, sources=fingerprints, excluded=excluded,
                     policy='Native identities, curves, ingredients and success table. Entitlement, manufacture XP, refining yield and item stat scaling are reconstructed; see content README.')
        return catalog, audit


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directory',type=Path); p.add_argument('--output',type=Path,default=Path(__file__).resolve().parents[1]/'content')
    a=p.parse_args(); catalog,audit=recover(a.directory)
    a.output.mkdir(parents=True,exist_ok=True)
    for name,value in [('professions-v1.json',catalog),('professions-v1.audit.json',audit)]:
        (a.output/name).write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:len(v) for k,v in catalog.items() if isinstance(v,list)}))
    from collections import Counter
    print(dict(Counter(e['reason'] for e in audit['excluded'])))
