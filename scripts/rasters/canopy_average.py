"""Mean canopy height over canopy-covered source pixels, retaining gaps and verified zero canopy."""
import numpy as np
from osgeo import gdal


def canopy_average(source, window, average):
    """Bound memory by processing target rows; source nodata is never reclassified as bare ground."""
    result = np.full((window['rows'], window['columns']), np.nan)
    dataset = gdal.Open(str(source['path']))
    if 'dtm_path' in source:
        raise ValueError('DSM minus DTM requires a same-epoch vegetation mask; unclassified surfaces are not canopy')
    density = window['nodes_per_degree']
    for start in range(0, len(result), 16):
        count = min(16, len(result) - start)
        stripe = dict(window, north_node=window['north_node'] - start, rows=count)
        north, west = stripe['north_node'], stripe['west_node']
        crop = gdal.Translate('', dataset, format='MEM', outputType=gdal.GDT_Float64, projWinSRS='EPSG:4326',
                              projWin=[(west-.5)/density, (north+.5)/density,
                                       (west+stripe['columns']-.5)/density, (north-count+.5)/density])
        heights = crop.ReadAsArray().astype(float)
        nodata = crop.GetRasterBand(1).GetNoDataValue()
        missing = ~np.isfinite(heights) | (heights > 250) | (heights < 0)
        if nodata is not None:
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
