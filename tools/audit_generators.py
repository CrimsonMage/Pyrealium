"""Audit native generator evidence without inventing child lists or locations.

First run Tools/AC2/population_probe.py for the same DAT directory. This reduces
its complete placement/template audit to generator properties and references.
Labels are names only, not generator definitions. No executable is run.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools/ac2'))
from dat_probe import Archive
from population_probe import Reader


def summarize(population, labels):
    templates = population['templates']
    names = population['property_names']

    def inherited(row):
        props, seen = {}, set()
        while row:
            for key, value in row.get('properties', {}).items():
                name = names.get(str(int(key[9:], 16)), key)
                props.setdefault(name, value)
            parent = row['data']
            if parent in seen:
                raise ValueError('cyclic entity inheritance')
            seen.add(parent)
            row = templates.get(f'{parent:08X}')
        return props

    records = []
    for did, row in templates.items():
        props = {k: v for k, v in inherited(row).items() if k.startswith('Generator')}
        if not props:
            continue
        for key in ('GeneratorRegenPeriod', 'GeneratorInitialDelay'):
            if key in props:
                props[key] = struct.unpack('<f', struct.pack('<I', props[key]))[0]
        records.append(dict(entity=did, properties=props,
                            profile_name=labels.get(props.get('GeneratorProfile'))))
    placements = [r for r in population['placements'] + population['interior_placements']
                  if 'GeneratorProfile' in inherited(r)]
    return dict(template_records_with_generator_properties=len(records),
                profile_label_count=len(labels), placed_generator_profiles=len(placements),
                outdoor_placements_examined=population['placement_count'],
                interior_placements_examined=population['interior_placement_count'],
                encounter_descriptor=population['encounter_descriptor'],
                parse_rejections=population['rejected'], templates=records, placements=placements,
                limitation='Profile labels and template properties do not provide child lists, spawn regions, '
                           'probabilities or population sizes. Do not bind them to captured monsters by name.')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directory', type=Path)
    p.add_argument('--population-audit', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    population = json.loads(args.population_audit.read_text(encoding='utf-8'))
    portal = Archive(args.directory/'portal.dat')
    labels = {}
    try:
        try:
            r = Reader(portal.read(0x23000106))
        except KeyError:
            r = None
        if r:
            assert r.word() == 0x23000106
            r.word()
            for _ in range(r.word()):
                key = r.word()
                labels[key] = r.string()
            assert r.pos == len(r.data)
    finally:
        portal.file.close()
    report = summarize(population, labels)
    report['inputs'] = {}
    for name, path in [('portal.dat', args.directory/'portal.dat'), ('cell_1.dat', args.directory/'cell_1.dat'),
                       ('population_audit', args.population_audit)]:
        with path.open('rb') as stream:
            report['inputs'][name] = hashlib.file_digest(stream, 'sha256').hexdigest()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False)+'\n', encoding='utf-8')
    print(json.dumps({k: v for k, v in report.items() if k not in ('templates', 'placements')}, indent=2))


if __name__ == '__main__':
    main()
