"""Read-only GUN transport/schema evidence; never exports attribute values.

This does not replay traffic, decrypt tickets, or implement a reliable UDP peer.
Only complete XML messages in individual datagrams are inspected. Fragmented
messages are counted but not guessed/reassembled. Source captures remain local.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET

from capture_packets import datagrams

MAX_CAPTURE = 128 * 1024 * 1024
PORT = 2310
# Allowlisted schema names prevent accidental disclosure of capture content.
SCHEMA = {
    'Connect': {'Cx', 'ProtoVer'},
    'ConnectRes': {'Cx', 'HR', 'Random'},
    'RSAKey': {'Val'},
    'ConInfo': {'Cx', 'Locale', 'AppGuid'},
    'CdKey': {'Val'},
    'Random': {'Val'},
    'ConInfoRes': {'Cx', 'HR'},
    'PresentTicketCmd': {'Cx', 'Email'},
    'Ticket': {'Val'},
    'PresentTicketRes': {'Cx', 'HR', 'Status'},
    'ReleaseTicketCmd': {'Cx'},
    'ReleaseTicketRes': {'Cx', 'HR'},
    'FriendAdd': {'Cx', 'User'},
    'FriendAddRes': {'Cx', 'HR'},
    'FriendDelete': {'Cx', 'User'},
    'FriendDeleteRes': {'Cx', 'HR'},
    'GetFriendsList': {'Cx'},
    'GetFriendsListRes': {'Cx', 'HR', 'Cnt'},
    'FStart': {'Cnt'},
    'FriendStatus': {'User', 'Mode', 'Location'},
    'FriendStat': {'User', 'Mode', 'Location'},
    'GetIgnoredList': {'Cx'},
    'GetIgnoredListRes': {'Cx', 'HR', 'Cnt'},
    'IgnoredAdd': {'Cx', 'User'},
    'IgnoredAddRes': {'Cx', 'HR'},
    'IgnoredDelete': {'Cx', 'User'},
    'IgnoredDeleteRes': {'Cx', 'HR'},
    'Flags': {'Cx', 'Op', 'Word', 'Flags'},
    'FlagsRes': {'Cx', 'HR', 'WhichFlag', 'Flags'},
    'SetLoc': {'Cx', 'Flags', 'Location'},
    'SetLocRes': {'Cx', 'HR'},
}


def inspect(raw):
    if len(raw) > MAX_CAPTURE:
        raise ValueError('capture exceeds 128 MiB')
    counts, headers, messages, attrs = Counter(), Counter(), Counter(), defaultdict(Counter)
    for _, flow, payload in datagrams(raw):
        source, destination = struct.unpack('>HH', flow[1])
        if PORT not in (source, destination):
            continue
        direction = 'server_to_client' if source == PORT else 'client_to_server'
        counts[direction] += 1
        if len(payload) < 4:
            counts['short_datagrams'] += 1
            continue
        flags = struct.unpack_from('<H', payload)[0]
        headers[f'{flags:04x}'] += 1
        # Data flags 0x37/0x3f differ in their acknowledgement request bit.
        # Upper flag bytes vary on retransmission. Do not treat sequence bytes
        # as payload or mistake handshake/ACK packets for XML.
        if flags & 0xff in (0x1f, 0x2f):
            counts['fragmented_datagrams_not_decoded'] += 1
            continue
        if flags & 0xff not in (0x37, 0x3f) or len(payload) < 7:
            continue
        if payload[4:6] != b'\x50\xc3':
            continue
        xml = payload[6:].rstrip(b'\0')
        if b'<!' in xml or b'<?' in xml:
            counts['unsupported_xml'] += 1
            continue
        try:
            root = ET.fromstring(xml)
        except (ET.ParseError, ValueError):
            counts['invalid_or_incomplete_xml'] += 1
            continue
        counts['complete_xml_messages'] += 1
        for element in root.iter():
            if element.tag not in SCHEMA:
                counts['unknown_elements'] += 1
                continue
            messages[element.tag] += 1
            for key in element.attrib:
                if key in SCHEMA[element.tag]:
                    attrs[element.tag][key] += 1
                else:
                    counts['unknown_attributes'] += 1
    return {'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw),
            'udp_port': PORT, 'counts': dict(counts), 'header_flags': dict(headers),
            'elements': dict(messages), 'attributes': {k: dict(v) for k, v in attrs.items()}}


def audit(paths):
    results, seen = [], set()
    for path in paths:
        if path.stat().st_size > MAX_CAPTURE:
            raise ValueError('capture exceeds 128 MiB')
        result = inspect(path.read_bytes())
        if result['sha256'] not in seen:
            seen.add(result['sha256'])
            results.append(result)
    return {'version': 1, 'transport': 'UDP', 'captures': results,
            'limits': 'Complete individual datagrams only; no ticket values or live compatibility claim.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('captures', type=Path, nargs='+', help='PCAP/PCAPNG files (including .elog captures)')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    encoded = json.dumps(audit(args.captures), indent=2) + '\n'
    if args.output:
        args.output.write_text(encoded, encoding='utf-8')
    else:
        print(encoded, end='')


if __name__ == '__main__':
    main()
