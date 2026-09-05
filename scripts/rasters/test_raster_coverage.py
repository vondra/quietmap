"""Official inventory coverage must not turn missing or partly observed land into ocean."""

import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

import dem_sources
import worldcover_sources

spec = importlib.util.spec_from_file_location("repack_native_z9", Path(__file__).with_name("repack-native-z9.py"))
assert spec and spec.loader
repack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(repack)


class CoverageTest(unittest.TestCase):
    def test_source_catalog_drives_land_unknown_and_ocean_without_directory_inference(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dem = root / "dem"
            dem.mkdir()
            land = {(0, 0), (40, 44), (82, -40), (83, -40)}
            with sqlite3.connect(dem / "catalog.sqlite") as database:
                database.execute("CREATE TABLE catalog (resolution INTEGER PRIMARY KEY, inventory TEXT NOT NULL)")
                database.executemany("INSERT INTO catalog VALUES (?, ?)",
                    [(resolution, "\n".join(dem_sources.source_name(resolution, tile) for tile in sorted(land))) for resolution in (30, 90)])
            wc = root / "wc"
            keys = {worldcover_sources.PREFIX + f"ESA_WorldCover_10m_2021_v200_{tile}_Map.tif": 4
                    for tile in ("N00E000", "N81W042")}

            class S3:
                def get_paginator(self, name):
                    assert name == "list_objects_v2"
                    return self

                def paginate(self, **kwargs):
                    assert kwargs == {"Bucket": worldcover_sources.BUCKET, "Prefix": worldcover_sources.PREFIX}
                    return [{"Contents": [{"Key": key, "Size": size} for key, size in keys.items()]}]

            self.assertEqual(worldcover_sources.fetch_catalog(wc, S3()), keys)
            self.assertEqual(worldcover_sources.read_catalog(wc), keys)
            with self.assertRaises(ValueError):
                worldcover_sources.validate_source_files(wc, keys)
            for key in keys:
                (wc / key.rsplit("/", 1)[-1]).write_bytes(b"TIFF")
            worldcover_sources.validate_source_files(wc, keys)
            coverage = repack.source_coverage("imd", dem, wc)
            self.assertEqual(set(coverage["unknown"]), {(40, 44), (82, -40), (83, -40)})
            self.assertEqual(len(coverage["tiles"]), 16)
            self.assertNotIn((40, 44), coverage["tiles"])
            for channel in ("dem", "forest"):
                coverage = repack.source_coverage(channel, dem, None)
                self.assertEqual(set(coverage["tiles"]), land)
                self.assertEqual(coverage["unknown"], [])
            for invalid in [[], [(next(iter(keys)), 0)], list(keys.items()) * 2,
                            [(worldcover_sources.PREFIX + "ESA_WorldCover_10m_2021_v200_N00W181_Map.tif", 4)]]:
                with self.assertRaises(ValueError):
                    worldcover_sources.validate_inventory(invalid)


if __name__ == "__main__":
    unittest.main()
