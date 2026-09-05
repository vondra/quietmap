"""Native categorical IMD preserves source precedence, geometry, and atomic acquisition."""

import io
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import numpy as np
import rasterio
from rasterio.enums import Resampling
from rasterio.transform import Affine

import dem_sources
import worldcover_sources


class WorldCoverConversionTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        script = Path(__file__).with_name("convert-worldcover.sh").read_text()
        self.function = "convert_one() {" + script.split("convert_one() {", 1)[1].split(
            "export -f convert_one", 1
        )[0]
        self.preparation = script.split("<< 'PYEOF'\n", 1)[1].split("\nPYEOF", 1)[0]
        source, vrt = self.root / "worldcover.tif", self.root / "worldcover.vrt"
        values = np.full((512, 512), 10, dtype=np.uint8)
        values[259, 259] = 30  # Grass at target node row/column 1801.
        values[249, 249] = 80  # Water at target node row/column 1798.
        with rasterio.open(
            source, "w", driver="GTiff", width=512, height=512, count=1,
            dtype="uint8", crs="EPSG:4326",
            transform=Affine(1/12000, 0, 0.5-256/12000, 0, -1/12000, 0.5+256/12000),
        ) as dataset:
            dataset.write(values, 1)
            dataset.build_overviews([2, 4, 8], Resampling.nearest)
        subprocess.run(["gdalbuildvrt", "-q", str(vrt), str(source)], check=True)
        for name in ["forest", "imd"]:
            (self.root / name).mkdir()
        self.failures = self.root / "failures"
        self.failures.touch()
        self.warp_calls = self.root / "warp-calls"
        self.warp_calls.touch()
        (self.root / "worldcover-tiles.txt").write_text("N00E000\n")
        self.dem = self.root / "dem"
        self.dem.mkdir()
        with sqlite3.connect(self.dem / "catalog.sqlite") as database:
            database.execute("CREATE TABLE catalog (resolution INTEGER PRIMARY KEY, inventory TEXT)")
            database.executemany("INSERT INTO catalog VALUES (?, ?)",
                [(r, dem_sources.source_name(r, (0, 0))) for r in (30, 90)])
        self.background(130)

    def background(self, category):
        pixel = self.root / "background-pixel.tif"
        with rasterio.open(pixel, "w", driver="GTiff", width=1, height=1,
                           count=1, dtype="uint8", crs="EPSG:4326",
                           transform=Affine(360, 0, -180, 0, -180, 90)) as source:
            source.write(np.array([[category]], dtype=np.uint8), 1)
        path = worldcover_sources.cci_source_path(self.root)
        path.parent.mkdir(exist_ok=True)
        path.write_text(f'''<VRTDataset rasterXSize="129600" rasterYSize="64800">
<SRS>EPSG:4326</SRS><GeoTransform>-180,0.002777777777777778,0,90,0,-0.002777777777777778</GeoTransform>
<VRTRasterBand dataType="Byte" band="1"><SimpleSource>
<SourceFilename relativeToVRT="0">{pixel}</SourceFilename><SourceBand>1</SourceBand>
<SrcRect xOff="0" yOff="0" xSize="1" ySize="1"/>
<DstRect xOff="0" yOff="0" xSize="129600" ySize="64800"/>
</SimpleSource></VRTRasterBand></VRTDataset>''')

    def convert(self, *, tile="N00E000", force_imd=False, fail_warp=False):
        environment = {
            **os.environ, "VRT": str(self.root / "worldcover.vrt"),
            "VRT_DIR": str(self.root), "FOREST_DST": str(self.root / "forest"),
            "IMD_DST": str(self.root / "imd"), "IMD_FORCE": str(int(force_imd)),
            "FAIL_LIST": str(self.failures), "QM_VENV_PYTHON": sys.executable,
            "WARP_CALLS": str(self.warp_calls), "FAIL_WARP": str(int(fail_warp)),
            "WC_SRC": str(self.root), "PYTHONPATH": str(Path(__file__).parent),
        }
        before = len(self.warp_calls.read_text().splitlines())
        result = subprocess.run(
            ["bash", "-euo", "pipefail", "-c", 'source "$1"\n' + self.function + '''
gdalwarp() {
    printf '%s\\n' "${@: -2:1}" >> "$WARP_CALLS"
    if [ "$FAIL_WARP" = 1 ]; then return 73; fi
    command gdalwarp "$@"
}
convert_one "$2"
''', "_", str(Path(__file__).with_name("node-extent.sh").resolve()), tile],
            env=environment, capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        if not fail_warp:
            self.assertEqual(self.failures.read_text(), "")
        return len(self.warp_calls.read_text().splitlines()) - before

    def test_nearest_classes_ignore_source_overviews(self):
        with rasterio.open(self.root / "worldcover.tif") as dataset:
            self.assertEqual(dataset.overviews(1), [2, 4, 8])
            self.assertEqual(dataset.read(1, out_shape=(256, 256))[129, 129], 10)
            self.assertEqual(dataset.read(1)[259, 259], 30)
        with rasterio.open(self.root / "worldcover.vrt") as dataset:
            self.assertIn(2, dataset.overviews(1), "fixture must expose the misleading overview")
        self.convert()
        # Source bounds contain target nodes 1724..1876, with two non-tree pixels.
        for layer, tree, grass, water in [("forest", 100, 0, 0), ("imd", 2, 5, 100)]:
            expected = np.full((3601, 3601), 5 if layer == "imd" else 0, dtype=np.uint8)
            expected[1724:1877, 1724:1877] = tree
            expected[1801, 1801], expected[1798, 1798] = grass, water
            actual = np.fromfile(self.root / layer / "N00E000.raw", dtype=np.uint8)
            np.testing.assert_array_equal(actual.reshape(3601, 3601), expected)

    def test_one_warp_preserves_independent_resume_and_force(self):
        self.assertEqual(self.convert(), 1, "both layers must share one native-class warp")
        outputs = [self.root / layer / "N00E000.raw" for layer in ["forest", "imd"]]
        original = [path.read_bytes() for path in outputs]
        self.assertEqual(self.convert(), 0, "complete outputs require no warp")
        for missing in outputs:
            with self.subTest(missing=missing.parent.name):
                missing.unlink()
                self.assertEqual(self.convert(), 1)
                self.assertEqual([path.read_bytes() for path in outputs], original)
        np.full((3601, 3601), 37, dtype=np.uint8).tofile(outputs[0])
        np.full((3601, 3601), 23, dtype=np.uint8).tofile(outputs[1])
        preserved_forest = outputs[0].read_bytes()
        self.assertEqual(self.convert(force_imd=True), 1)
        self.assertEqual(outputs[0].read_bytes(), preserved_forest)
        self.assertEqual(outputs[1].read_bytes(), original[1])
        existing = [path.read_bytes() for path in outputs]
        self.assertEqual(self.convert(force_imd=True, fail_warp=True), 1)
        self.assertEqual(len(self.failures.read_text().splitlines()), 1)
        self.assertEqual([path.read_bytes() for path in outputs], existing)
        self.assertEqual(list(self.root.glob("wc_*.tif")), [])

    def test_dateline_uses_native_local_sources_and_identical_shared_nodes(self):
        sources = self.root / "sources"
        sources.mkdir()
        rows, cols = np.indices((512, 512))
        values = np.where((rows//16 + cols//13) % 2 == 0, 10, 80).astype(np.uint8)
        self.assertTrue(np.all(values[:, 0] != values[:, -1]))
        paths = []
        for latitude in [63, 66]:
            for longitude, suffix in [(177, "E177"), (-180, "W180")]:
                source = sources / f"ESA_WorldCover_10m_2021_v200_N{latitude}{suffix}_Map.tif"
                with rasterio.open(
                    source, "w", driver="GTiff", width=512, height=512, count=1,
                    dtype="uint8", crs="EPSG:4326",
                    transform=Affine(3/512, 0, longitude, 0, -3/512, latitude+3),
                ) as dataset:
                    dataset.write(values, 1)
                paths.append(source)
        subprocess.run(["gdalbuildvrt", "-q", str(self.root / "worldcover.vrt"),
                        *map(str, paths)], check=True)
        with sqlite3.connect(sources / "catalog.sqlite") as database:
            database.execute("CREATE TABLE worldcover_sources (key TEXT PRIMARY KEY, bytes INTEGER)")
            database.executemany("INSERT INTO worldcover_sources VALUES (?, ?)",
                [(worldcover_sources.PREFIX + path.name, path.stat().st_size) for path in paths])
        subprocess.run([sys.executable, "-", str(sources), str(self.root / "tiles.txt"), str(self.dem)],
                       input=self.preparation, text=True, check=True,
                       env={**os.environ, "PYTHONPATH": str(Path(__file__).parent)})
        for name in ["N65E179", "N65W180"]:
            self.assertEqual(self.convert(tile=name), 1)
        for path in self.warp_calls.read_text().splitlines():
            with rasterio.open(path) as source:
                self.assertEqual(source.width, 1024, "warp must not span the world's longitudes")
                self.assertEqual(source.transform.a, 3/512, "source pixels must remain native")
        target_rows = np.arange(3601)
        native_classes = values[target_rows*512//10800, 0]
        strict = target_rows % 675 != 0  # Exclude exact native-pixel boundary ties.
        for layer in ["forest", "imd"]:
            east, west = [np.fromfile(self.root / layer / f"{name}.raw", dtype=np.uint8)
                          .reshape(3601, 3601) for name in ["N65E179", "N65W180"]]
            np.testing.assert_array_equal(east[:, -1], west[:, 0])
            expected = np.where(native_classes == 10, 100, 0) if layer == "forest" else np.where(
                native_classes == 10, 2, 100)
            np.testing.assert_array_equal(west[strict, 0], expected[strict])
            self.assertIn(100, west[:, 0], "a zero-only edge is not a positive sample oracle")
            for tile, offset in [(east, 7200), (west, 0)]:
                columns = np.arange(1, 3600)
                native_column_numerators = (offset + columns)*512
                non_ties = native_column_numerators % 10800 != 0
                source_classes = values[np.ix_(target_rows[strict]*512//10800,
                                                native_column_numerators[non_ties]//10800)]
                expected = np.where(source_classes == 10, 100, 0) if layer == "forest" else np.where(
                    source_classes == 10, 2, 100)
                np.testing.assert_array_equal(tile[np.ix_(strict, columns[non_ties])], expected)

    def test_valid_worldcover_wins_and_unclassified_nodes_fail(self):
        wc = np.array([[70, 80, 10, 50, 0, 255, 17]], dtype=np.uint8)
        cci = np.array([[210, 220, 190, 50, 220, 210, 130]], dtype=np.uint8)
        np.testing.assert_array_equal(worldcover_sources.categorical_imd(wc, cci),
                                      [[0, 100, 2, 85, 0, 100, 5]])
        for invalid in (0, 1, 255):
            with self.subTest(invalid=invalid):
                with self.assertRaisesRegex(ValueError, "unclassified"):
                    worldcover_sources.categorical_imd(np.array([[0]], dtype=np.uint8),
                                                       np.array([[invalid]], dtype=np.uint8))
        # A background hole is irrelevant when the finer source has a real class.
        np.testing.assert_array_equal(worldcover_sources.categorical_imd(
            np.array([[70]], dtype=np.uint8), np.array([[0]], dtype=np.uint8)), [[0]])

    def test_background_only_imd_does_not_add_forest_tiles(self):
        self.convert(tile="N01E000")
        self.assertFalse((self.root / "forest/N01E000.raw").exists())
        actual = np.fromfile(self.root / "imd/N01E000.raw", dtype=np.uint8)
        self.assertEqual(actual.size, 3601*3601)
        self.assertTrue(np.all(actual == 5))

    def test_categorical_crosswalk_retains_crop_dominance_and_surface_types(self):
        categories = np.array([[30, 40, 100, 110, 150, 170, 190, 210, 220]], dtype=np.uint8)
        np.testing.assert_array_equal(worldcover_sources.categorical_imd(
            np.zeros_like(categories), categories), [[10, 5, 5, 5, 15, 2, 85, 100, 0]])

    def test_native_background_sampling_wraps_dateline_and_clamps_polar_edge(self):
        path = self.root / "native-background.tif"
        with rasterio.open(path, "w", driver="GTiff", width=129600, height=64800,
                           count=1, dtype="uint8", crs="EPSG:4326", tiled=True,
                           compress="deflate", SPARSE_OK="YES",
                           transform=Affine(1/360, 0, -180, 0, -1/360, 90)) as source:
            source.write(np.array([[130, 150, 220], [210, 50, 190]], dtype=np.uint8), 1,
                         window=((0, 2), (0, 3)))
            source.write(np.array([[120]], dtype=np.uint8), 1,
                         window=((0, 1), (129599, 129600)))
            source.write(np.array([[210, 220]], dtype=np.uint8), 1,
                         window=((64799, 64800), (0, 2)))
        with rasterio.open(path) as source:
            west = worldcover_sources.cci_tile_classes(source, (89, -180))
            east = worldcover_sources.cci_tile_classes(source, (89, 179))
            south = worldcover_sources.cci_tile_classes(source, (-90, -180))
        np.testing.assert_array_equal(west[:20, :30], np.repeat(np.repeat(
            [[130, 150, 220], [210, 50, 190]], 10, axis=0), 10, axis=1))
        np.testing.assert_array_equal(east[:, -1], west[:, 0])
        self.assertEqual(east[0, -2], 120)
        np.testing.assert_array_equal(south[-11:, :20],
                                      np.tile(np.repeat([210, 220], 10), (11, 1)))

    def test_background_identity_rejects_unreviewed_bytes(self):
        with self.assertRaisesRegex(ValueError, "reviewed source identity"):
            worldcover_sources.validate_cci_source(self.root)

    def test_failed_background_acquisition_never_publishes_a_source(self):
        target = self.root / "new-source"

        class Response(io.BytesIO):
            def __init__(self, declared_bytes):
                super().__init__(b"x")
                self.headers = {"Content-Length": str(declared_bytes)}

        for declared_bytes, error in [(2, "Incomplete"), (1, "reviewed source identity")]:
            with self.subTest(declared_bytes=declared_bytes), \
                    mock.patch.object(worldcover_sources.urllib.request, "urlopen",
                                      return_value=Response(declared_bytes)):
                with self.assertRaisesRegex(ValueError, error):
                    worldcover_sources.download_cci_source(target)
                self.assertFalse(worldcover_sources.cci_source_path(target).exists())
                self.assertEqual(list((target / "imd-background").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
