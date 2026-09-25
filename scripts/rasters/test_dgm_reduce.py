"""Derived-grid averaging, lattice decoding and manifest identity contracts."""
import json
from pathlib import Path
import tempfile
import unittest

import numpy as np
from osgeo import gdal, osr
from dgm_reduce import (block_average, decode_xyz_lattice, reduce_geotiff, reduce_xyz,
                        manifest_entry, write_manifest, load_manifest, normalize_grid)


def write_source(path, values, west=400000.0, north=5710000.0, step=1.0, epsg=25832, nodata=-9999.0):
    ds = gdal.GetDriverByName('GTiff').Create(str(path), values.shape[1], values.shape[0], 1,
                                              gdal.GDT_Float32)
    crs = osr.SpatialReference()
    crs.ImportFromEPSG(epsg)
    ds.SetProjection(crs.ExportToWkt())
    ds.SetGeoTransform([west, step, 0, north, 0, -step])
    ds.GetRasterBand(1).SetNoDataValue(nodata)
    ds.GetRasterBand(1).WriteArray(values.astype(np.float32))
    ds = None


class ReduceTest(unittest.TestCase):
    def test_block_average_is_an_exact_area_mean_with_nodata(self):
        values = np.arange(100, dtype=float).reshape(10, 10)
        mean = block_average(values, 5)
        self.assertEqual(mean.tolist(), [[22.0, 27.0], [72.0, 77.0]])
        gapped = values.copy()
        gapped[:5, :5] = np.nan
        partial = block_average(gapped, 5)
        self.assertTrue(np.isnan(partial[0, 0]))
        self.assertAlmostEqual(float(partial[0, 1]), 27.0)
        with self.assertRaises(ValueError):
            block_average(np.zeros((7, 10)), 5)

    def test_reduce_geotiff_keeps_edges_and_crs(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / 'tile.tif'
            values = np.arange(100, dtype=float).reshape(10, 10)
            write_source(source, values)
            target = Path(temp) / 'tile-5m.tif'
            stats = reduce_geotiff(source, target, -9999.0, factor=5)
            self.assertEqual((stats['rows'], stats['columns']), (2, 2))
            ds = gdal.Open(str(target))
            self.assertEqual(ds.GetGeoTransform(), (400000.0, 5.0, 0.0, 5710000.0, 0.0, -5.0))
            self.assertIn('25832', ds.GetProjection())
            self.assertEqual(ds.GetRasterBand(1).GetNoDataValue(), -9999.0)
            self.assertEqual(ds.GetRasterBand(1).ReadAsArray().tolist(),
                             [[22.0, 27.0], [72.0, 77.0]])
            ds = None

    def test_reduce_xyz_decodes_the_lattice_then_averages(self):
        with tempfile.TemporaryDirectory() as temp:
            lattice = Path(temp) / 'tile.xyz'
            rows = ['x y z'] + [f'{400000 + c + 0.5:.1f} {5710000 - r - 0.5:.1f} {r * 10 + c:.1f}'
                                for r in range(10) for c in range(10)]
            lattice.write_text('\n'.join(rows) + '\n')
            target = Path(temp) / 'tile-5m.tif'
            stats = reduce_xyz(lattice, target, 1, 25832, factor=5)
            self.assertAlmostEqual(stats['valid_fraction'], 1.0)
            ds = gdal.Open(str(target))
            self.assertEqual(ds.GetGeoTransform(), (400000.0, 5.0, 0.0, 5710000.0, 0.0, -5.0))
            self.assertEqual(ds.GetRasterBand(1).ReadAsArray().tolist(),
                             [[22.0, 27.0], [72.0, 77.0]])
            ds = None
            with self.assertRaises(ValueError):
                decode_xyz_lattice(lattice, 5)

    def test_normalize_remaps_voids_and_assigns_a_missing_crs(self):
        with tempfile.TemporaryDirectory() as temp:
            raw = Path(temp) / 'raw.tif'
            ds = gdal.GetDriverByName('GTiff').Create(str(raw), 4, 2, 1, gdal.GDT_UInt16)
            ds.SetGeoTransform([400000.0, 5.0, 0.0, 5710000.0, 0.0, -5.0])
            ds.GetRasterBand(1).SetNoDataValue(0)
            ds.GetRasterBand(1).WriteArray(np.array([[285, 0, 300, 310]] * 2, dtype=np.uint16))
            ds = None
            target = Path(temp) / 'norm.tif'
            self.assertTrue(normalize_grid(raw, target, assign_epsg=25832))
            self.assertFalse(normalize_grid(raw, target, assign_epsg=25832))
            ds = gdal.Open(str(target))
            self.assertEqual(ds.GetRasterBand(1).DataType, gdal.GDT_Float32)
            self.assertEqual(ds.GetRasterBand(1).GetNoDataValue(), -9999.0)
            self.assertEqual(ds.GetRasterBand(1).ReadAsArray().tolist(),
                             [[285.0, -9999.0, 300.0, 310.0]] * 2)
            crs = osr.SpatialReference(wkt=ds.GetProjection())
            ds = None
            self.assertEqual(crs.GetAuthorityCode('PROJCS'), '25832')

    def test_normalize_refuses_a_contradicting_crs_and_untagged_voids(self):
        with tempfile.TemporaryDirectory() as temp:
            raw = Path(temp) / 'raw.tif'
            write_source(raw, np.ones((4, 4)), epsg=25833)
            with self.assertRaises(ValueError):
                normalize_grid(raw, Path(temp) / 'out.tif', assign_epsg=25832)
            untagged = Path(temp) / 'untagged.tif'
            ds = gdal.GetDriverByName('GTiff').Create(str(untagged), 4, 4, 1, gdal.GDT_Float32)
            crs = osr.SpatialReference()
            crs.ImportFromEPSG(25832)
            ds.SetProjection(crs.ExportToWkt())
            ds.SetGeoTransform([400000.0, 5.0, 0.0, 5710000.0, 0.0, -5.0])
            ds.GetRasterBand(1).WriteArray(np.ones((4, 4)))
            ds = None
            with self.assertRaises(ValueError):
                normalize_grid(untagged, Path(temp) / 'out2.tif', assign_epsg=25832)

    def test_manifest_links_each_raw_source_to_its_derivative(self):
        with tempfile.TemporaryDirectory() as temp:
            raw, derived = Path(temp) / 'raw.tif', Path(temp) / 'raw-5m.tif'
            raw.write_bytes(b'raw-bytes')
            derived.write_bytes(b'derived-bytes')
            entry = manifest_entry('https://example.invalid/raw.tif', raw, derived, '5 m area mean')
            self.assertEqual(entry['raw_bytes'], 9)
            manifest = Path(temp) / 'stream-manifest.json'
            write_manifest(manifest, [entry])
            loaded = load_manifest(manifest)
            self.assertEqual(loaded['count'], 1)
            self.assertEqual(loaded['entries'][0]['derived'], 'raw-5m.tif')
            tampered = json.loads(manifest.read_text())
            tampered['count'] = 2
            broken = Path(temp) / 'broken.json'
            broken.write_text(json.dumps(tampered))
            with self.assertRaises(ValueError):
                load_manifest(broken)


if __name__ == '__main__':
    unittest.main()
