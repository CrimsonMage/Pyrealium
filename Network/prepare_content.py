"""Build a DAT-compatible subset of upstream placements without changing game assets."""
import shutil
import argparse
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'Tools/AC2'))
from dat_probe import Archive
from content_profile import ContentProfile, nearby_landmark


def asset_index(directory):
    """Read a complete candidate installation without modifying its archives."""
    result = {}
    for name in ('portal.dat', 'cell_1.dat'):
        archive = Archive(directory / name)
        try:
            result[name] = {entry[0] for entry in archive.entries()}
        finally:
            archive.file.close()
    return result


def missing_records(fields, index):
    entity, visual, cell = [int(v, 16) for v in fields[1:4]]
    record = cell if cell & 65535 >= 256 else cell | 65535
    return [{'kind': kind, 'archive': archive, 'record': f'{did:08x}'}
            for kind, archive, did in [('entity', 'portal.dat', entity),
                                      ('visual', 'portal.dat', visual),
                                      ('cell', 'cell_1.dat', record)]
            if did not in index[archive]]

def apply_placement_replacements(content, lines, input_rows, admitted_sources):
    """Retire audited observations only after their verified replacement passes.

    Capture IDs and subcells can change between snapshots. Do not infer unique
    NPCs from names, templates or proximity: repeated actors are valid. These
    authoring decisions pin both source rows and keep the raw packs intact.
    """
    path = content / 'placement-replacements.json'
    if not path.exists():
        return lines, [], []
    policy = json.loads(path.read_text(encoding='utf-8'))
    if policy.get('schema') != 1:
        raise ValueError('Unsupported placement replacement schema')
    rules = policy['replacements']
    retired = {rule['superseded']['id'] for rule in rules}
    retained = {rule['retained']['id'] for rule in rules}
    if len(retired) != len(rules) or retired & retained:
        raise ValueError('Duplicate or chained placement replacements')
    removed, pending = [], []
    for rule in rules:
        old, new = rule['superseded'], rule['retained']
        if new['source'] != 'recovered-population.tsv':
            raise ValueError('Placement replacement requires verified recovered population')
        rows = []
        for endpoint in (old, new):
            row = input_rows.get((endpoint['source'], endpoint['id']))
            if row is None or hashlib.sha256(row.encode()).hexdigest() != endpoint['row_sha256']:
                raise ValueError(f'Stale placement replacement evidence: {endpoint["id"]}')
            rows.append(row.split('\t'))
        if rows[0][1:3] != rows[1][1:3]:
            raise ValueError('Placement replacement must preserve entity and visual identity')
        if admitted_sources.get(old['id']) != old['source']:
            continue
        entry = dict(rule)
        if admitted_sources.get(new['id']) != new['source']:
            entry['reason'] = 'Replacement was not admitted; original observation retained'
            pending.append(entry)
        else:
            removed.append(entry)
    removed_ids = {entry['superseded']['id'] for entry in removed}
    return [line for line in lines if line.split('\t', 1)[0] not in removed_ids], removed, pending

QUICK_EXIT_ENTITY = '47001451'  # wsl_genericportal_pink, the Shelter's quick-exit portal
QUICK_EXIT_NAME = 'Quick Exit to the Arwic Outlands'


def without_training_placements(content, lines, shelter_quest=True):
    """The human start instance without the training kit's authored placements.

    `training-world.tsv` is always merged above, so its placements leave by IID. Block 0x0299 then
    holds nothing: retail's captured population of this dungeon, the Shelter (training stones,
    straw drudges, the guard, lieutenant and scout, the pink portal), is recorded in the
    byte-identical copy 0x0298. It is copied into 0x0299 at the same cell-local places, each under
    a new IID in the 0x8140 namespace (0x81400299 and the original's low 32 bits, the next free
    one if that is taken), as the kit's own mirror of the guard did. With `shelter_quest`, Rescue
    the Shelter Hold's placements that no capture holds (`human-shelter-world.tsv`) are added.
    The kit itself (`--training-kit`) is a server option and stays independent of this.
    """
    kit = {line.split('\t', 1)[0]
           for line in (content / 'training-world.tsv').read_text(encoding='utf-8').splitlines()
           if not line.startswith('#')}
    before = len(lines)
    lines = [line for line in lines if line.split('\t', 1)[0] not in kit]
    removed = before - len(lines)
    taken = {line.split('\t', 1)[0].lower() for line in lines}
    mirrored = []
    for line in lines:
        f = line.split('\t')
        if len(f) > 4 and not line.startswith('#') and f[3].lower().startswith('0298'):
            value = 0x8140029900000000 | (int(f[0], 16) & 0xFFFFFFFF)
            while f'{value:016x}' in taken:
                value += 1
            taken.add(f'{value:016x}')
            f[0] = f'{value:016x}'
            # The captured pink portal (cell 0x0120 beside the Warning Skull and the quick-exit
            # training stone) has no captured name, so clients show "Name_StringInfoStat
            # missing". It is the Shelter's quick exit: the other races' Shelters were captured
            # with the same trio and the portal named "Quick Exit to Lost Wish" (0x03F4) and
            # "Quick Exit to the Esper Reaches" (0x04F3); the human one is the DAT's "Quick Exit
            # to the Arwic Outlands" (portal string table 0x2500158C). The Menhir Portal stands at
            # the dungeon's other portal arch (`shelter.rs`, `human-shelter-world.tsv`).
            # Column 13 is the v4 world's captured name.
            if f[1].lower() == QUICK_EXIT_ENTITY and len(f) > 12 and f[12] in ('', '-'):
                f[12] = QUICK_EXIT_NAME
            f[3] = '0299' + f[3][4:]
            mirrored.append('\t'.join(f))
    lines = lines + mirrored
    added = []
    shelter_world = content / 'human-shelter-world.tsv'
    if shelter_quest and shelter_world.exists():
        # The file's first non-comment line is its world header.
        added = [line for line in shelter_world.read_text(encoding='utf-8').splitlines()
                 if not line.startswith('#')][1:]
        lines = lines + added
    return lines, {'training_placements_removed': removed,
                   'shelter_mirrored': len(mirrored), 'shelter_quest_placements': len(added)}


def sha256_any_eol(path):
    """A checked-in file's digest with LF or CRLF endings: `* text=auto` checks
    text out with CRLF on Windows and LF elsewhere, and evidence may come from either."""
    lf = path.read_bytes().replace(b'\r\n', b'\n')
    return {hashlib.sha256(lf).hexdigest(), hashlib.sha256(lf.replace(b'\n', b'\r\n')).hexdigest()}


def prepare(directory, output, compare_directory=None, training_placements=True, shelter_quest=True):
    index = asset_index(directory)
    comparison = asset_index(compare_directory) if compare_directory else None
    content = ROOT / 'emulator/content'
    newer = content / 'capture-spawns-upstream.tsv'
    audited = content / 'capture-spawns-audited.tsv'
    recovered = content / 'recovered-population.tsv'
    version = 4 if newer.exists() or audited.exists() else 3
    lines = [f'pyrealium-world-v{version}']
    skipped = []
    inactive = []
    seen = set()
    lifestones = []
    duplicates = []
    incompatible = []
    curated_pairs = set()
    profile = ContentProfile(directory)
    candidate_regressions = []
    sources = ['world-observed-expanded.tsv', 'training-world.tsv']
    recovered_cells = set()
    if recovered.exists():
        evidence = json.loads(recovered.with_suffix('.report.json').read_text(encoding='utf-8'))
        if not evidence.get('native_ground_verified') or evidence.get('manifest_sha256') not in sha256_any_eol(recovered):
            raise ValueError('Recovered population requires current native ground verification')
        sources.append(recovered.name)
        version = 4
        lines[0] = 'pyrealium-world-v4'
    sources.append('capture-spawns.tsv')
    if newer.exists(): sources.append(newer.name)
    if audited.exists(): sources.append(audited.name)
    inferred = content / 'inferred-world-v1.tsv'
    regional = content / 'regional-world-v1.tsv'
    wilderness = content / 'wilderness-world-v1.tsv'
    exploration = content / 'exploration-world-v1.tsv'
    hunting = content / 'hunting-world-v1.tsv'
    hunting_expansion = content / 'hunting-world-v2.tsv'
    workshops = content / 'workshops-world-v1.tsv'
    mines = content / 'arwic-mines-world-v1.tsv'
    npcs = content / 'npc-world-v1.tsv'
    for inferred_pack, catalog in [(inferred, 'inferred-content-v1.json'),
                                   (regional, 'regional-content-v1.json'),
                                   (wilderness, 'wilderness-content-v1.json'),
                                   (exploration, 'exploration-content-v1.json'),
                                   (hunting, 'hunting-content-v1.json'),
                                   (hunting_expansion, 'hunting-content-v2.json'),
                                   (workshops, 'workshops-content-v1.json'),
                                   (mines, 'arwic-mines-world-v1.json'),
                                   (npcs, 'npc-dialogue-v1.json')]:
        if not inferred_pack.exists():
            continue
        evidence = json.loads(inferred_pack.with_suffix('.report.json').read_text(encoding='utf8'))
        if (not evidence.get('native_ground_verified')
            or evidence['manifest_sha256'] not in sha256_any_eol(inferred_pack)
            or evidence['source_sha256'] not in sha256_any_eol(content/catalog)):
            raise ValueError('Inferred content requires fresh native ground validation')
        sources.append(inferred_pack.name)
        version = 4
        lines[0] = 'pyrealium-world-v4'
    state_file=content / 'capture-spawns-audited.report.json'
    state_exclusions={}
    if audited.exists() and state_file.exists():
        evidence=json.loads(state_file.read_text(encoding='utf-8'))
        state_exclusions={e['id']:e for e in evidence['excluded_existing']}
    transient=[]
    input_seen = set()
    input_counts = {}
    input_rows = {}
    admitted_sources = {}
    earlier_capture_cells = set()
    overlapping = []
    for name in sources:
        source_cells=set()
        for line in (content / name).read_text().splitlines()[1:]:
            if not line or line.startswith('#'):
                continue
            fields = line.split('\t')
            if len(fields) not in (12, 13): raise ValueError(f'Unsupported placement in {name}')
            iid, entity, visual, cell = [int(v, 16) for v in fields[:4]]
            input_rows[name, fields[0]] = line
            # A later capture cannot replace an earlier placement identity.
            # Incompatible older rows are also retained in the audit exactly once.
            if iid in input_seen: continue
            input_seen.add(iid)
            input_counts[name] = input_counts.get(name, 0) + 1
            if name.startswith('capture-spawns') and cell in recovered_cells:
                overlapping.append({'source':name,'id':fields[0],'cell':fields[3],
                                    'reason':'verified translated snapshot owns this cell'})
                continue
            if name in ('capture-spawns-upstream.tsv','capture-spawns-audited.tsv') and cell in earlier_capture_cells:
                overlapping.append({'source':name,'id':fields[0],'cell':fields[3],
                                    'reason':'earlier capture already owns this cell'})
                continue
            if name == 'world-observed-expanded.tsv' or name.startswith('capture-spawns'):
                source_cells.add(cell)
            exclusion=state_exclusions.get(fields[0])
            if exclusion and exclusion['source']==name and exclusion['row_sha256']==hashlib.sha256(line.encode()).hexdigest():
                transient.append(exclusion)
                continue
            # Outdoor cells use the landblock record; interior cells have individual records.
            missing = missing_records(fields, index)
            if missing:
                entry = {'source': name, 'id': fields[0], 'entity': fields[1], 'visual': fields[2],
                         'cell': fields[3], 'missing_records': missing}
                if comparison is not None:
                    entry['candidate_missing_records'] = missing_records(fields, comparison)
                    entry['candidate_has_required_records'] = not entry['candidate_missing_records']
                skipped.append(entry)
                continue
            if iid not in seen:
                if name.startswith('capture-spawns') or name in (recovered.name, inferred.name, regional.name, wilderness.name, exploration.name, hunting.name, hunting_expansion.name, workshops.name, npcs.name):
                    expected = profile.visual(entity)
                    if (entity, visual) not in curated_pairs and expected != visual:
                        incompatible.append({'source': name, 'id': fields[0], 'entity': fields[1],
                            'visual': fields[2], 'cell': fields[3],
                            'dat_visual': f'{expected:08x}' if expected else None,
                            'reason': 'visual differs from installed entity template' if expected else
                                      'capture visual cannot be verified against installed templates'})
                        continue
                else:
                    # The curated observation and authored training packs supply
                    # known profiles for script-only templates (e.g. lifestones).
                    curated_pairs.add((entity, visual))
                if entity == 0x470007de:
                    position = tuple(float(v) for v in fields[4:7])
                    duplicate = next((old for old, c, p in lifestones
                                      if nearby_landmark(cell, position, c, p)), None)
                    if duplicate is not None:
                        duplicates.append({'source': name, 'id': fields[0], 'retained': f'{duplicate:016x}'})
                        continue
                    lifestones.append((iid, cell, position))
                if version == 4 and len(fields) == 12: fields.append('-')
                lines.append('\t'.join(fields))
                seen.add(iid)
                admitted_sources[fields[0]] = name
                if name == recovered.name: recovered_cells.add(cell)
                if comparison is not None:
                    absent = missing_records(fields, comparison)
                    if absent:
                        candidate_regressions.append({'id': fields[0], 'source': name, 'missing_records': absent})
        earlier_capture_cells.update(source_cells)
    profile.close()
    lines, superseded, pending_replacements = apply_placement_replacements(
        content, lines, input_rows, admitted_sources)
    retired_ids = {entry['superseded']['id'] for entry in superseded}
    seen.difference_update(int(iid, 16) for iid in retired_ids)
    candidate_regressions = [entry for entry in candidate_regressions if entry['id'] not in retired_ids]
    start_instance = None
    if not training_placements:
        lines, start_instance = without_training_placements(content, lines, shelter_quest)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text('\n'.join(lines) + '\n', encoding='utf-8')
    # The scripted quests' tables (quest registry, stages, emote sets and actions) are read
    # beside the world table at server start, each base table with its families
    # (`quest-registry-vault.tsv`, ...; quest_emotes::families).
    for name in ('quest-registry.tsv', 'quest-stages.tsv', 'emote-sets.tsv', 'emote-actions.tsv'):
        stem = name[:-len('.tsv')]
        for stale in output.parent.glob(f'{stem}-*.tsv'):
            stale.unlink()
        for source in [content / name, *sorted(content.glob(f'{stem}-*.tsv'))]:
            shutil.copyfile(source, output.parent / source.name)
    report = {'included': len(seen), 'excluded': len(skipped)+len(incompatible)+len(duplicates)+len(overlapping)+len(transient)+len(superseded), 'missing_assets': skipped,
              'superseded_placements': superseded, 'unapplied_placement_replacements': pending_replacements,
              'transient_capture_placements':transient,
              'source_unique_placements': input_counts,
              'overlapping_capture_cells':overlapping,
              'inactive_training_services': inactive, 'duplicate_landmarks': duplicates,
              'incompatible_capture_placements': incompatible,
              'dat_directory': str(directory.resolve()),
              'missing_record_counts': {kind: len({r['record'] for e in skipped for r in e['missing_records'] if r['kind'] == kind})
                                       for kind in ('entity', 'visual', 'cell')}}
    if start_instance is not None:
        report['start_instance'] = start_instance
    if comparison is not None:
        report['comparison'] = {'dat_directory': str(compare_directory.resolve()),
                                'recoverable_placements': sum(e['candidate_has_required_records'] for e in skipped),
                                'currently_supported_placements_missing_in_candidate': candidate_regressions,
                                'note': 'Presence check only; dependent assets and protocol compatibility must also be verified.'}
    output.with_suffix('.report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(f'Compatible server placements: {len(seen)}; placements missing DAT records: {len(skipped)}; duplicate landmarks: {len(duplicates)}; inactive training service props: {len(inactive)}')
    if start_instance is not None:
        print(f"Training placements removed: {start_instance['training_placements_removed']}; Shelter mirrored into 0x0299: {start_instance['shelter_mirrored']}; Shelter Hold quest placements: {start_instance['shelter_quest_placements']}")
    print(f'Unverified or incompatible capture placements withheld: {len(incompatible)}. Source captures are unchanged; see {output.with_suffix(".report.json")}')
    if overlapping: print(f'Overlapping capture observations withheld: {len(overlapping)}; one verified snapshot retained per cell.')
    if transient: print(f'Captured looted/dead objects withheld: {len(transient)}; exact original observation rows retained in source files.')
    if superseded: print(f'Superseded duplicate placements withheld: {len(superseded)}; verified replacements retained.')
    if pending_replacements: print(f'Placement replacements unavailable: {len(pending_replacements)}; original observations retained.')
    if skipped:
        print(f'Missing unique records: {report["missing_record_counts"]}. Details: {output.with_suffix(".report.json")}')
    if comparison is not None:
        print(f'Candidate contains required records for {report["comparison"]["recoverable_placements"]}/{len(skipped)} excluded placements; installed DATs unchanged.')
        print(f'Currently supported placements missing in candidate: {len(candidate_regressions)}')

if __name__ == '__main__':
    p = argparse.ArgumentParser()
    p.add_argument('dat_directory', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--compare-dat', type=Path, help='Read-only comparison with another complete installation; does not merge archives.')
    p.add_argument('--strip-training-placements', action='store_true',
                   help="Leave out training-world.tsv's placements and mirror the captured 0x0298 Shelter into the human start instance 0x0299.")
    p.add_argument('--no-shelter-quest', action='store_true',
                   help="With --strip-training-placements, leave out human-shelter-world.tsv (Rescue the Shelter Hold's placements).")
    a = p.parse_args()
    prepare(a.dat_directory, a.output, a.compare_dat,
            training_placements=not a.strip_training_placements, shelter_quest=not a.no_shelter_quest)
