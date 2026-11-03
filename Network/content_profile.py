"""Validate capture visuals against this installation, not just DID presence.

The retained captures span different client builds. A DID existing in portal.dat
does not mean it still describes the captured model. Unverified observations
stay in the source manifest and are reported for later recovery.
"""
from population_probe import Reader, entity
from dat_probe import Archive
import struct


def door_physical(data):
    """CBaseQualities prefix including nested StringInfo substitutions.

    Door visuals often live in class defaults instead of EntityDesc::PhysObj.
    Read DID quality 1; never guess the last word or use a captured visual.
    """
    r = Reader(data)
    mask, did, desc = r.word(), r.word(), r.word()
    assert mask & 1 and did >> 24 == 0x81 and not desc & 0x80000000
    def string_info(depth=0):
        assert depth < 8
        r.take(8); flags = r.word(); count = flags & 65535
        assert count <= 256
        if flags >> 16: r.string(2)
        for _ in range(count):
            r.word(); kind = struct.unpack('<H', r.take(2))[0]; r.align()
            if kind == 9: string_info(depth + 1)
            elif kind in (2, 6): r.take(4)
            elif kind in (3, 4, 7): r.take(10)
            elif kind == 5: r.take(16)
            elif kind in (10, 12): r.take(8)
            elif kind in (11, 13): r.take(14)
            else: assert kind in (0, 1, 8)
            r.align()
    for bit in (1, 0x2000000, 2):
        if desc & bit: r.word()
    for bit in (4, 8):
        if desc & bit: string_info()
    for bit in (0x10, 0x20, 0x40, 0x100, 0x200, 0x20000000, 0x40000000, 0x4000000,
                0x80, 0x800, 0x1000, 0x2000, 0x200000, 0x400000, 0x8000,
                0x10000, 0x20000, 0x40000, 0x80000, 0x100000, 0x8000000, 0x10000000):
        if desc & bit: r.take(8 if bit in (0x20, 0x40, 0x100, 0x200, 0x20000000, 0x40000000, 0x4000000) else 4)
    for bit, width in ((2, 8), (4, 8), (8, 8), (0x10, 12)):
        if mask & bit:
            count = r.word() & 0xffffff; assert count <= 512; r.take(count * width)
    if mask & 0x20:
        count = r.word() & 0xffffff; assert count <= 512
        for _ in range(count):
            key, value = r.word(), r.word()
            if key == 1: return value
    return None


class ContentProfile:
    def __init__(self, directory):
        self.cache = {}
        self.portal = Archive(directory / 'portal.dat')
        r = Reader(self.portal.read(0x34000000))
        assert r.word() == 0x34000000
        r.take(8)
        for _ in range(r.word()):
            r.word()
            r.string()
        self.types = {}
        for _ in range(r.word() & 65535):
            key = r.word()
            self.types[key] = r.word()
            r.take(16)

    def close(self):
        self.portal.file.close()
        self.cache.clear()

    def visual(self, did):
        if did not in self.cache:
            self.cache[did] = self.resolve_visual(did)
        return self.cache[did]

    def resolve_visual(self, did):
        original = did
        seen = set()
        for _ in range(32):
            if did >> 24 == 0x1f:
                return did
            if did >> 24 != 0x47 and did != 0:
                try:
                    physical = door_physical(self.portal.read(0x81000000 | (original & 0xffffff)))
                    return self.visual(physical) if physical and physical != original else None
                except (KeyError, ValueError, AssertionError):
                    return None
            if did >> 24 != 0x47 or did in seen:
                return None
            seen.add(did)
            try:
                r = Reader(self.portal.read(did))
                desc = entity(r, self.types)
                if r.pos != len(r.data):
                    return None
            except (KeyError, ValueError, AssertionError):
                return None
            # EntityDesc::GetTotalProperties: an explicit PhysObj overrides the
            # parent. Otherwise follow the template chain, never a script class.
            did = desc.get('properties', {}).get('40000017:4100020B') or desc['data']
        return None


def landmark_position(cell, position):
    """Outdoor subcells share one global space; sealed rooms do not."""
    if 1 <= (cell & 65535) <= 64:
        return 0, (position[0] + (cell >> 24) * 160,
                   position[1] + ((cell >> 16) & 255) * 160, position[2])
    return cell, position


def nearby_landmark(cell, position, other_cell, other_position):
    space, point = landmark_position(cell, position)
    other_space, other_point = landmark_position(other_cell, other_position)
    return space == other_space and sum((a-b)**2 for a, b in zip(point, other_point)) <= 144
