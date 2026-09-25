"""Mean canopy height over canopy-covered source pixels, retaining gaps and verified zero canopy."""
import numpy as np
from osgeo import gdal


def classified_heights(source, dataset, crop, source_window):
    """Only a same-grid, same-epoch vegetation classification can authorize DSM minus DTM."""
    required = ('vegetation_mask_path', 'vegetation_values', 'epoch', 'dtm_epoch',
                'mask_epoch', 'vertical_crs', 'dtm_vertical_crs')
    if any(key not in source for key in required) or not source['vegetation_values']:
        raise ValueError('DSM minus DTM requires a same-epoch vegetation mask and explicit datums')
    if source['epoch'] != source['dtm_epoch'] or source['epoch'] != source['mask_epoch']:
        raise ValueError('DSM, DTM and vegetation mask epochs differ')
    if source['vertical_crs'] != source['dtm_vertical_crs']:
        raise ValueError('DSM and DTM vertical datums differ')
    layers = []
    for key in ('dtm_path', 'vegetation_mask_path'):
        auxiliary = gdal.Open(str(source[key]))
        if (auxiliary.GetGeoTransform() != dataset.GetGeoTransform()
                or auxiliary.GetProjection() != dataset.GetProjection()
                or (auxiliary.RasterXSize, auxiliary.RasterYSize) != (dataset.RasterXSize, dataset.RasterYSize)):
            raise ValueError('DSM, DTM and mask must share their native pixel grid')
        clipped = gdal.Translate('', auxiliary, format='MEM', outputType=gdal.GDT_Float64,
                                 srcWin=source_window)
        a = clipped.ReadAsArray().astype(float)
        nodata = clipped.GetRasterBand(1).GetNoDataValue()
        if nodata is not None:
            a[a == nodata] = np.nan
        layers.append(a)
    ground, mask = layers
    surface = crop.ReadAsArray().astype(float)
    nodata = crop.GetRasterBand(1).GetNoDataValue()
    if nodata is not None:
        surface[surface == nodata] = np.nan
    heights = np.where(np.isin(mask, source['vegetation_values']), np.maximum(surface - ground, 0), 0)
    heights[~np.isfinite(mask)] = np.nan
    return heights


def canopy_average(source, window, average):
    """Bound memory by processing target rows; source nodata is never reclassified as bare ground."""
    result = np.full((window['rows'], window['columns']), np.nan)
    dataset = source['path'] if isinstance(source['path'], gdal.Dataset) else gdal.Open(str(source['path']))
    density = window['nodes_per_degree']
    for start in range(0, len(result), 16):
        count = min(16, len(result) - start)
        stripe = dict(window, north_node=window['north_node'] - start, rows=count)
        north, west = stripe['north_node'], stripe['west_node']
        projwin = [(west-.5)/density, (north+.5)/density,
                   (west+stripe['columns']-.5)/density, (north-count+.5)/density]
        native_window = gdal.Translate('', dataset, format='VRT', projWinSRS='EPSG:4326', projWin=projwin)
        original, clipped = dataset.GetGeoTransform(), native_window.GetGeoTransform()
        row_offset = round((clipped[3] - original[3]) / original[5]) - 2
        column_offset = round((clipped[0] - original[0]) / original[1]) - 2
        # Translate may round both origin and size inward; retain two native pixels on each side.
        source_window = [column_offset, row_offset, native_window.RasterXSize + 4, native_window.RasterYSize + 4]
        crop = gdal.Translate('', dataset, format='MEM', outputType=gdal.GDT_Float64, srcWin=source_window)
        heights = (classified_heights(source, dataset, crop, source_window) if 'dtm_path' in source
                   else crop.ReadAsArray().astype(float))
        nodata = crop.GetRasterBand(1).GetNoDataValue()
        missing = ~np.isfinite(heights) | (heights > 250) | (heights < 0)
        # Translate pads outside a nodata-free source with zero. Those pixels are unobserved.
        rr = np.arange(heights.shape[0]) + row_offset
        cc = np.arange(heights.shape[1]) + column_offset
        missing |= ((rr < 0) | (rr >= dataset.RasterYSize))[:, None]
        missing |= ((cc < 0) | (cc >= dataset.RasterXSize))[None, :]
        if nodata is not None and 'dtm_path' not in source:
            missing |= heights == nodata
        heights[missing] = np.nan
        crop.GetRasterBand(1).SetNoDataValue(float('nan'))
        crop.GetRasterBand(1).WriteArray(heights)
        coverage_mean = average(dict(source, path=crop, nodata=float('nan')), stripe)
        heights[heights == 0] = np.nan
        crop.GetRasterBand(1).WriteArray(heights)
        positive_mean = average(dict(source, path=crop, nodata=float('nan')), stripe)
        # An all-zero valid cell is no canopy; any available canopy contributes only to its covered part.
        positive_mean[(coverage_mean == 0) & ~np.isfinite(positive_mean)] = 0
        result[start:start+count] = positive_mean
    return result
