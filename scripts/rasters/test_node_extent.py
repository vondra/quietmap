"""Shared raster nodes must select the same water class from either tile."""

from pathlib import Path
import subprocess
import tempfile
import unittest

import numpy as np
import rasterio
from rasterio.transform import Affine


class NodeExtentTests(unittest.TestCase):
    def test_adjacent_worldcover_tiles_share_identical_water_nodes(self):
        extent_script = Path(__file__).with_name("node-extent.sh").resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "water-boundary.tif"
            # WorldCover's 1/12000-degree pixels and classes 10/80 reproduce
            # the real land/water seam seen between N50E014 and N50E015.
            values = np.full((20, 20), 10, dtype=np.uint8)
            values[:, 10:] = 80
            values[10:, :] = 80
            with rasterio.open(
                source, "w", driver="GTiff", width=20, height=20, count=1,
                dtype="uint8", crs="EPSG:4326",
                transform=Affine(1/12000, 0, 15-10/12000, 0, -1/12000, 51+10/12000),
            ) as dataset:
                dataset.write(values, 1)
            tiles = {}
            for lon, lat in [(14, 50), (15, 50), (14, 51)]:
                extent = subprocess.run(
                    ["bash", "-c", 'source "$1"; node_extent "$2" "$3" 3601',
                     "_", str(extent_script), str(lon), str(lat)],
                    check=True, capture_output=True, text=True,
                ).stdout.split()
                destination = root / f"{lat}-{lon}.tif"
                subprocess.run(
                    ["gdalwarp", "-q", "-te", *extent, "-ts", "3601", "3601",
                     "-r", "near", "-ot", "Byte", str(source), str(destination)],
                    check=True,
                )
                with rasterio.open(destination) as dataset:
                    tiles[lon, lat] = dataset.read(1)
            west, east, north = tiles[14, 50], tiles[15, 50], tiles[14, 51]
            self.assertIn(80, west[:, -1])
            self.assertIn(80, west[0, :])
            np.testing.assert_array_equal(west[:, -1], east[:, 0])
            np.testing.assert_array_equal(west[0, :], north[-1, :])


if __name__ == "__main__":
    unittest.main()
