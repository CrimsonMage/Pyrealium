"""Importer regressions for differing Legions effect layouts and typed tables."""
import struct
import unittest
from import_skill_effects import NODE, ROOT, ROW, LIST, effect_record, value_rows


class Portal:
    def __init__(self, data):
        self.entries = {1: True}
        self.data = data

    def read(self, did):
        return self.data


def effect(kind, slot):
    # Four graph objects: root, magnitude list, row, and duration list.
    fields = [0] * 39
    fields[0], fields[2], fields[3], fields[slot] = 3, 0x3f800000, kind, 1
    words = [0, 0, 4, 0, 0, 0, 0]
    words += [ROOT, 0, 0, len(fields), *fields]
    words += [NODE, 0, LIST, 1, 2]
    words += [NODE, 0, ROW, 2, 0x41c80000, 0x3f800000]  # 25 at rank 1
    words += [NODE, 0, LIST, 0]
    return struct.pack(f'<{len(words)}I', *words)


class SkillEffects(unittest.TestCase):
    def test_7230_layout_is_explicit_and_does_not_change_legions(self):
        for kind in (0x21, 0x22, 0x11, 0x12):
            portal = Portal(effect(kind, 37))
            self.assertEqual(effect_record(portal, 1, set(), profile='7230')[0]['values'], [(1.0, 25.0)])
            self.assertEqual(effect_record(portal, 1, set())[0]['values'], [])

    def test_local_vital_layouts_are_distinct(self):
        for kind, slot in [(0x21, 36), (0x22, 36), (0x11, 38), (0x12, 38)]:
            result = effect_record(Portal(effect(kind, slot)), 1, set())[0]
            self.assertEqual(result['values'], [(1.0, 25.0)])
            self.assertEqual(result['duration'], [])
        # A later-build field must not silently become a duration or root field.
        self.assertEqual(effect_record(Portal(effect(0x11, 37)), 1, set())[0]['values'], [])

    def test_scalar_zero_and_untyped_nodes_are_not_value_tables(self):
        self.assertEqual(value_rows([ROOT, 0, 0, 0], [(ROOT, 0)], 0), [])
        self.assertEqual(value_rows([NODE, 0, 123, 0], [(NODE, 0)], 0), [])
        self.assertEqual(value_rows([], [], 50), [])


if __name__ == '__main__':
    unittest.main()
