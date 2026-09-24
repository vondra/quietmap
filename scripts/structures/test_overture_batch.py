"""Mixed Overture batches retain ownership, exclusion and original source order."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import pyarrow as pa
import pyarrow.parquet as pq
import shapely

from test_structures_fixtures import SOURCES, GRID, SQUARE, OSM_POLY, OVT_LONELY


class OvertureBatchTests(unittest.TestCase):
    def test_mixed_batch_excludes_non_stock_before_decoding_and_keeps_order(self):
        geoms = [shapely.to_wkb(OSM_POLY), b'invalid underground geometry', None,
                 shapely.to_wkb(shapely.Point(14.17, 49.78)),
                 shapely.to_wkb(shapely.Polygon()),
                 shapely.to_wkb(shapely.box(14.1, 50.1, 14.2, 50.2)),
                 shapely.to_wkb(OVT_LONELY)]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'source.parquet'
            pq.write_table(pa.table({
                'geometry': pa.array(geoms, type=pa.binary()),
                'height': [4.5, 100., None, None, None, None, None],
                'num_floors': [None, None, None, None, None, None, 3],
                'class': ['carport', None, None, None, None, None, 'garage'],
                'subtype': ['commercial'] * 7,
                'is_underground': [None, True, False, False, False, False, False],
            }), path)
            with patch.object(SOURCES, 'overture_sources', return_value=[(49, 14, path)]):
                rows, inputs = SOURCES.read_overture_parquet(directory, GRID.parse_square_name(SQUARE))
        self.assertEqual(inputs, [path])
        self.assertEqual([r['wkb'] for r in rows], [geoms[0], geoms[-1]])
        self.assertEqual([(r['overture_height'], r['overture_floors'], r['open_roof'], r['envelope'])
                          for r in rows], [(4.5, 0, True, 0), (None, 3, False, 5)])
        for row, geom in zip(rows, [OSM_POLY, OVT_LONELY]):
            self.assertEqual((row['clat'], row['clon']), SOURCES.footprint_centroid(geom))


if __name__ == '__main__':
    unittest.main()
