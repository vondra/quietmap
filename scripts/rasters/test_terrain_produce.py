"""Terrain source identity, node footprint and missing-data publication contracts."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

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

    def test_nodes_past_the_datum_grid_fall_back_without_a_shift(self):
        import terrain_produce
        real = terrain_produce.datum_transform
        class GridEdge:
            def TransformPoints(self, points):
                if any(point[0] < 0 for point in points):
                    raise RuntimeError('Coordinate to transform falls outside grid')
                return [(x, y, z + 2.0) for x, y, z in points]
        terrain_produce.datum_transform = lambda *args, **kwargs: GridEdge()
        try:
            window=dict(north_node=1,west_node=-1,rows=1,columns=3,nodes_per_degree=1)
            values = convert_datum(np.full((1, 3), 10.0), window, 5778)
            self.assertTrue(np.isnan(values[0, 0]))
            self.assertEqual(values[0, 1], 12.0)
            self.assertEqual(values[0, 2], 12.0)
        finally:
            terrain_produce.datum_transform = real

    def test_silent_inf_grid_cells_fall_back_without_a_shift(self):
        import terrain_produce
        real = terrain_produce.datum_transform
        class HoleyGrid:
            def TransformPoints(self, points):
                return [(x, y, z + 2.0) if x >= 0 else (float('inf'),) * 3
                        for x, y, z in points]
        terrain_produce.datum_transform = lambda *args, **kwargs: HoleyGrid()
        try:
            window=dict(north_node=1,west_node=-1,rows=1,columns=3,nodes_per_degree=1)
            values = convert_datum(np.full((1, 3), 10.0), window, 5778)
            self.assertTrue(np.isnan(values[0, 0]))
            self.assertEqual(values[0, 1], 12.0)
            self.assertEqual(values[0, 2], 12.0)
        finally:
            terrain_produce.datum_transform = real

    def test_conversion_without_any_node_is_a_setup_error(self):
        import terrain_produce
        real = terrain_produce.datum_transform
        class EmptyGrid:
            def TransformPoints(self, points):
                return [(float('inf'),) * 3 for _ in points]
        terrain_produce.datum_transform = lambda *args, **kwargs: EmptyGrid()
        try:
            window=dict(north_node=1,west_node=0,rows=1,columns=2,nodes_per_degree=1)
            with self.assertRaisesRegex(ValueError, 'not a zero shift'):
                convert_datum(np.full((1, 2), 10.0), window, 5778)
        finally:
            terrain_produce.datum_transform = real

    def test_unexpected_datum_error_still_raises(self):
        import terrain_produce
        real = terrain_produce.datum_transform
        class Broken:
            def TransformPoints(self, points):
                raise RuntimeError('cannot find datum grid')
        terrain_produce.datum_transform = lambda *args, **kwargs: Broken()
        try:
            window=dict(north_node=1,west_node=0,rows=1,columns=1,nodes_per_degree=1)
            with self.assertRaisesRegex(RuntimeError, 'cannot find datum grid'):
                convert_datum(np.full((1, 1), 10.0), window, 5778)
        finally:
            terrain_produce.datum_transform = real

    def test_suppressed_grid_edge_errors_fall_back_after_fresh_probe(self):
        # OSR collapses repeated failures on one transform object into a bare
        # suppressed message; the fresh probe must recover the grid-edge cause.
        import terrain_produce
        real = terrain_produce.datum_transform
        class Suppressing:
            def __init__(self):
                self.errors = 0
            def TransformPoints(self, points):
                if any(point[0] < 0 for point in points):
                    self.errors += 1
                    if self.errors > 1:
                        raise RuntimeError('Reprojection failed, err = 2052, further '
                                           'errors will be suppressed on the transform object.')
                    raise RuntimeError('Coordinate to transform falls outside grid')
                return [(x, y, z + 2.0) for x, y, z in points]
        class GridEdge:
            def TransformPoints(self, points):
                if any(point[0] < 0 for point in points):
                    raise RuntimeError('Coordinate to transform falls outside grid')
                return [(x, y, z + 2.0) for x, y, z in points]
        made = []
        def factory(*args, **kwargs):
            made.append(True)
            return Suppressing() if len(made) == 1 else GridEdge()
        terrain_produce.datum_transform = factory
        try:
            window=dict(north_node=2,west_node=-1,rows=2,columns=3,nodes_per_degree=1)
            values = convert_datum(np.full((2, 3), 10.0), window, 5778)
            self.assertTrue(np.isnan(values[:, 0]).all())
            self.assertTrue((values[:, 1:] == 12.0).all())
        finally:
            terrain_produce.datum_transform = real

    def test_fresh_probe_keeps_unexpected_datum_errors_loud(self):
        import terrain_produce
        real = terrain_produce.datum_transform
        class Broken:
            def TransformPoints(self, points):
                raise RuntimeError('cannot find datum grid')
        terrain_produce.datum_transform = lambda *args, **kwargs: Broken()
        try:
            window=dict(north_node=1,west_node=0,rows=1,columns=1,nodes_per_degree=1)
            with self.assertRaisesRegex(RuntimeError, 'cannot find datum grid'):
                convert_datum(np.full((1, 1), 10.0), window, 5778)
        finally:
            terrain_produce.datum_transform = real

    def assemble_mem(self, national_value):
        from terrain_produce import assemble_with_statistics
        window = dict(north_node=100, west_node=0, rows=100, columns=100,
                      nodes_per_degree=1)
        crs = osr.SpatialReference()
        crs.ImportFromEPSG(4326)
        fallback = gdal.GetDriverByName('MEM').Create('', 136, 136, 1, gdal.GDT_Float32)
        fallback.SetProjection(crs.ExportToWkt())
        fallback.SetGeoTransform((-18, 1, 0, 118, 0, -1))
        fallback.GetRasterBand(1).Fill(0)
        cells = np.full((136, 136), national_value, dtype=np.float32)
        cells[:, :40] = -9999
        national = gdal.GetDriverByName('MEM').Create('', 136, 136, 1, gdal.GDT_Float32)
        national.SetProjection(crs.ExportToWkt())
        national.SetGeoTransform((-18, 1, 0, 118, 0, -1))
        national.GetRasterBand(1).SetNoDataValue(-9999)
        national.GetRasterBand(1).WriteArray(cells)
        sources = [dict(path=fallback, horizontal_crs='EPSG:4326', vertical_crs=3855,
                        role='fallback', group='fallback'),
                   dict(path=national, horizontal_crs='EPSG:4326', vertical_crs=3855,
                        role='national', group='national', nodata=-9999)]
        return assemble_with_statistics(sources, window, 'dem', 'average', 16)

    def test_conformance_keeps_true_terrain_past_fallback_voids(self):
        from terrain_seams import require_seam_gate
        values, owner, stats = self.assemble_mem(600.0)
        self.assertEqual(stats['groups'][0]['conformed_nodes'], 96 * 136)
        self.assertAlmostEqual(float(values[50, 90]), 600.0)
        self.assertAlmostEqual(float(values[50, 10]), 0.0)
        require_seam_gate(stats)

    def test_agreeing_terrain_is_never_conformed(self):
        values, owner, stats = self.assemble_mem(1.0)
        self.assertEqual(stats['groups'][0]['conformed_nodes'], 0)
        self.assertAlmostEqual(float(values[50, 90]), 1.0, places=5)
        self.assertAlmostEqual(float(values[50, 10]), 0.0)

    def test_conformance_threshold_is_strict(self):
        _, _, stats = self.assemble_mem(2.0)
        self.assertEqual(stats['groups'][0]['conformed_nodes'], 0)

    def test_disjoint_group_seams_stack_per_edge_not_per_group(self):
        import terrain_produce
        from terrain_produce import assemble_with_statistics
        window = dict(north_node=0, west_node=0, rows=1, columns=60, nodes_per_degree=1)
        fallback = dict(path='fallback', role='fallback', group='sea', vertical_crs=3855, epoch='2020')
        west = dict(path='west', role='national', group='west', vertical_crs=3855, epoch='2020')
        east = dict(path='east', role='national', group='east', vertical_crs=3855, epoch='2020')
        def fake_average(source, work, kernel='average'):
            shape = (work['rows'], work['columns'])
            if source['role'] == 'fallback':
                return np.full(shape, 10.)
            out = np.full(shape, np.nan)
            if source['group'] == 'west':
                out[:, :15] = 10.24
            else:
                out[:, -15:] = 10.24
            return out
        with patch.object(terrain_produce, 'read_average', side_effect=fake_average):
            _, _, stats = assemble_with_statistics([fallback, west, east], window, 'dem', halo=2)
        self.assertAlmostEqual(stats['groups'][0]['maximum_selection_step_m'], .12)
        self.assertAlmostEqual(stats['groups'][1]['maximum_selection_step_m'], .12)
        # A per-group sum would reach 0.44; disjoint edges stack to 0.12.
        self.assertAlmostEqual(stats['maximum_artificial_step_bound_m'], .32)

    def test_french_frames_fall_through_past_clipped_onshore_extents(self):
        import terrain_produce
        from unittest.mock import MagicMock, patch
        seen = {}

        def fake_options():
            options = MagicMock()
            options.SetOnlyBest.side_effect = lambda flag: seen.setdefault('flags', []).append(flag)
            return options

        with patch.object(terrain_produce.osr, 'SpatialReference', return_value=MagicMock()), \
                patch.object(terrain_produce.osr, 'CoordinateTransformationOptions',
                             side_effect=fake_options), \
                patch.object(terrain_produce.osr, 'CreateCoordinateTransformation',
                             return_value=MagicMock()):
            terrain_produce.datum_transform(5720, [-5.2, 41.3, 10.0, 51.2])
            terrain_produce.datum_transform(5721, [8.1, 41.3, 9.9, 43.1])
            terrain_produce.datum_transform(5778, None)
        self.assertEqual(seen['flags'], [False, False, True])

    def test_colocated_ramps_exempt_in_relief_but_trip_on_the_flat(self):
        import terrain_produce
        from terrain_produce import assemble_with_statistics
        from terrain_seams import require_seam_gate
        window = dict(north_node=0, west_node=0, rows=1, columns=60, nodes_per_degree=1)
        fallback = dict(path='fallback', role='fallback', group='sea', vertical_crs=3855, epoch='2020')
        groups = [dict(path=name, role='national', group=name, vertical_crs=3855, epoch='2020')
                  for name in ('west', 'east', 'north')]

        def assemble(slope):
            def fake_average(source, work, kernel='average'):
                shape = (work['rows'], work['columns'])
                base = np.broadcast_to(np.arange(shape[1], dtype=float) * slope, shape).copy()
                if source['role'] == 'fallback':
                    return base
                out = np.full(shape, np.nan)
                out[:, :15] = base[:, :15] + .24
                return out
            with patch.object(terrain_produce, 'read_average', side_effect=fake_average):
                return assemble_with_statistics([fallback, *groups], window, 'dem', halo=2)

        # Three colocated 0.12 ramps stack past the 0.3 selection budget; the
        # stack hides in steep relief and trips on the flat.
        _, _, steep = assemble(50.)
        for group in steep['groups']:
            self.assertGreater(group['maximum_selection_step_m'], 0)
        self.assertGreater(steep['selection_exempt_stacked_edges'], 0)
        require_seam_gate(steep)
        _, _, flat = assemble(0.)
        self.assertEqual(flat['selection_exempt_stacked_edges'], 0)
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(flat)

    def test_waivers_cover_reviewed_sites_not_squares(self):
        import json
        from terrain_produce import load_waivers
        good = [dict(x=266, y=173, lon=7.7344, lat=50.1078, radius_m=1500,
                     reason='Rhine gorge at Boppard: independent DEM agrees',
                     evidence=dict(national_m=88.5, fallback_m=171.3, glo30_m=161.0))]
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'waivers.json'
            path.write_text(json.dumps(good))
            self.assertEqual(load_waivers(path, {(266, 173)}), good)
            for bad in (dict(good[0], x=1), dict(good[0], radius_m=40),
                        dict(good[0], radius_m=6000), dict(good[0], reason=''),
                        dict(good[0], evidence=[])):
                path.write_text(json.dumps([bad]))
                with self.assertRaises(ValueError):
                    load_waivers(path, {(266, 173)})

    def test_waiver_disc_clears_only_its_proven_real_seam(self):
        import terrain_produce
        from terrain_produce import assemble_with_statistics
        from terrain_seams import expanded, require_seam_gate
        window = dict(north_node=0, west_node=0, rows=1, columns=60, nodes_per_degree=1)
        fallback = dict(path='fallback', role='fallback', group='sea', vertical_crs=3855, epoch='2020')
        groups = [dict(path=name, role='national', group=name, vertical_crs=3855, epoch='2020')
                  for name in ('west', 'east', 'north')]

        def fake_average(source, work, kernel='average'):
            shape = (work['rows'], work['columns'])
            if source['role'] == 'fallback':
                return np.full(shape, 10.)
            out = np.full(shape, np.nan)
            out[:, :15] = 10.24
            return out

        def assemble(exclusions=()):
            with patch.object(terrain_produce, 'read_average', side_effect=fake_average):
                return assemble_with_statistics([fallback, *groups], window, 'dem', halo=2,
                                                exclusions=exclusions)

        # Three colocated 0.24 ramps stack past the budget on the flat.
        _, _, bare = assemble()
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(bare)
        # A waiver disc over the seam clears it; one far away does not.
        work = expanded(window, 4)
        west, south, east, north = bounds(work)
        node_lon = lambda c: west + (c + .5) / work['columns'] * (east - west)
        node_lat = lambda r: north - (r + .5) / work['rows'] * (north - south)
        spacing_m = (east - west) / work['columns'] * 111320
        seam = [(node_lon(15), node_lat(work['rows'] // 2), 4 * spacing_m)]
        _, _, cleared = assemble(seam)
        require_seam_gate(cleared)
        far = [(node_lon(60), node_lat(work['rows'] // 2), 4 * spacing_m)]
        _, _, elsewhere = assemble(far)
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(elsewhere)


if __name__ == '__main__': unittest.main()
