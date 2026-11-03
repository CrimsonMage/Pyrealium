"""Read-only AC2 record inspection for local format research (never modifies DATs)."""
import struct
from pathlib import Path

class Archive:
    def __init__(self, path):
        self.file = open(path, 'rb')
        self.file.seek(300)
        h = struct.unpack('<20I', self.file.read(80))
        assert h[0] == 0x5442
        self.sector, self.root, self.size = h[1], h[8], h[2]
    def chain(self, offset, size):
        assert 0 <= size <= 64*1024*1024
        out = bytearray(); visited = set()
        while len(out) < size:
            assert offset not in visited and offset >= 1024 and offset % self.sector == 0 and offset < self.size
            visited.add(offset)
            self.file.seek(offset)
            offset = struct.unpack('<I', self.file.read(4))[0]
            out += self.file.read(min(size-len(out), self.sector-4))
        assert offset == 0
        return bytes(out)
    def read(self, did):
        offset = self.root
        for _ in range(64):
            if not offset: break
            b = self.chain(offset, 1228); n = struct.unpack_from('<I', b, 248)[0]
            assert n <= 61
            for i in range(n):
                key, addr, size, _ = struct.unpack_from('<4I', b, 252+i*16)
                if key == did: return self.chain(addr, size)
                if key > did: break
            else: i = n
            offset = struct.unpack_from('<I', b, 4*i)[0] if struct.unpack_from('<I',b)[0] else 0
        raise KeyError(f'{did:08X}')
    def entries(self, offset=None):
        b = self.chain(self.root if offset is None else offset, 1228)
        n = struct.unpack_from('<I', b, 248)[0]
        for i in range(n+1):
            child = struct.unpack_from('<I', b, i*4)[0] if struct.unpack_from('<I',b)[0] else 0
            if child: yield from self.entries(child)
            if i<n: yield struct.unpack_from('<4I', b, 252+i*16)

if __name__ == '__main__':
    import argparse
    p=argparse.ArgumentParser();p.add_argument('archive',type=Path);p.add_argument('id',type=lambda x:int(x,16));a=p.parse_args()
    b=Archive(a.archive).read(a.id)
    print(f'{a.id:08X}: {len(b)} bytes')
    print(b[:256].hex(' '))
