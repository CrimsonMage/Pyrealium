"""Tabulate the appearance keys retail servers gave each item, from the AC2 captures.

A CreateObject (0x10058) carries the object's VisualDesc first: a pack bitfield, the parent
VisualDesc DID (bit 2) and, with bit 0x100, the global part group mods (PartGroupDataDesc:
bitfield 0x21, key 0x40000002, then the appearance hash: table DID -> {appearance key: scalar}).
Retail sent those mods on many items, mostly one key per item (0x41000021) whose scalar differs
between instances of one item type and stays with an instance across sightings; the client takes
an inventory icon's atoms from them (VisualDesc::GetTotalIconDesc). This tool decodes the
VisualDesc, the PhysicsDesc and the gmWeenieDesc entity DID of every item CreateObject whose
VisualDesc is parent-only (0x2) or parent plus global mods (0x102), and writes, for each item
type (entity, visual) observed at least once with mods, every distinct set seen (an empty set for
parent-only), counted by distinct object instance and by server era (object id digit 2: the
pre-2012 servers; 3: the 2012-2017 relaunch).

How a retail server chose a new item's set is not recorded anywhere; the table is evidence of
what it sent, nothing more. Raw captures, names and positions are not exported.

    python3 -I tools/audit_item_appearance.py CAPTURE_ROOT content/7230/item-appearance-observed.tsv
"""
import argparse
import collections
import pathlib
import struct
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from capture_packets import reassemble  # noqa: E402

PCAP = b'\xd4\xc3\xb2\xa1'
CREATE_OBJECT = 0x10058
PHYSICS_KNOWN = (0x80 | 2 | 0x20000 | 0x8000 | 0x100 | 4 | 0x400000 | 0x800000 | 0x1000000 | 8 | 0x10
                 | 0x2000000 | 0x4000000 | 0x8000000 | 0x400 | 0x800 | 0x1000 | 0x200 | 0x4000)
PHYSICS_FIXED = [(0x100, 4), (4, 12), (0x400000, 12), (0x800000, 4), (0x1000000, 4), (8, 12), (0x10, 12),
                 (0x2000000, 8), (0x4000000, 4), (0x8000000, 4), (0x400, 8), (0x800, 32), (0x1000, 12)]


class Reader:
    def __init__(self, data):
        self.data, self.pos = data, 0

    def take(self, n):
        if self.pos + n > len(self.data):
            raise ValueError('short message')
        value = self.data[self.pos:self.pos + n]
        self.pos += n
        return value

    def word(self):
        return struct.unpack('<I', self.take(4))[0]

    def real(self):
        return struct.unpack('<f', self.take(4))[0]


def global_mods(r):
    """PartGroupDataDesc of the global mods: only the key and appearance hash are expected."""
    bits = r.word()
    if bits != 0x21 or r.word() != 0x40000002:
        raise ValueError('unexpected global part group')
    tables = []
    for _ in range(r.word() & 0xffffff):
        table = r.word()
        keys = tuple(sorted((r.word(), round(r.real(), 4)) for _ in range(r.word() & 0xffffff)))
        tables.append((table, keys))
    return tuple(sorted(tables))


def item(data):
    """(object id, visual, mods, entity) of an item CreateObject, or None."""
    r = Reader(data)
    if r.word() != CREATE_OBJECT:
        return None
    iid = struct.unpack('<Q', r.take(8))[0]
    mask = r.word()
    if mask not in (0x2, 0x102):
        return None
    visual = r.word()
    mods = global_mods(r) if mask & 0x100 else ()
    flags = r.word()
    if flags & ~PHYSICS_KNOWN:
        return None
    r.take(4)
    if flags & 0x80:
        r.word()
    if flags & 2:
        r.take(12 * r.word())
    if flags & 0x20000:
        r.take(4)
    if flags & 0x8000:
        r.take(32)
    for bit, size in PHYSICS_FIXED:
        if flags & bit:
            r.take(size)
    if flags & 0x200:
        r.take(16 * r.word())
    r.take(8)
    r.take((-r.pos) % 4)
    weenie = r.word()
    if weenie & 1:
        r.take(4)
    if not weenie & 0x2000000:
        return None
    entity = r.word()
    if entity >> 24 != 0x47 or visual >> 24 != 0x1f:
        return None
    return iid, visual, mods, entity


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('root', type=pathlib.Path)
    parser.add_argument('output', type=pathlib.Path)
    args = parser.parse_args()
    seen = collections.defaultdict(lambda: collections.defaultdict(set))
    for path in sorted(args.root.rglob('*')):
        if not path.is_file():
            continue
        raw = path.read_bytes()
        if raw[:4] != PCAP:
            continue
        try:
            messages = list(reassemble(raw, collections.Counter()))
        except ValueError:
            continue
        for message in messages:
            data = message.data
            if len(data) < 20 or struct.unpack_from('<I', data, 0)[0] != CREATE_OBJECT:
                continue
            try:
                found = item(data)
            except ValueError:
                continue
            if found:
                iid, visual, mods, entity = found
                seen[(entity, visual)][mods].add(iid)
    rows = []
    for (entity, visual), sets in sorted(seen.items()):
        if all(not mods for mods in sets):
            continue
        for mods, iids in sorted(sets.items()):
            eras = collections.Counter((iid >> 52) & 0xf for iid in iids)
            text = ';'.join('%08X:' % table + ','.join('%08X=%s' % (key, value) for key, value in keys)
                            for table, keys in mods)
            rows.append('%08X\t%08X\t%d\t%d\t%s' % (entity, visual, eras[2], sum(eras.values()) - eras[2], text))
    header = ('# Item appearance sets retail servers sent (tools/audit_item_appearance.py over the AC2 captures).\n'
              '# entity\tvisual\tinstances_pre2012\tinstances_2012_2017\tglobal mods (empty: parent-only)\n')
    args.output.write_text(header + '\n'.join(rows) + '\n')
    print(len(rows), 'sets for', len({tuple(r.split('\t')[:2]) for r in rows}), 'item types')


if __name__ == '__main__':
    main()
