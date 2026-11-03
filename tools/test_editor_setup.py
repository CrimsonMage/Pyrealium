"""Setup connection points and multi-part visual assembly (synthetic records)."""
import math
import struct
import unittest

from editor_setup import IDENTITY, apply, compose, decode_setup, place_groups, placed_mesh, rest_pose
from preview_assets import visual_record


def transform(offset=(0, 0, 0), scale=(1, 1, 1), quaternion=(1, 0, 0, 0)):
    return struct.pack("<3f3f4f", *offset, *scale, *quaternion)


def setup_record(did, bones, rest, connections):
    """bones: parent per bone (None for a root); rest: one TransformBase per bone;
    connections: [(point id, bone, transform bytes)]."""
    out = struct.pack("<II", did, 0) + struct.pack("<I", 1) + struct.pack("<I", 0x06000001)
    out += struct.pack("<I", len(bones)) + b"".join(
        struct.pack("<II", 0xFFFFFFFF if parent is None else parent, 0) for parent in bones)
    out += struct.pack("<I", 1) + struct.pack("<II", 1, len(rest)) + b"".join(rest)
    out += struct.pack("<I", 0)  # holding locations
    out += struct.pack("<I", len(connections)) + b"".join(
        struct.pack("<II", point, bone) + data for point, bone, data in connections)
    return out + b"\0" * 64  # cylspheres, spheres and the rest are not read


class SetupTests(unittest.TestCase):
    def test_child_group_sits_at_the_parents_connection_point(self):
        half_turn = (math.cos(math.pi / 4), 0, 0, math.sin(math.pi / 4))  # 90 degrees about z
        body = setup_record(0x04000001, [None, 0], [transform((0, 0, 1)), transform((0, 0, 0.5), quaternion=half_turn)],
                            [(7, 1, transform((1, 0, 0)))])
        setups = {0x04000001: decode_setup(body, 0x04000001)}
        pose = rest_pose(setups[0x04000001])
        self.assertEqual([round(v, 6) for v in pose[1][0]], [0, 0, 1.5])
        groups = [dict(key=2, parent=1, connection=7, setup=0x04000002), dict(key=1, setup=0x04000001),
                  dict(key=3, parent=9, setup=0x04000003)]
        placed = place_groups(groups, setups.__getitem__)
        # The orphan (parent 9) is not drawn; the child follows its parent.
        self.assertEqual([setup for setup, unused in placed], [0x04000001, 0x04000002])
        self.assertEqual(placed[0][1], IDENTITY)
        # The point's 1 m along x is turned onto y by bone 1's quarter turn, then lifted to 1.5 m.
        self.assertEqual([round(v, 6) for v in apply(placed[1][1], (0, 0, 0))], [0, 1, 1.5])
        mesh = dict(vertices=[[0, 0, 0], [1, 0, 0]], normals=[[1, 0, 0], [1, 0, 0]])
        moved = placed_mesh(mesh, placed[1][1])
        self.assertTrue(moved["attached"])
        self.assertEqual([round(v, 6) + 0 for v in moved["vertices"][1]], [0, 2, 1.5])
        self.assertEqual([round(v, 6) for v in moved["normals"][0]], [0, 1, 0])
        self.assertIs(placed_mesh(mesh, IDENTITY), mesh)

    def test_a_missing_connection_point_leaves_the_child_at_its_parents_root(self):
        body = setup_record(0x04000001, [None], [transform()], [])
        lifted = compose(IDENTITY, ((0, 0, 2), (1, 1, 1), (1, 0, 0, 0)))
        placed = place_groups([dict(key=1, setup=0x04000001), dict(key=2, parent=1, connection=5, setup=0x04000002)],
                              {0x04000001: decode_setup(body, 0x04000001)}.__getitem__)
        self.assertEqual(placed[1][1], placed[0][1])
        self.assertEqual(apply(lifted, (0, 0, 0)), (0, 0, 2))


class VisualIconTests(unittest.TestCase):
    def test_icon_layers_are_read_and_the_groups_follow(self):
        did = 0x1f000001
        group = struct.pack("<I", 1 | 8) + struct.pack("<II", 5, 0x04000001)
        data = (struct.pack("<II", 1 | 0x80 | 0x200, did) + struct.pack("<I", 2)
                + struct.pack("<3I", 1, 0x41000001, 0) + struct.pack("<3I", 2, 0x41000002, 0xff)
                + struct.pack("<I", 1) + struct.pack("<I", 5) + group)
        record = visual_record(data, did)
        self.assertEqual(record["icons"], [(1, 0x41000001, 0), (2, 0x41000002, 0xff)])
        self.assertEqual(record["groups"][5]["setup"], 0x04000001)


if __name__ == "__main__":
    unittest.main()
