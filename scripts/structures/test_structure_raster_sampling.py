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

    def test_survey_answers_first_and_ghsl_is_sampled_only_where_needed(self):
        rows = [dict(clon=float(i), clat=0.0, geom=None if i == 2 else shapely.Point(i, 0),
                     needs_ghsl=i != 4) for i in range(6)]
        regional = SimpleNamespace(
            tr=SimpleNamespace(transform=lambda xs, ys: (xs, ys)),
            covers=lambda x, _y: x < 3,
            zonal_measured_mean=lambda geometry: {1: 20.5}.get(int(geometry.x)))
        ghsl = SimpleNamespace(sample_many=Mock(return_value=np.array([7.0, 8.0, 9.0, 10.0])))
        stats = dict(regional=0, abstain=0)
        SOURCES.sample_raster_heights(rows, regional, ghsl, stats)
        self.assertEqual([row['regional_m'] for row in rows], [None, 20.5, None, None, None, None])
        np.testing.assert_array_equal([row['ghsl_m'] for row in rows],
                                      [7.0, np.nan, 8.0, 9.0, np.nan, 10.0])
        self.assertEqual(stats, dict(regional=1, abstain=1))
        ghsl.sample_many.assert_called_once_with([0., 2., 3., 5.], [0.] * 4)

    def test_large_square_batches_preserve_row_order_and_bound_temporary_arrays(self):
        rows = [dict(clon=float(i), clat=0.0, geom=None, needs_ghsl=True) for i in range(65539)]
        sizes = []
        def sample(lons, lats):
            sizes.append(len(lons))
            return np.asarray(lons) % 40
        SOURCES.sample_raster_heights(rows, None, SimpleNamespace(sample_many=sample),
                                      dict(regional=0, abstain=0))
        self.assertEqual(sizes, [65536, 3])
        self.assertEqual([r['ghsl_m'] for r in rows], [float(i % 40) for i in range(len(rows))])


if __name__ == '__main__':
    unittest.main(verbosity=2)
