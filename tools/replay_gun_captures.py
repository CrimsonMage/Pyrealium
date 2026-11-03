"""Validate local retail captures with the emulator's native GUN Rust decoder.

Build examples/gun_replay first. Packet bytes travel only through subprocess
stdin, never through command arguments, temporary files, or exported reports.
This is offline decoding; no network packets are transmitted.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess

from audit_gun_captures import MAX_CAPTURE
from capture_packets import datagrams


def replay(paths, decoder):
    seen, records = set(), []
    byte_count = 0
    for path in paths:
        if path.stat().st_size > MAX_CAPTURE:
            raise ValueError('capture exceeds 128 MiB')
        raw = path.read_bytes()
        digest = hashlib.sha256(raw).hexdigest()
        if digest in seen:
            continue
        seen.add(digest)
        flows = {}
        records.append('{"reset":true}\n')
        for _, flow, payload in datagrams(raw):
            if 2310 not in struct.unpack('>HH', flow[1]):
                continue
            stream = flows.setdefault(flow, len(flows))
            if len(flows) > 128:
                raise ValueError('capture stream limit')
            encoded = json.dumps({'stream': stream, 'hex': payload.hex()}) + '\n'
            byte_count += len(encoded)
            if byte_count > 32 * 1024 * 1024:
                raise ValueError('replay input limit; split corpus into batches')
            records.append(encoded)
    result = subprocess.run([str(decoder)], input=''.join(records), text=True,
                            capture_output=True, timeout=120, check=False)
    if result.returncode:
        # Never relay subprocess input/error excerpts that might contain a ticket.
        raise ValueError('native decoder failed; no payload data exported')
    report = json.loads(result.stdout)
    return {'version': 1, 'unique_captures': len(seen),
            'capture_sha256': sorted(seen), 'native_decoder': report,
            'limits': 'Unsigned framing and reliable sequential receive only; no live login claim.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('captures', type=Path, nargs='+', help='Capture files or directories to scan for .elog')
    parser.add_argument('--decoder', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    paths = []
    for path in args.captures:
        paths.extend(sorted(path.rglob('*.elog')) if path.is_dir() else [path])
    if not paths:
        parser.error('no captures found')
    report = replay(paths, args.decoder.resolve())
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'unique_captures': report['unique_captures'], **report['native_decoder']}))


if __name__ == '__main__':
    main()
