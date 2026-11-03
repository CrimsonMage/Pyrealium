import struct
from collections import Counter
import unittest
from capture_packets import frames, reassemble


def fragment(data, index=0, count=1, blob=1):
    return struct.pack('<Q4H',blob,count,16+len(data),index,2)+data


def frame(parts, port=9000, optional=b'', flags=4):
    payload=struct.pack('<IIIHHHH',1,flags,0,1,0,len(optional)+len(parts),1)+optional+parts
    udp=struct.pack('>4H',port,9001,len(payload)+8,0)+payload
    ip=bytearray(20);ip[0]=0x45;ip[9]=17;ip[12:20]=bytes(range(8))
    struct.pack_into('>H',ip,2,len(udp)+20)
    return bytes(12)+b'\x08\0'+ip+udp


def pcap(items):
    result=struct.pack('<IHHIIII',0xa1b2c3d4,2,4,0,0,65535,1)
    for data in items:result+=struct.pack('<4I',3,250000,len(data),len(data))+data
    return result


def block(kind, data, endian='<'):
    data+=bytes((-len(data))%4);size=len(data)+12
    return struct.pack(endian+'II',kind,size)+data+struct.pack(endian+'I',size)


def section(endian='<'):
    return block(0x0a0d0d0a,struct.pack(endian+'IHHq',0x1a2b3c4d,1,0,-1),endian)


class CaptureTest(unittest.TestCase):
    def test_interleaved_flows_out_of_order_and_retransmission(self):
        raw=pcap([frame(fragment(b'B',1,2)),frame(fragment(b'Y',1,2),9002),
                  frame(fragment(b'B',1,2)),frame(fragment(b'A',0,2)),frame(fragment(b'X',0,2),9002),
                  frame(fragment(b'A',0,2))])
        self.assertEqual([m.data for m in reassemble(raw)],[b'AB',b'XY'])

    def test_offline_patch_records_can_exceed_live_fragment_limits(self):
        raw=pcap([frame(fragment(bytes([i%256]),i,4066)) for i in range(4066)])
        self.assertEqual([len(m.data) for m in reassemble(raw)],[4066])

    def test_bad_packet_is_discarded_atomically(self):
        raw=pcap([frame(fragment(b'unsafe')+b'bad'),frame(fragment(b'good',blob=2))])
        with self.assertRaises(ValueError):list(reassemble(raw))
        issues=Counter()
        self.assertEqual([m.data for m in reassemble(raw,issues)],[b'good'])
        self.assertEqual(issues['truncated fragment header'],1)

    def test_conflicting_blob_is_quarantined(self):
        raw=pcap([frame(fragment(b'A',0,2)),frame(fragment(b'X',0,2)),frame(fragment(b'B',1,2)),frame(fragment(b'good',blob=2))])
        issues=Counter()
        self.assertEqual([m.data for m in reassemble(raw,issues)],[b'good'])
        self.assertEqual(issues['conflicting duplicate fragment'],1)

    def test_native_optional_headers_are_skipped_in_wire_order(self):
        optional=struct.pack('<IIId',1,42,43,12.5)
        raw=pcap([frame(fragment(b'event'),optional=optional,flags=4|0x1000|0x4000|0x1000000)])
        self.assertEqual([m.data for m in reassemble(raw)],[b'event'])

    def test_pcapng_multiple_interfaces_clocks_and_byte_orders(self):
        data=frame(fragment(b'event'));raw=b''
        for order in ('<','>'):
            raw+=section(order)
            raw+=block(1,struct.pack(order+'HHI',1,0,65535),order)
            options=struct.pack(order+'HHB3xHHqHH',9,1,0x8a,14,8,10,0,0)
            raw+=block(1,struct.pack(order+'HHI',1,0,65535)+options,order)
            raw+=block(6,struct.pack(order+'5I',1,0,1536,len(data),len(data))+data,order)
            # Obsolete packet block: 16-bit interface then 16-bit drop count.
            raw+=block(2,struct.pack(order+'HH4I',1,7,0,1536,len(data),len(data))+data,order)
        decoded=list(frames(raw))
        self.assertEqual([t for t,_,_ in decoded],[11.5]*4)
        self.assertTrue(all(f==data for _,_,f in decoded))

    def test_container_truncation_fails_closed(self):
        raw=pcap([frame(fragment(b'event'))])
        with self.assertRaises(ValueError):list(frames(raw[:-1]))
        raw=section()+block(1,struct.pack('<HHI',1,0,65535))
        with self.assertRaises(ValueError):list(frames(raw[:-4]+bytes(4)))

if __name__=='__main__':unittest.main()
