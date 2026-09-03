#!/usr/bin/env python3
"""Regression for the Planet-derived obstacle download census."""

import importlib.util
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("world_tile_census", HERE / "world-tile-census.py")
CENSUS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CENSUS)


class FakeH3:
    valid = "841e309ffffffff"

    @classmethod
    def is_valid_cell(cls, cell):
        return cell == cls.valid

    @staticmethod
    def get_resolution(_cell):
        return 4

    @staticmethod
    def cell_to_boundary(_cell):
        return [(49.1, 14.1), (49.1, 14.9), (49.9, 14.9), (49.9, 14.1)]


class WorldTileCensusTests(unittest.TestCase):
    def test_prepared_inventory_ignores_non_h3_directories(self):
        with tempfile.TemporaryDirectory() as root:
            Path(root, FakeH3.valid).mkdir()
            Path(root, "000000000000000").mkdir()
            self.assertEqual(CENSUS.census(root, FakeH3), ["N49E014"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
