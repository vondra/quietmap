#!/usr/bin/env python3
"""Regression for per-cell obstacle emptiness (the promotion rule).

Locks the three answers a cell can give, because conflating any two of them
paints a loud place quiet: MERGED (staging shards exist), EMPTY (the finished
sweep found nothing) and UNFINISHED (the sweep has not reached the cell, so no
answer may be written at all).
"""

import importlib.util
import os
import shutil
import tempfile
import unittest
from pathlib import Path

import h3
import pyarrow as pa
import pyarrow.ipc as ipc
import shapely

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("promotion", HERE / "enrich-obstacle-heights.py")
PROMOTION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROMOTION)

# The footprint's shape is irrelevant here: the rule under test is which FILE
# exists, not what the height ladder does to a polygon.
WKB = shapely.to_wkb(shapely.box(14.17, 49.78, 14.1701, 49.7801), output_dimension=2)
CELL = "841e309ffffffff"  # Dobris/Praha R4 cell, four degree tiles in its bbox
GHSL_HEIGHT_M = 12.5


class FakeGlobalPrior:
    """ANBH stand-in: the raster's own reads are not what this test asserts."""

    def sample(self, _lon, _lat):
        return GHSL_HEIGHT_M


def staging_shard(path, tile_rows, with_envelope):
    columns = {
        "polygon_wkb": pa.array([WKB] * tile_rows, pa.binary()),
        "height_m": pa.array([8.0] * tile_rows, pa.float32()),
        "centroid_lat": pa.array([49.78] * tile_rows, pa.float64()),
        "centroid_lon": pa.array([14.17] * tile_rows, pa.float64()),
        "height_tier": pa.array([2] * tile_rows, pa.uint8()),
    }
    if with_envelope:
        columns["envelope_class"] = pa.array([1] * tile_rows, pa.uint8())
    table = pa.table(columns)
    path.parent.mkdir(parents=True, exist_ok=True)
    with ipc.new_file(str(path), table.schema) as writer:
        writer.write_table(table)


class ObstaclePromotionTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.h3r4 = self.root / "h3r4"
        self.staging = self.root / "staging"
        (self.h3r4 / CELL).mkdir(parents=True)
        self.staging.mkdir()
        self.tiles = self.root / ".ingested-tiles"

    def swept(self, listed):
        self.tiles.write_text("".join(f"{tile}\n" for tile in listed), encoding="utf-8")
        return PROMOTION.SweptCells(str(self.tiles), h3)

    def materialize(self, swept):
        return PROMOTION.enrich_cell(
            CELL, str(self.h3r4), str(self.staging), FakeGlobalPrior(), None, swept
        )

    def output(self):
        return ipc.open_file(str(self.h3r4 / CELL / "obstacles.arrow"))

    def test_staged_cell_is_materialized_from_shards_of_both_ingest_eras(self):
        # One cell straddles degree tiles ingested before and after the
        # envelope_class column existed; Arrow refuses to concatenate those
        # schemas, so the merge has to unify them (~1 in 150 staged cells).
        staging_shard(self.staging / CELL / "obstacles-N49E014.arrow", 2, with_envelope=True)
        staging_shard(self.staging / CELL / "obstacles-N50E014.arrow", 3, with_envelope=False)
        # A staged cell needs no sweep proof; the shards are the answer.
        self.assertTrue(self.materialize(self.swept(["N00E000"])))
        table = self.output().read_all()
        self.assertEqual(table.num_rows, 5)
        self.assertEqual(table.column_names, PROMOTION.SCHEMA.names)
        self.assertEqual(
            table.column("envelope_class").to_pylist(),
            [1, 1, PROMOTION.ENVELOPE_CLASS_DEFAULT, PROMOTION.ENVELOPE_CLASS_DEFAULT,
             PROMOTION.ENVELOPE_CLASS_DEFAULT],
        )

    def test_swept_cell_without_shards_becomes_an_empty_table(self):
        swept = self.swept(PROMOTION.world_tile_census.cell_degree_tiles(CELL, h3))
        self.assertTrue(self.materialize(swept))
        reader = self.output()
        self.assertEqual(reader.read_all().num_rows, 0)
        self.assertEqual(reader.schema.names, PROMOTION.SCHEMA.names)

    def test_unfinished_sweep_writes_nothing(self):
        tiles = PROMOTION.world_tile_census.cell_degree_tiles(CELL, h3)
        self.assertFalse(self.materialize(self.swept(tiles[:-1])))
        self.assertFalse((self.h3r4 / CELL / "obstacles.arrow").exists())

    def test_a_cell_outside_the_prepared_inventory_is_refused(self):
        # Materializing one would ADD a cell to the world the orchestrator
        # plans over; the obstacle store follows the inventory, never extends it.
        shutil.rmtree(self.h3r4 / CELL)
        with self.assertRaises(SystemExit):
            self.materialize(self.swept(PROMOTION.world_tile_census.cell_degree_tiles(CELL, h3)))
        self.assertFalse((self.h3r4 / CELL).exists())

    def test_rows_without_staging_refuse_to_be_emptied(self):
        staging_shard(self.staging / CELL / "obstacles-N49E014.arrow", 2, with_envelope=True)
        swept = self.swept(PROMOTION.world_tile_census.cell_degree_tiles(CELL, h3))
        self.assertTrue(self.materialize(swept))
        os.remove(self.staging / CELL / "obstacles-N49E014.arrow")
        with self.assertRaises(SystemExit):
            self.materialize(swept)
        self.assertEqual(self.output().read_all().num_rows, 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
