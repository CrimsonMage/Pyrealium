"""Bounded, read-only PCAP/PCAPNG and AC2 fragment reader.

Capture checksums may have been changed by the original logging/decryption tool;
this is evidence extraction, never a live packet replay or authentication path.
"""
from collections import OrderedDict
from dataclasses import dataclass
import struct

MAX_FRAME = 1024 * 1024


def frames(raw):
    """Yield (seconds, link type, frame), including PCAPNG interface clocks."""
    magic = raw[:4]
    classic = {b'\xd4\xc3\xb2\xa1': ('<', 1e6), b'\xa1\xb2\xc3\xd4': ('>', 1e6),
               b'\x4d\x3c\xb2\xa1': ('<', 1e9), b'\xa1\xb2\x3c\x4d': ('>', 1e9)}
    if magic in classic:
        if len(raw) < 24: raise ValueError('truncated PCAP header')
        order, clock = classic[magic]
        link = struct.unpack_from(order+'I', raw, 20)[0] & 65535
        pos = 24
        while pos < len(raw):
            if pos+16 > len(raw): raise ValueError('truncated PCAP record')
            sec, fraction, size, original = struct.unpack_from(order+'4I', raw, pos)
            pos += 16
            if size > MAX_FRAME or size > original or pos+size > len(raw):
                raise ValueError('invalid PCAP frame size')
            yield sec+fraction/clock, link, raw[pos:pos+size]
            pos += size
        return
    if magic != b'\x0a\x0d\x0d\x0a': raise ValueError('unsupported capture container')
    pos, interfaces, order = 0, [], None
    while pos < len(raw):
        if pos+12 > len(raw): raise ValueError('truncated PCAPNG block')
        if raw[pos:pos+4] == b'\x0a\x0d\x0d\x0a':
            bom = raw[pos+8:pos+12]
            order = '<' if bom == b'\x4d\x3c\x2b\x1a' else '>' if bom == b'\x1a\x2b\x3c\x4d' else None
            if order is None: raise ValueError('invalid PCAPNG byte order')
            interfaces = []
        if order is None: raise ValueError('PCAPNG section missing')
        kind, size = struct.unpack_from(order+'2I', raw, pos)
        if size < 12 or size % 4 or size > MAX_FRAME+4096 or pos+size > len(raw):
            raise ValueError('invalid PCAPNG block size')
        if struct.unpack_from(order+'I', raw, pos+size-4)[0] != size:
            raise ValueError('PCAPNG trailing length mismatch')
        body = raw[pos+8:pos+size-4]
        pos += size
        if kind == 0x0a0d0d0a:
            if len(body) < 16: raise ValueError('short PCAPNG section')
        elif kind == 1:
            if len(body) < 8 or len(interfaces) >= 256: raise ValueError('invalid PCAPNG interface')
            link, _, snap = struct.unpack_from(order+'HHI', body)
            clock, offset, p = 1e6, 0, 8
            while p+4 <= len(body):
                code, length = struct.unpack_from(order+'HH', body, p); p += 4
                if p+length > len(body): raise ValueError('invalid PCAPNG option')
                value = body[p:p+length]; p += (length+3)&~3
                if code == 0: break
                if code == 9 and length == 1:
                    exponent = value[0]&127
                    if exponent > 63: raise ValueError('invalid PCAPNG timestamp resolution')
                    clock = (2 if value[0]&128 else 10)**exponent
                if code == 14 and length == 8: offset = struct.unpack(order+'q', value)[0]
            interfaces.append((link, snap, clock, offset))
        elif kind in (2, 6):
            if len(body) < 20: raise ValueError('short PCAPNG packet')
            interface, hi, lo, captured, original = struct.unpack_from(order+'5I', body)
            if kind == 2: interface = struct.unpack_from(order+'H', body)[0]
            if interface >= len(interfaces): raise ValueError('unknown PCAPNG interface')
            if captured > MAX_FRAME or captured > original or 20+captured > len(body):
                raise ValueError('invalid PCAPNG packet size')
            link, _, clock, offset = interfaces[interface]
            yield ((hi << 32)|lo)/clock+offset, link, body[20:20+captured]
        elif kind == 3:
            if len(body) < 4 or not interfaces: raise ValueError('invalid PCAPNG simple packet')
            original = struct.unpack_from(order+'I', body)[0]
            link, snap, _, _ = interfaces[0]
            captured = min(original, snap or original)
            if captured > MAX_FRAME or captured+4 > len(body): raise ValueError('short PCAPNG simple packet')
            yield None, link, body[4:4+captured]


def datagrams(raw):
    for timestamp, link, frame in frames(raw):
        if link == 1:
            if len(frame) < 14: continue
            protocol = struct.unpack_from('>H', frame, 12)[0]; pos = 14
            for _ in range(2):
                if protocol not in (0x8100, 0x88a8): break
                if pos+4 > len(frame): break
                protocol = struct.unpack_from('>H', frame, pos+2)[0]; pos += 4
            if protocol != 0x800: continue
            ip = frame[pos:]
        elif link == 101: ip = frame
        elif link == 113 and len(frame) >= 16 and frame[14:16] == b'\x08\0': ip = frame[16:]
        elif link == 276 and len(frame) >= 20 and frame[:2] == b'\x08\0': ip = frame[20:]
        else: continue
        if len(ip) < 20: continue
        ih = (ip[0]&15)*4
        total = struct.unpack_from('>H', ip, 2)[0] or len(ip)
        if ip[0]>>4 != 4 or ih < 20 or total > len(ip) or total < ih+8 or ip[9] != 17:
            continue
        if struct.unpack_from('>H', ip, 6)[0]&0x3fff: continue  # no IP-fragment guessing
        udp = ip[ih:total]; size = struct.unpack_from('>H', udp, 4)[0]
        if size < 8 or size > len(udp): continue
        # Endpoint bytes are used only to isolate reassembly, never exported.
        yield timestamp, (ip[12:20], udp[:4]), udp[8:size]


@dataclass
class Message:
    time: float | None
    queue: int
    data: bytes


def fragments(payload):
    if len(payload) < 20: return []
    flags = struct.unpack_from('<I', payload, 4)[0]
    allowed = 2|4|0x1000|0x2000|0x4000|0x1000000|0x2000000|0x4000000|0x8000000
    if not flags&4 or flags&~allowed or len(payload) != 20+struct.unpack_from('<H', payload, 16)[0]: return []
    offset = 20
    for flag, size in [(0x1000,None),(0x2000,None),(0x4000,4),(0x1000000,8),(0x2000000,4),(0x4000000,8),(0x8000000,6)]:
        if flags&flag:
            if size is None:
                if offset+4 > len(payload): raise ValueError('truncated optional packet header')
                count = struct.unpack_from('<I', payload, offset)[0]
                if count > 256: raise ValueError('optional header count limit')
                size = 4+4*count
            offset += size
    if offset > len(payload): raise ValueError('truncated optional packet fields')
    result = []
    while offset < len(payload):
        if offset+16 > len(payload): raise ValueError('truncated fragment header')
        blob,count,size,index,queue = struct.unpack_from('<Q4H',payload,offset)
        # Offline patch captures legitimately contain 4,066-fragment DAT records.
        # Live transport admission limits are deliberately unchanged.
        if not 0 < count <= 4096 or index >= count or not 16 < size <= 464 or offset+size > len(payload):
            raise ValueError('invalid fragment bounds')
        result.append((blob,count,index,queue,payload[offset+16:offset+size]))
        offset += size
    return result


def reassemble(raw, issues=None):
    groups, done, storage = OrderedDict(), OrderedDict(), 0
    for timestamp, flow, payload in datagrams(raw):
        try: parts_in_packet = fragments(payload)
        except ValueError as error:
            if issues is None: raise
            issues[str(error)] += 1
            continue
        for blob,count,index,queue,fragment in parts_in_packet:
            key = (flow,payload[12:14],payload[18:20],blob,queue,count)
            if key in done: continue
            parts = groups.setdefault(key,{})
            if index in parts and parts[index] != fragment:
                if issues is None: raise ValueError('conflicting duplicate fragment')
                issues['conflicting duplicate fragment'] += 1
                storage -= sum(map(len,groups.pop(key).values()));done[key] = None
                if len(done) > 131072: done.popitem(last=False)
                continue
            if index not in parts: storage += len(fragment); parts[index] = fragment
            if sum(map(len,parts.values())) > 2*1024*1024: raise ValueError('capture reassembly limit')
            if len(parts) == count:
                del groups[key]; storage -= sum(map(len,parts.values())); done[key] = None
                if len(done) > 131072: done.popitem(last=False)
                yield Message(timestamp,queue,b''.join(parts[i] for i in range(count)))
            while len(groups) > 8192 or storage > 16*1024*1024:
                _, stale = groups.popitem(last=False); storage -= sum(map(len,stale.values()))
                if issues is not None: issues['incomplete assembly evicted'] += 1
    if issues is not None and groups: issues['incomplete assemblies at EOF'] += len(groups)


def messages(raw):
    for message in reassemble(raw): yield message.data
