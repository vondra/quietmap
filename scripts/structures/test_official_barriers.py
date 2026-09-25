"""Official barrier cache, OSM replacement, hop and normalizer regressions."""

import json
import math
import os
from pathlib import Path
import tempfile
import unittest

import pyarrow.ipc as ipc
import shapely

import normalize_barrier_cache as NORMALIZE
import official_barriers as OFFICIAL
from structure_inputs import read_official_cache
from test_structures_fixtures import (
    BUILDER, CONTRACT, GRID, SQUARE, FakeGlobalPrior, buildings_arrow, barriers_arrow,
    osm_row, OSM_POLY,
)

LAT = 49.78
LON = 14.17
LON_METRE = 111_320.0 * math.cos(math.radians(LAT))


def official_row(lon, height_m=4.0, kind=0, span=0.002, lat=LAT):
    return {"geom": shapely.LineString([(lon, lat), (lon, lat + span)]),
            "clat": lat + span / 2, "clon": lon, "height_m": height_m,
            "measured": True, "kind": kind, "source": "TEST", "as_of": "2026-01-01"}


def replacement(offsets_m, kind=0):
    tree, framed, reference = OFFICIAL.replacement_tree([official_row(LON, kind=kind)])
    if tree is None:
        return [False for _ in offsets_m]
    return [OFFICIAL.osm_segment_is_replaced(
        LON + dx / LON_METRE, LAT, LON + dx / LON_METRE, LAT + 0.001,
        tree, framed, reference) for dx in offsets_m]


class ReplacementTests(unittest.TestCase):
    def test_same_wall_is_replaced_and_a_parallel_wall_is_kept(self):
        self.assertEqual(replacement([3.0, 8.0]), [True, False])

    def test_berm_never_replaces_an_osm_wall(self):
        self.assertEqual(replacement([3.0], kind=OFFICIAL.KIND_BERM), [False])

    def test_combined_replaces_like_a_wall(self):
        self.assertEqual(replacement([3.0], kind=OFFICIAL.KIND_COMBINED), [True])

    def test_no_official_lines_replaces_nothing(self):
        self.assertEqual(OFFICIAL.replacement_tree([]), (None, [], 0.0))


class SplitHopTests(unittest.TestCase):
    def test_long_hop_chops_at_the_osm_cap(self):
        hops = OFFICIAL.split_hops([(LON, LAT), (LON, LAT + 0.006)])  # ~668 m
        self.assertEqual(len(hops), 3)
        for (_, _), (_, _), length in hops:
            self.assertLessEqual(length, OFFICIAL.HOP_CAP_M)
        self.assertAlmostEqual(sum(length for _, _, length in hops), 667.9, delta=1.0)

    def test_short_hop_passes_through(self):
        hops = OFFICIAL.split_hops([(LON, LAT), (LON + 0.001, LAT)])
        self.assertEqual(len(hops), 1)
        self.assertAlmostEqual(hops[0][2], LON_METRE * 0.001, delta=1.0)

    def test_dateline_hop_chops_the_short_way(self):
        hops = OFFICIAL.split_hops([(179.9, 0.0), (-179.9, 0.0)])
        total = sum(length for _, _, length in hops)
        self.assertAlmostEqual(total, 0.2 * 111_320.0, delta=10.0)
        for (lon0, _), (lon1, _), length in hops:
            self.assertLessEqual(length, OFFICIAL.HOP_CAP_M)
            self.assertLess(abs(GRID.wrapped_longitude_delta(lon0, lon1)), 1.0)


class CacheRoundtripTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.cache = os.path.join(self.temporary.name, "barriers")

    def test_tiles_roundtrip_and_squares_own_by_centroid(self):
        from structure_inventory import degree_name
        rows = [official_row(LON), official_row(-122.3, height_m=3.4, lat=47.6)]
        by_tile = {}
        for row in rows:
            tile = degree_name(math.floor(row["clat"]), math.floor(row["clon"]))
            columns = by_tile.setdefault(tile, {name: [] for name in OFFICIAL.SCHEMA.names})
            columns["geometry"].append(shapely.to_wkb(row["geom"]))
            columns["height_m"].append(row["height_m"])
            columns["measured"].append(row["measured"])
            columns["kind"].append(row["kind"])
            columns["source"].append(row["source"])
            columns["as_of"].append(row["as_of"])
        from structure_inventory import write_official_cache
        write_official_cache(by_tile, self.cache, OFFICIAL.SCHEMA,
                             OFFICIAL.CONTRACT_KEY, OFFICIAL.CONTRACT_VERSION)
        square = GRID.square_of(LAT + 0.001, LON)
        got, files = read_official_cache(self.cache, square, OFFICIAL.SCHEMA, OFFICIAL.CONTRACT_KEY,
                                OFFICIAL.CONTRACT_VERSION)
        self.assertEqual(len(got), 1)
        self.assertAlmostEqual(got[0]["height_m"], 4.0)
        self.assertEqual(len(files), 1)
        other, _ = read_official_cache(self.cache, GRID.square_of(47.6, -122.3), OFFICIAL.SCHEMA,
                                OFFICIAL.CONTRACT_KEY, OFFICIAL.CONTRACT_VERSION)
        self.assertEqual(len(other), 1)

    def test_missing_tiles_are_absent_data(self):
        os.makedirs(self.cache)
        got, files = read_official_cache(self.cache, GRID.parse_square_name(SQUARE), OFFICIAL.SCHEMA,
                                OFFICIAL.CONTRACT_KEY, OFFICIAL.CONTRACT_VERSION)
        self.assertEqual((got, files), ([], []))


class BuildSquareOfficialTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.prepared = Path(self.temporary.name)
        (self.prepared / SQUARE).mkdir(parents=True)

    def test_official_line_replaces_osm_and_emits_its_own_rows(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0, height=6.0)])
        on_line = LON + 2.0 / LON_METRE
        far = LON + 50.0 / LON_METRE
        barriers_arrow(self.prepared / SQUARE / "barriers.arrow", [
            {"osm_id": 1, "segment_idx": 0, "start_lat": LAT, "start_lon": on_line,
             "end_lat": LAT + 0.001, "end_lon": on_line, "height": 0.0, "height_tier": 2},
            {"osm_id": 2, "segment_idx": 0, "start_lat": LAT, "start_lon": far,
             "end_lat": LAT + 0.001, "end_lon": far, "height": 3.0, "height_tier": 0},
        ])
        official = [official_row(LON, height_m=4.4)]
        census = BUILDER.build_square(
            SQUARE, self.prepared, [], [], FakeGlobalPrior(), None, official, [], [], [])
        table = ipc.open_file(self.prepared / SQUARE / "structures.arrow").read_all()
        kinds = table.column("kind").to_pylist()
        self.assertEqual(census["official_walls"], 1)
        self.assertEqual(census["replaced_osm_walls"], 1)
        self.assertGreater(census["official_wall_km"], 0.0)
        self.assertGreater(census["replaced_osm_wall_km"], 0.0)
        walls = [i for i, kind in enumerate(kinds) if kind == 1]
        self.assertEqual(len(walls), 2)
        osm_ids = table.column("osm_id").to_pylist()
        sources = table.column("height_source").to_pylist()
        heights = table.column("height_m").to_pylist()
        kept = next(i for i in walls if osm_ids[i] == 2)
        self.assertEqual(sources[kept], CONTRACT.HEIGHT_SOURCE_OSM_HEIGHT)
        added = next(i for i in walls if osm_ids[i] is None)
        self.assertEqual(sources[added], CONTRACT.HEIGHT_SOURCE_OFFICIAL_BARRIER)
        self.assertEqual(heights[added], 4)
        meta = table.schema.metadata or {}
        self.assertEqual(meta[b"official_barrier_rows"], b"1")
        self.assertEqual(meta[b"replaced_osm_barrier_rows"], b"1")
        self.assertEqual(meta[b"barrier_rows"], b"2")

    def test_berm_rows_never_screen(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0, height=6.0)])
        official = [official_row(LON, kind=OFFICIAL.KIND_BERM)]
        census = BUILDER.build_square(
            SQUARE, self.prepared, [], [], FakeGlobalPrior(), None, official, [], [], [])
        table = ipc.open_file(self.prepared / SQUARE / "structures.arrow").read_all()
        self.assertEqual(census["official_walls"], 0)
        self.assertEqual(table.column("kind").to_pylist(), [0])


class NormalizerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def write(self, name, features):
        path = self.root / name
        path.write_text(json.dumps({"type": "FeatureCollection", "features": features}),
                        encoding="utf-8")
        return str(path)

    def segment(self, top_z, kind="Scherm"):
        line = [[155000.0, 463000.0, z] for z in top_z]
        return {"geometry": {"type": "LineString", "coordinates": line},
                "properties": {"id_gw_vz": 7, "id_segment": 1, "type": kind}}

    def test_gwv_height_is_top_minus_reference(self):
        segmenten = self.write("seg.json", [self.segment([8.0, 8.2, 7.9])])
        kant = self.write("kant.json", [{
            "geometry": {"type": "LineString",
                         "coordinates": [[155000.0, 463000.0, 4.1]] * 3},
            "properties": {"id_gw_vz": 7, "id_segment": 1}}])
        rows = NORMALIZE.read_gwv(segmenten, kant)
        self.assertEqual(len(rows), 1)
        self.assertAlmostEqual(rows[0][1], 3.9, places=1)
        self.assertTrue(rows[0][2])
        self.assertEqual(rows[0][3], OFFICIAL.KIND_WALL)
        # EPSG:28992 Amersfoort reprojects to lon/lat, not stays in metres.
        self.assertTrue(5.0 < rows[0][0].centroid.x < 6.0)

    def test_gwv_broken_height_falls_back_to_the_type_median(self):
        segmenten = self.write("seg.json", [self.segment([40.0, 41.0])])
        kant = self.write("kant.json", [{
            "geometry": {"type": "LineString",
                         "coordinates": [[155000.0, 463000.0, 4.0]] * 2},
            "properties": {"id_gw_vz": 7, "id_segment": 1}}])
        rows = NORMALIZE.read_gwv(segmenten, kant)
        self.assertEqual((rows[0][1], rows[0][2]), (3.8, False))

    def test_wsdot_zero_minimum_means_unknown(self):
        path = self.write("wsdot.json", [
            {"geometry": {"type": "LineString", "coordinates": [[-122.3, 47.6], [-122.29, 47.6]]},
             "properties": {"MinHeightFt": 0, "MaxHeightFt": 12}},
            {"geometry": {"type": "LineString", "coordinates": [[-122.3, 47.6], [-122.29, 47.6]]},
             "properties": {"MinHeightFt": 10, "MaxHeightFt": 14}},
            {"geometry": {"type": "LineString", "coordinates": [[-122.3, 47.6], [-122.29, 47.6]]},
             "properties": {"MinHeightFt": 0, "MaxHeightFt": 0}},
            {"geometry": None,
             "properties": {"MinHeightFt": 10, "MaxHeightFt": 12}},
            {"geometry": {"type": "LineString", "coordinates": [[-122.3, 47.6]]},
             "properties": {"MinHeightFt": 10, "MaxHeightFt": 12}},
        ])
        rows = NORMALIZE.read_wsdot(path)
        self.assertEqual(len(rows), 3)
        self.assertAlmostEqual(rows[0][1], 12 * 0.3048, places=3)
        self.assertTrue(rows[0][2])
        self.assertAlmostEqual(rows[1][1], 12 * 0.3048, places=3)
        self.assertEqual((rows[2][1], rows[2][2]), (12.0 * 0.3048, False))

    def test_fdot_keeps_only_standing_walls_in_feet(self):
        def wall(kind, feet):
            return {"geometry": {"type": "LineString",
                                 "coordinates": [[-81.4, 28.5], [-81.39, 28.5]]},
                    "properties": {"TYPE": kind, "FED_HEIGHT": feet}}
        path = self.write("fdot.json", [
            wall("CONSTRUCTED BARRIERS", 14), wall("RECOMMENDED BARRIERS", 14),
            wall("REMOVED BARRIERS", 14), wall("REPLACED BARRIERS", 14),
            wall("CONSTRUCTED BARRIERS", 0),
        ])
        rows = NORMALIZE.read_fdot(path)
        self.assertEqual(len(rows), 2)
        self.assertAlmostEqual(rows[0][1], 14 * 0.3048, places=3)
        self.assertTrue(rows[0][2])
        self.assertEqual((rows[1][1], rows[1][2]), (14.0 * 0.3048, False))

    def test_vdot_takes_the_us_mean_unmeasured(self):
        path = self.write("vdot.json", [{
            "geometry": {"type": "LineString", "coordinates": [[-77.3, 38.8], [-77.29, 38.8]]},
            "properties": {}}])
        rows = NORMALIZE.read_vdot(path)
        self.assertEqual((rows[0][1], rows[0][2]), (4.45, False))

    def test_repeated_inventory_records_cache_once(self):
        import pyarrow.parquet as pq
        line = shapely.LineString([(-122.3, 47.6), (-122.29, 47.6)])
        cache = str(self.root / "cache")
        kept = NORMALIZE.append_cache(
            [(line, 3.0, True, 0), (line, 3.0, True, 0), (line, 4.0, True, 0)],
            "TEST", "2026-01-01", cache)
        self.assertEqual(kept, 2)
        table = pq.read_table(self.root / "cache" / "N47W123.parquet")
        self.assertEqual(table.num_rows, 2)


if __name__ == "__main__":
    unittest.main()
