"""Terrain source identity, node footprint and missing-data publication contracts."""
from pathlib import Path
import tempfile
import unittest

import numpy as np
from osgeo import gdal, osr
from terrain_produce import bounds, encode, read_average, convert_datum, grouped_sources
from terrain_io import publish_bytes
from canopy_average import canopy_average


class TerrainTest(unittest.TestCase):
    def test_average_uses_the_cell_around_each_node_not_a_half_cell_shift(self):
        window = dict(north_node=1, west_node=0, rows=1, columns=1, nodes_per_degree=1)
        self.assertEqual(bounds(window), [-.5, .5, .5, 1.5])
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'ground.tif'
            ds = gdal.GetDriverByName('GTiff').Create(str(path), 4, 4, 1, gdal.GDT_Float32)
            crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
            ds.SetProjection(crs.ExportToWkt())
            ds.SetGeoTransform([-.5, .25, 0, 1.5, 0, -.25])
            z = np.zeros((4,4)); z[0,0] = 160
            ds.GetRasterBand(1).WriteArray(z)
            ds.GetRasterBand(1).SetScale(.1)
            ds = None
            source = dict(path=str(path), horizontal_crs='EPSG:4326')
            self.assertAlmostEqual(float(read_average(source, window)[0,0]), 10)
            self.assertNotAlmostEqual(float(read_average(source, window, 'bilinear')[0,0]), 10)

    def test_encoding_extremes_and_missing_are_distinct_from_ocean(self):
        window = dict(dem_offset_m=-500, dem_codes_per_metre=5, dem_missing=65535)
        z = np.array([[-500., -.2, 0, .2, 12606.8, np.nan]])
        self.assertEqual(encode(z, 'dem', window).tolist(), [[0,2499,2500,2501,65534,65535]])
        self.assertEqual(encode(np.array([[0.,250.,np.nan]]), 'canopy', window).tolist(), [[0,250,255]])
        with self.assertRaises(ValueError): encode(np.array([[-501.]]), 'dem', window)
        with self.assertRaises(ValueError): encode(np.array([[251.]]), 'canopy', window)

    def test_canopy_mean_excludes_bare_ground_and_keeps_missing(self):
        window = dict(north_node=1, west_node=0, rows=1, columns=3, nodes_per_degree=1)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'canopy.tif'
            ds = gdal.GetDriverByName('GTiff').Create(str(path), 6, 2, 1, gdal.GDT_Byte)
            crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
            ds.SetProjection(crs.ExportToWkt())
            ds.SetGeoTransform([-.5,.5,0,1.5,0,-.5])
            ds.GetRasterBand(1).SetNoDataValue(255)
            ds.GetRasterBand(1).WriteArray(np.array([[0,20,0,0,255,255]] * 2))
            ds = None
            out = canopy_average(dict(path=str(path), horizontal_crs='EPSG:4326'), window, read_average)
            self.assertEqual(out[0,:2].tolist(), [20,0])
            self.assertTrue(np.isnan(out[0,2]))

    def test_resume_accepts_only_identical_published_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            path=Path(temp)/'dem.u16le'
            publish_bytes(path,b'\xc4\x09')
            publish_bytes(path,b'\xc4\x09')
            with self.assertRaises(ValueError): publish_bytes(path,b'\0\0')

    def test_classified_surface_subtraction_excludes_buildings_and_missing_masks(self):
        window = dict(north_node=1, west_node=0, rows=1, columns=4, nodes_per_degree=1)
        with tempfile.TemporaryDirectory() as temp:
            paths = []
            for name, values in [('surface', [120, 140, 150, 100]), ('ground', [100]*4), ('mask', [5, 6, 255, 5])]:
                path = Path(temp) / (name + '.tif'); paths.append(str(path))
                ds = gdal.GetDriverByName('GTiff').Create(str(path), 4, 1, 1, gdal.GDT_Float32)
                crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
                ds.SetProjection(crs.ExportToWkt()); ds.SetGeoTransform([-.5, 1, 0, 1.5, 0, -1])
                ds.GetRasterBand(1).SetNoDataValue(255 if name == 'mask' else 0); ds.GetRasterBand(1).WriteArray(np.array([values]))
                ds = None
            source = dict(path=paths[0], dtm_path=paths[1], vegetation_mask_path=paths[2],
                          horizontal_crs='EPSG:4326', vertical_crs=3855, dtm_vertical_crs=3855,
                          epoch='2020', dtm_epoch='2020', mask_epoch='2020', vegetation_values=[5])
            actual = canopy_average(source, window, read_average)
            np.testing.assert_equal(actual, [[20, 0, np.nan, 0]])
            with self.assertRaisesRegex(ValueError, 'epochs differ'):
                canopy_average(dict(source, dtm_epoch='2019'), window, read_average)

    def test_nodata_free_canopy_does_not_report_zero_outside_its_footprint(self):
        window = dict(north_node=1, west_node=-1, rows=1, columns=3, nodes_per_degree=1)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'zero.tif'
            ds = gdal.GetDriverByName('GTiff').Create(str(path), 1, 1, 1, gdal.GDT_Byte)
            crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
            ds.SetProjection(crs.ExportToWkt()); ds.SetGeoTransform([-.5, 1, 0, 1.5, 0, -1])
            ds.GetRasterBand(1).WriteArray(np.array([[0]])); ds = None
            actual = canopy_average(dict(path=str(path), horizontal_crs='EPSG:4326'), window, read_average)
            np.testing.assert_equal(actual, [[np.nan, 0, np.nan]])

    def test_mosaics_reject_native_resampling_or_silently_skipped_members(self):
        with tempfile.TemporaryDirectory() as temp:
            sources = []
            for name, step, origin, dtype in [('fine', .25, 0, gdal.GDT_Float32),
                                              ('coarse', .5, 1, gdal.GDT_Float32),
                                              ('shifted', .25, 1.1, gdal.GDT_Float32),
                                              ('double', .25, 1, gdal.GDT_Float64)]:
                path = Path(temp) / (name + '.tif')
                ds = gdal.GetDriverByName('GTiff').Create(str(path), 4, 4, 1, dtype)
                crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
                ds.SetProjection(crs.ExportToWkt()); ds.SetGeoTransform([origin, step, 0, 1, 0, -step])
                ds.GetRasterBand(1).Fill(20); ds = None
                sources.append(dict(path=str(path), horizontal_crs='EPSG:4326', group='one-region'))
            for second in sources[1:]:
                with self.subTest(second=second['path']), self.assertRaises(ValueError):
                    grouped_sources([sources[0], second])

    def test_canopy_native_crop_includes_pixels_needed_at_the_last_output_column(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'zero.tif'
            ds = gdal.GetDriverByName('GTiff').Create(str(path), 2880, 64, 1, gdal.GDT_Byte)
            crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
            ds.SetProjection(crs.ExportToWkt()); ds.SetGeoTransform([11.95, .00025, 0, 49.815, 0, -.00025])
            ds.GetRasterBand(1).Fill(0); ds = None
            window = dict(north_node=179305, west_node=43031, rows=16, columns=2533, nodes_per_degree=3600)
            values = canopy_average(dict(path=str(path), horizontal_crs='EPSG:4326'), window, read_average)
            self.assertTrue(np.all(values == 0))

    def test_manifest_nodata_does_not_hide_overlapping_valid_source_pixels(self):
        with tempfile.TemporaryDirectory() as temp:
            sources = []
            for name, value in [('valid', 20), ('empty', 0)]:
                path = Path(temp) / (name + '.tif')
                ds = gdal.GetDriverByName('GTiff').Create(str(path), 2, 2, 1, gdal.GDT_Float32)
                crs = osr.SpatialReference(); crs.ImportFromEPSG(4326)
                ds.SetProjection(crs.ExportToWkt()); ds.SetGeoTransform([-.5, .5, 0, 1.5, 0, -.5])
                ds.GetRasterBand(1).Fill(value); ds = None
                sources.append(dict(path=str(path), horizontal_crs='EPSG:4326', group='one', nodata=0))
            source = grouped_sources(sources)[0]
            window = dict(north_node=1, west_node=0, rows=1, columns=1, nodes_per_degree=1)
            np.testing.assert_equal(read_average(source, window), [[20]])

    def test_missing_geoid_cannot_be_accepted_as_zero_shift(self):
        # An invalid vertical CRS must fail even for a flat ground surface.
        window=dict(north_node=0,west_node=0,rows=1,columns=1,nodes_per_degree=3600)
        with self.assertRaises(RuntimeError):
            convert_datum(np.zeros((1,1)),window,999999)


if __name__ == '__main__': unittest.main()
