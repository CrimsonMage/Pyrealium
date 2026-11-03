"""Recover missing private quest strings as evidence, without replacing DAT text.

The installed parent record must match the archived parent byte for byte. Every
redirect is followed by the same key and fingerprinted. Raw grammar markers are
preserved; this catalog is not a runtime localization overlay.
"""
import argparse
import shutil
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/ac2'))
from dat_probe import Archive


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def quest_fields(quest):
    for field in ('name', 'description', 'summary'):
        yield field, quest[field]
    for phase in quest['phases']:
        yield f"phases/{phase['phase']}/journal", phase['journal']


def resolve_missing(field, installed, archived, decoded):
    """Return (candidate, reason). Callbacks also allow bounded synthetic tests."""
    if field.get('text') is not None or field.get('error') != 'MissingPrivateTable':
        return None, 'not_missing_private_table'
    if field.get('variables') not in (None, '[]', []):
        return None, 'requires_string_variables'
    did, key = field['table'], f"{field['key']:08x}"
    seen, records = set(), []
    for depth in range(16):
        if did in seen:
            return None, 'redirect_cycle'
        seen.add(did)
        old, current = archived(did), installed(did)
        if old is None:
            return None, 'archived_record_missing'
        if depth == 0 and current is None:
            return None, 'installed_parent_missing'
        if current is not None and current != old:
            return None, 'record_version_mismatch'
        table = decoded(did)
        if table is None or table.get('error'):
            return None, 'record_decode_failed'
        entry = table.get('entries', {}).get(key)
        if entry is None:
            return None, 'key_missing'
        records.append({'table': f'{did:08x}', 'sha256': sha256(old),
                        'installed_record_identical': current is not None})
        if entry.get('variables'):
            return None, 'requires_string_variables'
        private = entry.get('private_table', 0)
        if private:
            if not 0x25000000 <= private <= 0x25ffffff:
                return None, 'invalid_private_table'
            did = private
            continue
        text = entry.get('text')
        if not text:
            return None, 'empty_archived_text'
        if all(r['installed_record_identical'] for r in records):
            return None, 'no_missing_record_in_chain'
        return {'text_raw': text, 'records': records}, None
    return None, 'redirect_depth_exceeded'


class LooseTables:
    def __init__(self, root, probe, dat_dir):
        self.root, self.probe, self.dat_dir = root, probe, dat_dir
        self.raw, self.decoded = {}, {}

    def read(self, did):
        if did not in self.raw:
            path = self.root / f'0x{did:08X}'
            if path.is_file() and path.stat().st_size <= 16 * 1024 * 1024:
                self.raw[did] = path.read_bytes()
            else:
                self.raw[did] = None
        return self.raw[did]

    def preload(self, ids):
        pending = set(ids)
        for _ in range(16):
            pending -= self.decoded.keys()
            if not pending:
                return
            ordered = sorted(pending)
            for offset in range(0, len(ordered), 200):
                batch = ordered[offset:offset+200]
                output = subprocess.check_output([str(self.probe), str(self.dat_dir),
                    '--loose-strings', str(self.root), *[f'{did:08x}' for did in batch]], encoding='utf-8')
                for line in output.splitlines():
                    table = json.loads(line)
                    self.decoded[int(table['table'], 16)] = table
                if any(did not in self.decoded for did in batch):
                    raise ValueError('Probe omitted a requested table')
            pending = {e['private_table'] for did in ordered
                       for e in self.decoded[did].get('entries', {}).values()
                       if 0x25000000 <= e.get('private_table', 0) <= 0x25ffffff}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('dat_directory', type=Path)
    parser.add_argument('--loose-strings', type=Path, required=True)
    parser.add_argument('--source-archive', type=Path, required=True)
    parser.add_argument('--quests', type=Path, default=ROOT/'content/native-quests.json')
    parser.add_argument('--probe', type=Path, default=Path(shutil.which('ac2-retail-probe') or 'ac2-retail-probe'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    raw_quests = args.quests.read_bytes()
    quests = json.loads(raw_quests)['quests']
    loose = LooseTables(args.loose_strings, args.probe, args.dat_directory)
    loose.preload({f['table'] for q in quests for _, f in quest_fields(q)
                   if f.get('text') is None and f.get('error') == 'MissingPrivateTable'})
    dat = Archive(args.dat_directory/'local_English.dat')
    current = {}
    def installed(did):
        if did not in current:
            try: current[did] = dat.read(did)
            except KeyError: current[did] = None
        return current[did]
    candidates, unresolved, excluded = [], [], Counter()
    try:
        for quest in quests:
            for path, field in quest_fields(quest):
                result, reason = resolve_missing(field, installed, loose.read, loose.decoded.get)
                identity = {'quest': f"{quest['id']:08x}", 'package': quest['package'],
                            'field': path, 'table': f"{field['table']:08x}", 'key': f"{field['key']:08x}"}
                if result:
                    candidates.append({**identity, **result})
                elif reason == 'not_missing_private_table':
                    excluded[reason] += 1
                else:
                    unresolved.append({**identity, 'reason': reason})
    finally:
        dat.file.close()
    report = {'format': 1, 'runtime_enabled': False,
        'scope': 'Authoring evidence only. Identical installed parent records; missing private records from historical Pyreal English archive. Raw grammar markers preserved. Not proof of quest eligibility, rewards, or matching private-record build.',
        'source_archive': args.source_archive.as_posix(),
        'source_archive_sha256': sha256(args.source_archive.read_bytes()),
        'native_quest_catalog_sha256': sha256(raw_quests),
        'summary': {'recovered_fields': len(candidates), 'quests': len({c['quest'] for c in candidates}),
                    'unresolved_fields': len(unresolved), 'excluded_fields': dict(excluded)},
        'candidates': candidates, 'unresolved': unresolved}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False)+'\n', encoding='utf-8')
    print(json.dumps(report['summary']))


if __name__ == '__main__':
    main()
