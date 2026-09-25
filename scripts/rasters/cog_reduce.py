"""Read one COG overview over HTTP and retain it as a reduced national raster."""
import os
from pathlib import Path

import numpy as np
from osgeo import gdal

gdal.UseExceptions()


def reduce_cog_overview(url, overview_index, target, username=None, password=None):
    """Retain overview `overview_index` as a Float32 raster with its exact grid.

    Returns (columns, rows, factor, nodata). Reads only the overview bytes.
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
        if band.XSize % 1 or overview_index >= band.GetOverviewCount():
            raise ValueError(f'COG lacks overview {overview_index}: {url}')
        overview = band.GetOverview(overview_index)
        factor_x = band.XSize / overview.XSize
        factor_y = band.YSize / overview.YSize
        if factor_x != int(factor_x) or factor_x != factor_y:
            raise ValueError(f'overview is not an exact integer reduction: {url}')
        factor = int(factor_x)
        transform = dataset.GetGeoTransform()
        values = overview.ReadAsArray()
        if values is None or values.shape != (overview.YSize, overview.XSize):
            raise ValueError(f'incomplete overview read: {url}')
        nodata = band.GetNoDataValue()
        target = Path(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        driver = gdal.GetDriverByName('GTiff')
        out = driver.Create(str(target), overview.XSize, overview.YSize, 1, gdal.GDT_Float32,
                            options=['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2'])
        out.SetProjection(dataset.GetProjection())
        out.SetGeoTransform((transform[0], transform[1] * factor, 0,
                             transform[3], 0, transform[5] * factor))
        out.GetRasterBand(1).SetNoDataValue(nodata)
        out.GetRasterBand(1).WriteArray(values.astype(np.float32))
        out = None
        return overview.XSize, overview.YSize, factor, nodata
    finally:
        if previous is None:
            os.environ.pop('GDAL_HTTP_USERPWD', None)
            os.environ.pop('GDAL_HTTP_AUTH', None)
        else:
            os.environ['GDAL_HTTP_USERPWD'] = previous
