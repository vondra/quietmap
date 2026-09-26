"""Read one COG overview over HTTP and retain it as a reduced national raster."""
import math
import os
from pathlib import Path

import numpy as np
from osgeo import gdal

gdal.UseExceptions()


def reduce_cog_to_grid(url, resolution, target, username=None, password=None):
    """Retain the finest exact COG overview, area-averaged onto `resolution`.

    Reads only overview bytes, snaps bounds out to absolute `resolution`
    multiples so every block lands on one shared grid, and returns
    (columns, rows, overview_factor, nodata). Exact grids reproduce bit for bit.
    """
    previous = os.environ.get('GDAL_HTTP_USERPWD')
    try:
        if username is not None:
            os.environ['GDAL_HTTP_USERPWD'] = username + ':' + password
            os.environ['GDAL_HTTP_AUTH'] = 'BASIC'
        os.environ['GDAL_HTTP_MAX_RETRY'] = '3'
        os.environ['GDAL_HTTP_RETRY_DELAY'] = '2'
        dataset = gdal.Open('/vsicurl/' + url)
        if dataset is None or dataset.RasterCount != 1:
            raise ValueError(f'expected one COG band: {url}')
        band = dataset.GetRasterBand(1)
        transform = dataset.GetGeoTransform()
        if transform[2] or transform[4] or transform[1] <= 0 or transform[1] != -transform[5]:
            raise ValueError(f'expected a north-up square grid: {url}')
        level, factor = None, None
        for candidate in (8, 4, 2):
            index = candidate.bit_length() - 2
            if index >= band.GetOverviewCount():
                continue
            part = band.GetOverview(index)
            if (band.XSize / part.XSize == candidate and band.YSize / part.YSize == candidate):
                level, factor = part, candidate
                break
        if level is None:
            raise ValueError(f'COG has no exact overview: {url}')
        driver = gdal.GetDriverByName('MEM')
        mem = driver.Create('', level.XSize, level.YSize, 1, gdal.GDT_Float32)
        mem.SetProjection(dataset.GetProjection())
        mem.SetGeoTransform((transform[0], transform[1] * factor, 0,
                             transform[3], 0, transform[5] * factor))
        values = level.ReadAsArray()
        if values is None or values.shape != (level.YSize, level.XSize):
            raise ValueError(f'incomplete overview read: {url}')
        nodata = band.GetNoDataValue()
        mem.GetRasterBand(1).SetNoDataValue(nodata)
        mem.GetRasterBand(1).WriteArray(values.astype(np.float32))
        pixel = transform[1] * factor
        west, north = transform[0], transform[3]
        east = west + level.XSize * pixel
        south = north + level.YSize * transform[5] * factor
        bounds = (math.floor(west / resolution) * resolution,
                  math.floor(south / resolution) * resolution,
                  math.ceil(east / resolution) * resolution,
                  math.ceil(north / resolution) * resolution)
        target = Path(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        warped = gdal.Warp(str(target), mem, format='GTiff', xRes=resolution, yRes=resolution,
                           outputBounds=bounds, resampleAlg='average',
                           srcNodata=nodata, dstNodata=nodata, outputType=gdal.GDT_Float32,
                           creationOptions=['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2'])
        if warped is None:
            raise ValueError(f'overview reduction failed: {url}')
        result = (warped.RasterXSize, warped.RasterYSize, factor, nodata)
        warped = None
        return result
    finally:
        if previous is None:
            os.environ.pop('GDAL_HTTP_USERPWD', None)
            os.environ.pop('GDAL_HTTP_AUTH', None)
        else:
            os.environ['GDAL_HTTP_USERPWD'] = previous
