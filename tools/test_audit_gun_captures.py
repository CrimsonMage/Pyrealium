import json
import struct
import unittest

from audit_gun_captures import inspect
from test_capture_packets import pcap


def packet(payload, port=2310, server=False):
    ports = (port, 2022) if server else (2022, port)
    udp = struct.pack('>4H', *ports, len(payload) + 8, 0) + payload
    ip = bytearray(20)
    ip[0], ip[9] = 0x45, 17
    ip[12:20] = bytes(range(8))
    struct.pack_into('>H', ip, 2, len(udp) + 20)
    return bytes(12) + b'\x08\0' + ip + udp


def message(xml, flags=0x003f):
    return struct.pack('<HHH', flags, 0x1234, 0xc350) + xml + b'\0'


class GunCaptureTest(unittest.TestCase):
    def test_native_udp_schema_and_sensitive_values_are_separate(self):
        raw = pcap([
            packet(message(b'<FriendAdd Cx="0x00000001" User="private-player"/>')),
            packet(message(b'<GetFriendsListRes Cx="0x00000001" HR="0x0" Cnt="1">'
                           b'<FriendStatus User="private-player" Mode="1" Location="private-place"/>'
                           b'</GetFriendsListRes>', 0x0037), server=True),
            packet(message(b'<PresentTicketCmd Cx="0x2" Email="private-email">'
                           b'<Ticket Val="private-token"/></PresentTicketCmd>')),
        ])
        result = inspect(raw)
        self.assertEqual(result['counts']['complete_xml_messages'], 3)
        self.assertEqual(result['counts']['server_to_client'], 1)
        self.assertEqual(result['attributes']['FriendAdd'], {'Cx': 1, 'User': 1})
        self.assertEqual(result['elements']['FriendStatus'], 1)
        self.assertNotIn('private', json.dumps(result))

    def test_framing_ignores_tcp_style_payload_and_unrelated_ports(self):
        xml = b'<FriendAdd Cx="0x1" User="test"/>'
        raw = pcap([packet(message(xml), port=9000), packet(struct.pack('>I', len(xml)) + xml),
                    packet(bytes.fromhex('880100000600') + bytes(10)),
                    packet(bytes.fromhex('800601000100') + bytes(6))])
        result = inspect(raw)
        self.assertEqual(result['counts']['client_to_server'], 3)
        self.assertFalse(result['elements'])

    def test_fragments_and_malformed_xml_are_not_guessed(self):
        raw = pcap([packet(message(b'<FriendAdd', 0x001f)),
                    packet(message(b'<FriendAdd/>', 0x002f)),
                    packet(message(b'<FriendAdd')), packet(message(b'<!DOCTYPE X><FriendAdd/>')),
                    packet(message(b'<private-tag private-attribute="private-value"/>'))])
        result = inspect(raw)
        self.assertEqual(result['counts']['fragmented_datagrams_not_decoded'], 2)
        self.assertEqual(result['counts']['invalid_or_incomplete_xml'], 1)
        self.assertEqual(result['counts']['unsupported_xml'], 1)
        self.assertNotIn('private', json.dumps(result))

    def test_truncated_container_rejects_partial_evidence(self):
        raw = pcap([packet(message(b'<GetFriendsList Cx="0x1"/>'))])
        with self.assertRaises(ValueError):
            inspect(raw[:-1])


if __name__ == '__main__':
    unittest.main()
