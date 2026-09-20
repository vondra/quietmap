"""Block sampling must preserve scalar GHSL pixels and the regional height ladder."""

import math
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock

import numpy as np
import rasterio
from affine import Affine
import shapely

from test_structures_fixtures import SOURCES


def scalar_sample(prior, lon, lat):
    x, y = prior.tr.transform(lon, lat)
    ci = int((x - prior.gt.c) / prior.gt.a)
    ri = int((y - prior.gt.f) / prior.gt.e)
    if not (0 <= ci < prior.w and 0 <= ri < prior.h):
        return np.nan
    value = float(prior.ds.read(1, window=((ri, ri + 1), (ci, ci + 1)))[0, 0])
    if (not math.isfinite(value) or value >= SOURCES.ANBH_MAX_VALID
            or (prior.ds.nodata is not None and value == prior.ds.nodata)):
        return np.nan
    return value


class RasterSamplingTests(unittest.TestCase):
    def test_block_sampling_keeps_scalar_pixels_at_edges_and_invalid_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'prior.tif'
            values = np.random.default_rng(721).uniform(-2, 300, (31, 35)).astype('float32')
            values[0, :8] = [np.nan, np.inf, -np.inf, 250, 255, -9999, 0, 249.99]
            transform = Affine(1, 0, -20, 0, -1, 20)
            with rasterio.open(path, 'w', driver='GTiff', height=31, width=35,
                               count=1, dtype='float32', crs='EPSG:4326',
                               transform=transform, nodata=-9999,
                               tiled=True, blockxsize=16, blockysize=16) as destination:
                destination.write(values, 1)
            prior = SOURCES.GlobalPrior(path)
            self.addCleanup(prior.ds.close)
            # The old int() includes positions between -1 and 0; floor() would change them.
            columns = [-1, -0.999, -0.001, 0, *np.arange(35) + 0.5, 15.999, 16, 16.001, 34.999, 35]
            rows = [-1, -0.999, -0.001, 0, *np.arange(31) + 0.5, 15.999, 16, 16.001, 30.999, 31]
            coordinates = [(transform.c + c, transform.f - r) for r in rows for c in columns]
            np.random.default_rng(22).shuffle(coordinates)
            lons, lats = zip(*coordinates)
            expected = [scalar_sample(prior, lon, lat) for lon, lat in coordinates]
            np.testing.assert_array_equal(prior.sample_many(lons, lats), expected)
            self.assertEqual(len(prior.sample_many([], [])), 0)
            for invalid in [np.nan, np.inf, -np.inf]:
                with self.subTest(invalid=invalid):
                    with self.assertRaises((ValueError, OverflowError)):
                        scalar_sample(prior, invalid, 0)
                    with self.assertRaises((ValueError, OverflowError)):
                        prior.sample_many([invalid], [0])

    def test_prior_batch_preserves_regional_precedence_abstention_and_clamping(self):
        rows = [dict(clon=float(i), clat=0.0, tier=tier, height_m=height,
                     geom=shapely.Point(i, 0))
                for i, (tier, height) in enumerate([(0, 17), (1, 9), (1, 6),
                    (2, 8), (2, 8), (2, 8), (2, 8), (2, 8), (2, 8)])]
        regional = SimpleNamespace(
            tr=SimpleNamespace(transform=lambda xs, ys: (xs, ys)),
            covers=lambda x, _y: x < 5,
            zonal_measured_mean=lambda geometry: {1: 500, 3: 0.5}.get(int(geometry.x)))
        ghsl = SimpleNamespace(sample_many=Mock(return_value=np.array([1.5, 200, 0.5, np.nan, 15])))
        stats = dict(tier3=0, tier4=0, abstain=0)
        SOURCES.apply_raster_tiers(rows, regional, ghsl, stats)
        self.assertEqual([(row['tier'], row['height_m']) for row in rows],
                         [(0, 17), (3, 250), (1, 6), (3, 2.5), (4, 3),
                          (4, 100), (2, 8), (2, 8), (4, 15)])
        self.assertEqual(stats, dict(tier3=2, tier4=3, abstain=2))
        ghsl.sample_many.assert_called_once_with([4., 5., 6., 7., 8.], [0.] * 5)

    def test_large_square_batches_preserve_row_order_and_bound_temporary_arrays(self):
        rows = [dict(clon=float(i), clat=0.0, tier=2, height_m=8) for i in range(65539)]
        sizes = []
        def sample(lons, lats):
            sizes.append(len(lons))
            return np.asarray(lons) % 40
        stats = dict(tier3=0, tier4=0, abstain=0)
        SOURCES.apply_raster_tiers(rows, None, SimpleNamespace(sample_many=sample), stats)
        self.assertEqual(sizes, [65536, 3])
        self.assertEqual(stats['tier4'], sum(i % 40 >= 1 for i in range(len(rows))))
        self.assertEqual([(r['tier'], r['height_m']) for r in rows],
                         [(4, max(float(i % 40), 3)) if i % 40 else (2, 8)
                          for i in range(len(rows))])


if __name__ == '__main__':
    unittest.main(verbosity=2)
