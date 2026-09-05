"""Sample aligned Copernicus nodes with integer bilinear weights and one final quantization."""

import math
from pathlib import Path
import sys

import numpy as np
from osgeo import gdal

gdal.UseExceptions()
DENSITY = 3600  # The existing 1-arcsecond HGT node contract.
NODATA = -32768
BLOCK_ROWS = 128  # Bound temporary interpolation arrays independently of world coverage.


def integer_pixel(value: float) -> int:
    rounded = round(value)
    # Decimal geotransforms accumulate sub-micro-pixel error at integer source offsets.
    if abs(value - rounded) > 1e-6:
        raise ValueError(f"Native DEM pixels are not aligned: {value}")
    return rounded


def sample_nodes(dataset, longitude: int, top_latitude: int, start_row: int,
                 rows: int, columns: int, density: int = DENSITY):
    geo = dataset.GetGeoTransform()
    source_width, source_height = integer_pixel(1 / geo[1]), integer_pixel(-1 / geo[5])
    origin_x = integer_pixel(geo[0] * source_width + 0.5)
    origin_y = integer_pixel(geo[3] * source_height - 0.5)
    gx, gy = math.gcd(source_width, density), math.gcd(source_height, density)
    dx, dy = density // gx, density // gy
    x, rx = np.divmod((longitude * source_width - origin_x) * dx
                     + np.arange(columns) * (source_width // gx), dx)
    y, ry = np.divmod((origin_y - top_latitude * source_height) * dy
                     + np.arange(start_row, start_row + rows) * (source_height // gy), dy)
    xx, yy = x + (rx != 0), y + (ry != 0)
    x0, y0, x1, y1 = int(x[0]), int(y[0]), int(xx[-1] + 1), int(yy[-1] + 1)
    window = np.full((y1 - y0, x1 - x0), NODATA, dtype=np.float64)
    left, top = max(x0, 0), max(y0, 0)
    right, bottom = min(x1, dataset.RasterXSize), min(y1, dataset.RasterYSize)
    if left < right and top < bottom:
        window[top-y0:bottom-y0, left-x0:right-x0] = dataset.ReadAsArray(
            left, top, right-left, bottom-top, buf_type=gdal.GDT_Float64)
    numerator = np.zeros((rows, columns), dtype=np.float64)
    missing = np.zeros((rows, columns), dtype=bool)
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    for yi, wy in [(y, dy-ry), (yy, ry)]:
        for xi, wx in [(x, dx-rx), (xx, rx)]:
            weight = wy[:, None] * wx[None, :]
            values = window[(yi-y0)[:, None], (xi-x0)[None, :]]
            invalid = ~np.isfinite(values) | (values == NODATA)
            if nodata is not None:
                invalid |= values == nodata
            missing |= invalid & (weight != 0)
            numerator += np.where(invalid, 0, values) * weight
    result = numerator / (dx * dy)
    result[missing] = NODATA
    return result


def quantize(values):
    rounded = np.copysign(np.floor(np.abs(values) + 0.5), values)
    if not np.all(np.isfinite(rounded) & (rounded >= NODATA) & (rounded <= 32767)):
        raise ValueError("DEM elevation cannot be represented by the HGT Int16 contract")
    return rounded.astype(">i2")


def write_aligned_gap(native: Path, output: Path, latitude: int, longitude: int,
                      width: int, height: int) -> None:
    if width != height:
        raise ValueError("GLO90 coverage gaps require the native square GLO30 latitude grid")
    source = gdal.Open(str(native))
    target = gdal.GetDriverByName("GTiff").Create(str(output), width, height, 1, gdal.GDT_Float64)
    target.SetProjection(source.GetProjection())
    target.SetGeoTransform([longitude-0.5/width, 1/width, 0, latitude+1+0.5/height, 0, -1/height])
    target.GetRasterBand(1).SetNoDataValue(NODATA)
    for row in range(0, height, BLOCK_ROWS):
        values = sample_nodes(source, longitude, latitude+1, row, min(BLOCK_ROWS, height-row), width, width)
        target.GetRasterBand(1).WriteArray(values, 0, row)
    target.FlushCache()


def write_hgt(mosaics: Path, longitude: int, latitude: int, output: Path) -> None:
    source = gdal.Open(str(mosaics / f"{latitude}.vrt"))
    south = mosaics / f"{latitude-1}.vrt"
    with output.open("wb") as stream:
        for row in range(0, DENSITY, BLOCK_ROWS):
            values = sample_nodes(source, longitude, latitude+1, row,
                                  min(BLOCK_ROWS, DENSITY-row), DENSITY+1)
            stream.write(quantize(values).tobytes())
        if south.is_file():
            # A latitude-band transition owns its shared row in the southern native grid.
            values = sample_nodes(gdal.Open(str(south)), longitude, latitude, 0, 1, DENSITY+1)
        else:
            values = sample_nodes(source, longitude, latitude+1, DENSITY, 1, DENSITY+1)
        stream.write(quantize(values).tobytes())


if __name__ == "__main__":
    write_hgt(Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), Path(sys.argv[4]))
