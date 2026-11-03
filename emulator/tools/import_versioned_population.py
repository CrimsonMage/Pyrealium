"""Translate captured population using both source and destination client DATs.

No DID substitution by number or display name alone: require an unambiguous
native localized name + AI table + role, and verify the capture's source visual.
Retain exact positions on native ground; require identical interior room records
because dungeon coordinates are local, not a shared world map. Select a single
capture per outdoor block (room indoors), and one initial five-second cohort
per cell to avoid stacking later respawns. Source data never becomes executable.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT/'Network'), str(ROOT/'Tools/AC2')]
from content_profile import ContentProfile
from population_probe import Reader, entity
from dat_probe import Archive
from import_audited_population import identity, localized_name, rejection

# Existing Pyrealium dialogue dispatch takes precedence over inherited combat data
# (Pectar uses the Drudge Mystic combat template). These are installed DIDs,
# applied only after the source entity has been semantically translated.
DIALOGUE_ENTITIES = {0x47001699, 0x47002448, *range(0x47001573, 0x47001582)}
# Installed label table 23000103: PlayerFriendly=24, SkyportServitor=79.
# These include named escort animals, a guiding mechanism and Skyport service
# NPCs with inherited combat statistics. Keep them available for scripting;
# do not turn them into generic XP/loot encounters without their quest rules.
SCRIPTED_FRIEND_TABLES = {24, 79}


class Templates:
    def __init__(self, directory):
        self.profile = ContentProfile(directory)
        self.rows = {}
        self.rejected = []
        raw = {}
        local = Archive(directory/'local_English.dat')
        try:
            for did, *_ in self.profile.portal.entries():
                if did >> 24 != 0x47: continue
                try:
                    r = Reader(self.profile.portal.read(did)); raw[did] = entity(r, self.profile.types)
                    if r.pos != len(r.data): raise ValueError('trailing template data')
                except (ValueError, AssertionError, KeyError):
                    raw.pop(did, None); self.rejected.append(f'{did:08x}')
            for did in raw:
                props, seen, current = {}, set(), did
                while current in raw and current not in seen:
                    seen.add(current); record = raw[current]
                    for key, value in record.get('properties', {}).items():
                        props.setdefault(int(key[9:], 16), value)
                    current = record['data']
                if current in seen: continue
                ai, name = props.get(0x41000189), props.get(0x41000143)
                if not ai or not isinstance(name, list) or len(name) != 2: continue
                name = localized_name(local, dict(key=name[0], table=name[1]))
                visual = self.profile.visual(did)
                if name == '-' or not visual: continue
                role = ('vendor' if props.get(0x410002de) or props.get(0x410002ec) else
                        'monster' if props.get(0x4100016e, 0) > 0 and props.get(0x41000184, 0) > 0
                        and props.get(0x4100018a, 0) != 0 else 'scripted')
                self.rows[did] = dict(name=name, ai=ai, visual=visual, role=role, props=props)
        finally:
            local.file.close()

    def close(self): self.profile.close()


def semantic_key(row): return row['name'], row['ai'], row['role']


def runtime_role(did, row):
    if did in DIALOGUE_ENTITIES or row['props'].get(0x4100018a) in SCRIPTED_FRIEND_TABLES:
        return 'vendor' if row['role'] == 'vendor' else 'scripted'
    return row['role']


def file_sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def mappings(source, target):
    index = defaultdict(list)
    for did, row in target.items(): index[semantic_key(row)].append(did)
    return {did: index[semantic_key(row)][0] for did, row in source.items()
            if len(index[semantic_key(row)]) == 1}


def cohorts(candidates):
    """One capture per block; never union repeated journeys or respawn cohorts."""
    groups = defaultdict(lambda: defaultdict(lambda: defaultdict(list)))
    for a in candidates:
        cell = a['cell']; region = cell & 0xffff0000 if 1 <= cell & 65535 <= 64 else cell
        groups[region][a['source_sha256']][cell].append(a)
    result = []
    for region, sources in sorted(groups.items()):
        snapshots = {}
        for sha, cells in sources.items():
            rows = []
            for cell, actors in sorted(cells.items()):
                actors = sorted(actors, key=lambda a: (a['first_seen_seconds'], a['source_iid']))
                start = actors[0]['first_seen_seconds']
                seen = set()
                for actor in actors:
                    if actor['first_seen_seconds'] > start + 5: break
                    key = actor['entity'], *(round(v, 2) for v in actor['position'])
                    if key not in seen:
                        seen.add(key); rows.append(actor)
                    if len(seen) >= 64: break
            snapshots[sha] = rows
        sha = min(snapshots, key=lambda s: (-len(snapshots[s]), s))
        result.extend(snapshots[sha])
    return result


def convert(audit, reference, installed, protected, ground=None):
    source, target = Templates(reference), Templates(installed)
    src_cells, dst_cells = Archive(reference/'cell_1.dat'), Archive(installed/'cell_1.dat')
    mapped = mappings(source.rows, target.rows)
    covered = set()
    for path in protected:
        for line in path.read_text(encoding='utf-8').splitlines()[1:]:
            if line and not line.startswith('#'): covered.add(int(line.split('\t')[3], 16))
    checks, skipped, candidates, sources = {}, Counter(), [], {}
    try:
        for path in sorted(audit.glob('*.json')):
            if len(path.stem) != 64: continue
            report = json.loads(path.read_text(encoding='utf-8'))
            if report.get('audit_version') != 5 or report['sha256'] != path.stem:
                raise ValueError('Unsupported or mismatched sanitized capture audit')
            if report.get('error'): continue
            for a in report['actors']:
                en = a.get('entity')
                dest = mapped.get(en)
                if dest is None: skipped['no unambiguous native semantic mapping'] += 1; continue
                if source.rows[en]['visual'] != a['visual']:
                    skipped['capture does not match reference client visual'] += 1; continue
                reason = rejection(a)
                row = target.rows[dest]
                role = runtime_role(dest, row)
                # A later wander or combat action cannot invalidate a healthy,
                # stationary initial creature observation. Other actors retain
                # the strict no-movement rule (transported/temporary services).
                if reason and not (reason == 'moving object in this capture' and role == 'monster'):
                    skipped[reason] += 1; continue
                if a['cell'] in covered: skipped['protected curated/training cell'] += 1; continue
                if a.get('first_seen_seconds') is None: skipped['missing observation time'] += 1; continue
                cell = a['cell']; record = cell | 65535 if cell & 65535 <= 64 else cell
                if record not in checks:
                    try:
                        before, after = src_cells.read(record), dst_cells.read(record)
                        checks[record] = dict(installed=hashlib.sha256(after).hexdigest(),
                                              reference=hashlib.sha256(before).hexdigest(), identical=before==after)
                    except (KeyError, ValueError): checks[record] = None
                if not checks[record] or (cell & 65535 > 64 and not checks[record]['identical']):
                    skipped['missing terrain or changed interior room'] += 1; continue
                iid = identity(path.stem, a['source_iid'])
                actor = dict(a, source_sha256=path.stem, source_entity=en, source_visual=a['visual'],
                             entity=dest, visual=row['visual'], name=row['name'], role=role, ai=row['ai'],
                             id=iid, cell_record_sha256=checks[record])
                candidates.append(actor)
                sources[path.stem] = dict(bytes=report['bytes'], decoder_sha256=report['decoder_sha256'])
        selected = cohorts(candidates)
        if ground is not None:
            validated = []
            for a in selected:
                # A probe result is tied to the full proposed row, not just an
                # IID. Stale verification cannot approve edited coordinates.
                line = manifest_row(a)
                if ground.get(a['id']) != hashlib.sha256(line.encode()).hexdigest():
                    skipped['native ground contact not verified'] += 1
                else: validated.append(a)
            selected = validated
        lines = ['pyrealium-world-v4', '# Cross-version capture population; native names/AI/roles verified; captured coordinates unchanged.']
        lines.extend(manifest_row(a) for a in selected)
        proof = [{k: a[k] for k in ('id', 'source_iid', 'source_sha256', 'message_sha256', 'first_seen_seconds',
                  'source_entity', 'source_visual', 'entity', 'visual', 'cell', 'name', 'role', 'ai', 'cell_record_sha256')}
                 for a in selected]
        report = dict(schema=1, policy=__doc__, placements=len(selected), roles=dict(Counter(a['role'] for a in selected)),
                      outdoor_monsters=sum(a['role']=='monster' and a['cell']&65535<=64 for a in selected),
                      native_ground_verified=ground is not None, rejected=dict(skipped), sources=sources,
                      actors=proof, combat_entities=sorted({f'{a["entity"]:08x}' for a in selected if a['role']=='monster'}),
                      protected_entities=sorted({f'{a["entity"]:08x}' for a in selected if a['role']!='monster'}),
                      mapping_count=len(mapped), template_rejections=dict(source=source.rejected, target=target.rejected),
                      protected_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in protected},
                      dat_sha256={label:{name:file_sha256(directory/name)
                                      for name in ('portal.dat','local_English.dat','cell_1.dat')}
                                  for label,directory in [('reference',reference),('installed',installed)]})
        text = '\n'.join(lines)+'\n'
        report['manifest_sha256'] = hashlib.sha256(text.encode()).hexdigest()
        return text, report
    finally:
        source.close(); target.close(); src_cells.file.close(); dst_cells.file.close()


def manifest_row(a):
    row = '\t'.join([a['id'], *(f'{a[k]:08x}' for k in ('entity','visual','cell')),
                    *(format(v,'.9g') for v in a['position']+a['orientation']),
                    '-' if a['scale'] is None else format(a['scale'],'.9g'), a['name']])
    if len(row.encode()) > 512: raise ValueError('Population row too long')
    return row


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('audit','reference','installed','output','report'): p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--protected',type=Path,action='append',required=True)
    p.add_argument('--ground',type=Path,help='Native population-ground probe JSON; omit only to generate a probe candidate file.')
    p.add_argument('--protected-entities-output',type=Path)
    a=p.parse_args(); ground=json.loads(a.ground.read_text()) if a.ground else None
    if a.protected_entities_output and ground is None:
        p.error('Protection catalogue requires verified population')
    text, report=convert(a.audit,a.reference,a.installed,a.protected,ground)
    a.output.write_text(text,encoding='utf-8',newline='\n');a.report.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    if a.protected_entities_output:
        a.protected_entities_output.write_text('# Native noncombat/service identities from recovered-population.report.json.\n'
                                              +'\n'.join(report['protected_entities'])+'\n',encoding='utf-8',newline='\n')
    print(json.dumps({k:report[k] for k in ('placements','roles','outdoor_monsters','native_ground_verified','rejected')},indent=2))

if __name__=='__main__':main()
