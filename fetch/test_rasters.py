"""Publisher normalization regressions: explicit grids, cyclic longitude and explicit missing-loss records."""
from pathlib import Path
import tempfile
import unittest

from osgeo import gdal
import numpy as np
from rasters import convert, validated_missing_loss


class RasterConversionTests(unittest.TestCase):
    def test_land_catalogue_marks_only_named_degrees_and_refuses_malformed_tiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            catalog = root / "catalog.txt"
            catalog.write_text("Copernicus_DSM_COG_30_S90_00_W180_00_DEM\n"
                               "Copernicus_DSM_COG_30_N89_00_E179_00_DEM\n")
            recipe = [dict(input="coverage", coverage_catalog=str(catalog))]
            convert(recipe, root / "normalized")
            dataset = gdal.Open(str(root / "normalized/coverage"))
            values = dataset.ReadAsArray()
            self.assertEqual(int(values.sum()), 2)
            self.assertEqual((values[179, 0], values[0, 359]), (1, 1))
            self.assertEqual(dataset.GetGeoTransform(), (-180, 1, 0, 90, 0, -1))
            catalog.write_text("Copernicus_DSM_COG_30_N90_00_E179_00_DEM\n")
            with self.assertRaisesRegex(ValueError, "outside"):
                convert(recipe, root / "invalid")
            self.assertEqual(list((root / "invalid").iterdir()), [])

    def test_longitude_relocation_keeps_native_pixels_after_temporary_files_are_removed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name, west, step, values in [("west", -180, 1, [10, 80]),
                                             ("east", 178, 1.0000000001, [50, 30])]:
                dataset = gdal.GetDriverByName("GTiff").Create(str(root / name), 2, 1, 1, gdal.GDT_Byte)
                dataset.SetProjection("EPSG:4326")
                dataset.SetGeoTransform((west, step, 0, 1, 0, -1))
                dataset.GetRasterBand(1).SetNoDataValue(255)
                dataset.GetRasterBand(1).WriteArray(np.array([values], dtype=np.uint8))
                dataset = None
            recipe = [dict(input="cover", files=[str(root / "west"), str(root / "east")],
                           longitude_offsets={str(root / "west"): 360},
                           pixel_size=[1, 1], bounds=[178, 0, 182, 1], nodata=255)]
            destination = root / "normalized"
            convert(recipe, destination)
            dataset = gdal.Open(str(destination / "cover"))
            self.assertEqual(dataset.GetGeoTransform(), (178, 1, 0, 1, 0, -1))
            self.assertEqual(dataset.ReadAsArray().tolist(), [[50, 30, 10, 80]])
            self.assertEqual([p.name for p in destination.iterdir()], ["cover"])
            before = (destination / "cover").read_bytes()
            with self.assertRaisesRegex(ValueError, "refusing to replace"):
                convert(recipe, destination)
            self.assertEqual((destination / "cover").read_bytes(), before)

    def test_missing_loss_is_explicit_and_other_download_errors_are_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "publisher_lossyear_tile.tif"
            path.write_text(f"<Error><Code>NoSuchKey</Code><Key>{path.name}</Key></Error>")
            self.assertTrue(validated_missing_loss(path))
            path.write_text("<Error><Code>NoSuchKey</Code><Key>another.tif</Key></Error>")
            self.assertFalse(validated_missing_loss(path))
            path.write_text(f"<Error><Code>AccessDenied</Code><Key>{path.name}</Key></Error>")
            self.assertFalse(validated_missing_loss(path))


if __name__ == "__main__":
    unittest.main()
