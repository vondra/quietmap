"""Hermetic tests for Hansen source identity and missing-loss semantics."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from rasterio.errors import RasterioIOError

sys.path.insert(0, str(Path(__file__).parent))
import hansen_sources  # noqa: E402


TREE_NAME = "Hansen_GFC-2024-v1.12_treecover2000_10S_060E.tif"
LOSS_NAME = "Hansen_GFC-2024-v1.12_lossyear_10S_060E.tif"
VALID_TREE_NAME = "Hansen_GFC-2024-v1.12_treecover2000_10S_050W.tif"
VALID_LOSS_NAME = "Hansen_GFC-2024-v1.12_lossyear_10S_050W.tif"


def no_such_key_marker(filename: str) -> bytes:
    return (
        "<?xml version='1.0' encoding='UTF-8'?>"
        "<Error><Code>NoSuchKey</Code>"
        f"<Details>No such object: bucket/release/{filename}</Details></Error>"
    ).encode()


class HansenSourceTests(unittest.TestCase):
    def test_failed_conversion_preserves_output_and_cleans_its_staging(self) -> None:
        script = Path(__file__).with_name("convert-forest-continuous.sh").read_text()
        function = "convert_one()" + script.split("convert_one()", 1)[1].split(
            "export -f convert_one", 1
        )[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "N50E014.raw"
            output.write_bytes(b"old")
            unrelated = root / "unrelated"
            unrelated.write_bytes(b"keep")
            environment = {
                **os.environ,
                "TMPDIR": directory,
                "FOREST_DST": directory,
                "EXPECTED_BYTES": "9",
                "GRID": "3",
                "TCD_VRT": str(root / "absent.vrt"),
                "TC_VRT": "unused.vrt",
                "LY_VRT": "unused.vrt",
            }
            command = (
                "node_extent() { echo '0 0 1 1'; }; "
                "gdalwarp() { return 42; };\n" + function + "\nconvert_one N50E014"
            )
            failed = subprocess.run(
                ["bash", "-euo", "pipefail", "-c", command],
                env=environment, capture_output=True, text=True, check=False,
            )
            self.assertEqual(failed.returncode, 42, failed.stderr)
            self.assertEqual(output.read_bytes(), b"old")
            self.assertEqual(set(root.iterdir()), {output, unrelated})
            output.write_bytes(b"completed")
            resumed = subprocess.run(
                ["bash", "-euo", "pipefail", "-c", command],
                env=environment, capture_output=True, text=True, check=False,
            )
            self.assertEqual(resumed.returncode, 0, resumed.stderr)
            self.assertEqual(output.read_bytes(), b"completed")
            self.assertEqual(unrelated.read_bytes(), b"keep")

    def test_only_matching_no_such_key_is_expected_missing_lossyear(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / LOSS_NAME
            marker.write_bytes(no_such_key_marker(LOSS_NAME))
            self.assertTrue(hansen_sources.is_expected_missing_lossyear(marker))

            marker.write_bytes(no_such_key_marker("other.tif"))
            self.assertFalse(hansen_sources.is_expected_missing_lossyear(marker))
            marker.write_text("<Error><Code>NoSuchKey</Code>", encoding="utf-8")
            self.assertFalse(hansen_sources.is_expected_missing_lossyear(marker))

    def test_explicit_missing_loss_is_omitted_from_vrt_sources(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            treecover = root / "treecover2000" / TREE_NAME
            lossyear = root / "lossyear" / LOSS_NAME
            valid_treecover = root / "treecover2000" / VALID_TREE_NAME
            valid_lossyear = root / "lossyear" / VALID_LOSS_NAME
            treecover.parent.mkdir()
            lossyear.parent.mkdir()
            treecover.write_bytes(b"raster")
            lossyear.write_bytes(no_such_key_marker(LOSS_NAME))
            valid_treecover.write_bytes(b"raster")
            valid_lossyear.write_bytes(b"raster")

            def validate(path: Path, _layer: str) -> None:
                if path == lossyear:
                    raise RasterioIOError("not a raster")

            with mock.patch.object(hansen_sources, "_validate_hansen_raster", side_effect=validate):
                tree_sources, loss_sources, missing = hansen_sources.validated_hansen_sources(root)
            self.assertEqual(set(tree_sources), {treecover, valid_treecover})
            self.assertEqual(loss_sources, [valid_lossyear])
            self.assertEqual(missing, 1)

    def test_corrupt_loss_source_is_not_silently_zeroed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            treecover = root / "treecover2000" / TREE_NAME
            lossyear = root / "lossyear" / LOSS_NAME
            treecover.parent.mkdir()
            lossyear.parent.mkdir()
            treecover.write_bytes(b"raster")
            lossyear.write_bytes(b"truncated")

            def validate(path: Path, _layer: str) -> None:
                if path == lossyear:
                    raise RasterioIOError("not a raster")

            with mock.patch.object(hansen_sources, "_validate_hansen_raster", side_effect=validate):
                with self.assertRaisesRegex(ValueError, "invalid Hansen lossyear"):
                    hansen_sources.validated_hansen_sources(root)

    def test_schema_and_filename_extent_must_match(self) -> None:
        dataset = mock.MagicMock()
        dataset.__enter__.return_value = dataset
        dataset.driver = "GTiff"
        dataset.width = dataset.height = 40_000
        dataset.count = 1
        dataset.dtypes = ("uint8",)
        dataset.crs.to_epsg.return_value = 4326
        dataset.bounds = (60.0, -20.0, 70.0, -10.0)
        with mock.patch.object(hansen_sources.rasterio, "open", return_value=dataset):
            hansen_sources._validate_hansen_raster(Path(LOSS_NAME), "lossyear")
            dataset.bounds = (60.0, -19.0, 70.0, -10.0)
            with self.assertRaisesRegex(ValueError, "schema or extent"):
                hansen_sources._validate_hansen_raster(Path(LOSS_NAME), "lossyear")


if __name__ == "__main__":
    unittest.main()
