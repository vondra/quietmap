"""National measured heights: cache join, reader and normalizer regressions."""

import json
from pathlib import Path
import tempfile
import unittest

import pyarrow.ipc as ipc
import shapely

import measured_heights as MEASURED
import normalize_heights_cache as NORMALIZE
from structure_inputs import read_official_cache
from test_structures_fixtures import (
    BUILDER, CONTRACT, GRID, SQUARE, buildings_arrow, osm_row,
    OSM_POLY,
)


def candidate(polygon):
    return {"geom": polygon, "regional_m": None}


def measured_row(polygon, height_m=9.5):
    centroid = polygon.centroid
    return {"geom": polygon, "clat": centroid.y, "clon": centroid.x,
            "height_m": height_m, "source": "TEST", "as_of": "2026-01-01"}


class JoinTests(unittest.TestCase):
    def test_covering_footprint_answers(self):
        rows = [measured_row(OSM_POLY)]
        ours = [candidate(OSM_POLY)]
        self.assertEqual(MEASURED.apply_measured_heights(ours, rows), 1)
        self.assertEqual(ours[0]["regional_m"], 9.5)

    def test_distant_footprint_abstains(self):
        rows = [measured_row(shapely.box(14.5, 49.9, 14.5001, 49.9001))]
        ours = [candidate(OSM_POLY)]
        self.assertEqual(MEASURED.apply_measured_heights(ours, rows), 0)
        self.assertIsNone(ours[0]["regional_m"])

    def test_vector_wins_over_the_raster_survey(self):
        rows = [measured_row(OSM_POLY, height_m=7.25)]
        ours = [candidate(OSM_POLY)]
        ours[0]["regional_m"] = 12.0
        self.assertEqual(MEASURED.apply_measured_heights(ours, rows), 1)
        self.assertEqual(ours[0]["regional_m"], 7.25)

    def test_empty_cache_answers_nothing(self):
        ours = [candidate(OSM_POLY)]
        self.assertEqual(MEASURED.apply_measured_heights(ours, []), 0)
        self.assertIsNone(ours[0]["regional_m"])


class ReaderTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.cache = str(Path(self.temporary.name) / "heights")

    def test_tiles_roundtrip_and_contract_mismatch_fails(self):
        import pyarrow.parquet as pq
        from structure_inventory import write_official_cache
        write_official_cache(
            {"N49E014": {
                "geometry": [shapely.to_wkb(OSM_POLY)],
                "height_m": [7.25], "source": ["TEST"], "as_of": ["2026-01-01"]}},
            self.cache, MEASURED.SCHEMA, MEASURED.CONTRACT_KEY, MEASURED.CONTRACT_VERSION)
        square = GRID.square_of(OSM_POLY.centroid.y, OSM_POLY.centroid.x)
        rows, files = read_official_cache(self.cache, square, MEASURED.SCHEMA, MEASURED.CONTRACT_KEY,
                                MEASURED.CONTRACT_VERSION)
        self.assertEqual(len(rows), 1)
        self.assertAlmostEqual(rows[0]["height_m"], 7.25)
        table = pq.read_table(files[0])
        pq.write_table(table.replace_schema_metadata({}), files[0])
        with self.assertRaises(SystemExit):
            read_official_cache(self.cache, square, MEASURED.SCHEMA, MEASURED.CONTRACT_KEY,
                                MEASURED.CONTRACT_VERSION)


class BuildSquareMeasuredTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.prepared = Path(self.temporary.name)
        (self.prepared / SQUARE).mkdir(parents=True)

    def test_measured_height_takes_ladder_rung_1(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0)])
        census = BUILDER.build_square(
            SQUARE, self.prepared, [], [], None,
            [], [], [measured_row(OSM_POLY, height_m=7.25)], [])
        table = ipc.open_file(self.prepared / SQUARE / "structures.arrow").read_all()
        self.assertEqual(census["measured"], 1)
        self.assertEqual(table.column("height_source").to_pylist(),
                         [CONTRACT.HEIGHT_SOURCE_REGIONAL_MEASURED])
        self.assertEqual(table.column("height_m").to_pylist(), [7])


class NormalizerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_nrw_lod1_takes_part_heights_and_ground_rings(self):
        gml = """<?xml version="1.0" encoding="UTF-8"?>
<core:CityModel xmlns:bldg="http://www.opengis.net/citygml/building/1.0"
    xmlns:gml="http://www.opengis.net/gml" xmlns:core="http://www.opengis.net/citygml/1.0">
  <core:cityObjectMember><bldg:Building gml:id="B1">
    <bldg:measuredHeight>9.0</bldg:measuredHeight>
    <bldg:lod1Solid><gml:Solid><gml:exterior><gml:CompositeSurface>
      <gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing>
        <gml:posList srsDimension="3">369000 5765000 80 369010 5765000 80
        369010 5765010 80 369000 5765010 80 369000 5765000 80</gml:posList>
      </gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>
      <gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing>
        <gml:posList srsDimension="3">369000 5765000 71 369010 5765000 71
        369010 5765010 71 369000 5765010 71 369000 5765000 71</gml:posList>
      </gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>
    </gml:CompositeSurface></gml:exterior></gml:Solid></bldg:lod1Solid>
  </bldg:Building></core:cityObjectMember></core:CityModel>"""
        path = self.root / "tile.gml"
        path.write_text(gml, encoding="utf-8")
        rows = NORMALIZE.read_nrw_lod1([str(path)])
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][1], 9.0)
        # UTM32 Legden reprojects to lon/lat, not stays in metres.
        self.assertTrue(6.0 < rows[0][0].centroid.x < 8.0)
        self.assertTrue(51.0 < rows[0][0].centroid.y < 53.0)

    def test_heights_geojson_explodes_multipolygons(self):
        path = self.root / "window.geojson"
        path.write_text(json.dumps({"type": "FeatureCollection", "features": [
            {"geometry": {"type": "MultiPolygon", "coordinates": [
                [[[5.39, 52.15], [5.391, 52.15], [5.391, 52.151], [5.39, 52.151],
                 [5.39, 52.15]]],
                [[[5.392, 52.15], [5.393, 52.15], [5.393, 52.151], [5.392, 52.151],
                 [5.392, 52.15]]]]},
             "properties": {"height_m": 8.5}}]}), encoding="utf-8")
        rows = NORMALIZE.read_heights_geojson([str(path)])
        self.assertEqual(len(rows), 2)
        self.assertEqual([height for _, height in rows], [8.5, 8.5])

    def test_heights_geojson_skips_a_missing_height(self):
        path = self.root / "broken.geojson"
        path.write_text(json.dumps({"type": "FeatureCollection", "features": [
            {"geometry": {"type": "Polygon", "coordinates":
                [[[5.39, 52.15], [5.391, 52.15], [5.391, 52.151], [5.39, 52.15]]]},
             "properties": {}}]}), encoding="utf-8")
        self.assertEqual(NORMALIZE.read_heights_geojson([str(path)]), [])


if __name__ == "__main__":
    unittest.main()
