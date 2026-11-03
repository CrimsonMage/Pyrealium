"""Read native crafting WState graphs without executing or modifying the DAT.

Object framing follows HeapObject::UnPack; only explicit bounded native shapes
are accepted. Unsupported graphs are reported, never scanned for guessed DIDs.
"""
import json
import struct
from collections import Counter
from pathlib import Path
from ac2_dat import Dat
from import_skill_catalog import header


def graph(data, did):
    count, pos = header(data, did)
    nodes = []
    def word():
        nonlocal pos
        if pos + 4 > len(data): raise ValueError('truncated graph')
        v = struct.unpack_from('<I', data, pos)[0]; pos += 4
        return v
    def bounded(n):
        if n > 65536: raise ValueError('graph collection limit')
        return n
    for _ in range(count):
        flags = word(); node = {'flags': flags}; nodes.append(node)
        if flags & 0x300 != 0x100:
            node['value'] = word(); continue
        if flags & 0x20000000:
            node['did'] = word(); continue
        if flags & 0x8000000 and not flags & 0x10000000: continue
        node['state'] = word(); kind = word(); node['package'] = kind >> 16
        native = kind & 65535; node['native'] = native
        if native in (1, 3, 0x11):
            node['pairs'] = [[word(), word()] for _ in range(bounded(word() & 0xffffff))]
        elif native in (5, 0xa, 0x61, 0x65):
            node['list'] = [word() for _ in range(bounded(word()))]
        elif native == 0x37:
            node['list'] = [word() | (word() << 32) for _ in range(bounded(word()))]
        elif native == 0x76:
            from import_skill_effects import decrypt
            n = struct.unpack_from('<H', data, pos)[0]; pos += 2
            if pos+n*2 > len(data): raise ValueError('truncated string')
            node['text'] = decrypt(data[pos:pos+n*2], n*2).decode('utf-16-le')
            pos = (pos+n*2+3)&~3
        elif native == 0x6a:
            node['string'] = [word(), word()]
            if word() != 0: raise ValueError('substituted StringInfo')
        elif native:
            raise ValueError(f'unsupported native {native:04x}')
        n = bounded(word()); node['slots'] = [word() for _ in range(n)]
        if pos + n > len(data): raise ValueError('truncated tags')
        node['tags'] = list(data[pos:pos+n]); pos = (pos+n+3)&~3
    if pos != len(data)-4 or word() != nodes[0].get('package'):
        raise ValueError('graph trailer')
    return nodes


if __name__ == '__main__':
    import argparse
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('portal', type=Path); p.add_argument('output', type=Path)
    a = p.parse_args(); records = {}; errors = Counter()
    with Dat(a.portal) as dat:
        for did in dat.index():
            if did >> 24 != 0x6f: continue
            try: records[f'{did:08x}'] = graph(dat.read(did), did)
            except ValueError as e: errors[str(e)] += 1
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(records), encoding='utf-8')
    print(json.dumps({'decoded': len(records), 'unsupported': dict(errors)}))
