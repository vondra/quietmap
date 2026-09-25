"""Aligned download windows and GEDTM land pre-filtering for terrain fetchers."""
import email
import http.client
import math
import time
import urllib.error
import urllib.request

from osgeo import gdal, osr

gdal.UseExceptions()
osr.UseExceptions()

RETRYABLE = (http.client.IncompleteRead, http.client.RemoteDisconnected,
             TimeoutError, ConnectionError, urllib.error.URLError)


def polite_sleep(started):
    """Hold each worker thread to one request per second; skips never sleep."""
    time.sleep(max(0, 1 - (time.monotonic() - started)))


def download_bytes(url, timeout=300, attempts=3):
    """GET a URL with backoff on transient failures; returns (body, headers).

    Géoplateforme answers valid WMS windows with a transient HTTP 400 under
    sustained load (observed 2026-09-25; the identical URL succeeds on retry),
    so 400 is retried like any other transient failure.
    """
    last = None
    for attempt in range(attempts):
        try:
            request = urllib.request.Request(url, headers={'User-Agent': 'QuietMap terrain producer'})
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read(), response.headers
        except urllib.error.HTTPError as error:
            if (error.code < 500 and error.code not in (400, 429)) or attempt + 1 == attempts:
                raise
            last = error
        except RETRYABLE as error:
            if attempt + 1 == attempts:
                raise
            last = error
        time.sleep(2 ** attempt)
    raise last


def split_wcs_multipart(body, content_type):
    """Extract the TIFF part of a WCS 2.0 multipart coverage response."""
    message = email.message_from_bytes(b'Content-Type: ' + content_type.encode() + b'\n\n' + body)
    for part in message.walk():
        if part.get_content_type() == 'image/tiff':
            return part.get_payload(decode=True)
    raise ValueError('WCS response holds no TIFF part')


def nodata_tag(path):
    """Read a single-band nodata tag while holding the dataset alive."""
    dataset = gdal.Open(str(path))
    if dataset is None or dataset.RasterCount != 1:
        raise ValueError(f'expected one band: {path}')
    return dataset.GetRasterBand(1).GetNoDataValue()


Z9 = 512


def square_of_longitude(longitude):
    return math.floor((longitude + 180) / 360 * Z9)


def square_of_latitude(latitude):
    mercator = math.log(math.tan(math.radians(latitude)) + 1 / math.cos(math.radians(latitude)))
    return math.floor((1 - mercator / math.pi) / 2 * Z9)


def latitude_of_square_edge(y):
    return math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / Z9))))


def square_bounds(x, y):
    return (x / Z9 * 360 - 180, latitude_of_square_edge(y + 1),
            (x + 1) / Z9 * 360 - 180, latitude_of_square_edge(y))


def grid_windows(xmin, ymin, xmax, ymax, step):
    """Cover [xmin, xmax) x [ymin, ymax) with step-sized windows on the step grid."""
    if step <= 0 or xmin >= xmax or ymin >= ymax:
        raise ValueError('invalid window grid')
    windows = []
    for ix in range(math.floor(xmin / step), math.ceil(xmax / step)):
        for iy in range(math.floor(ymin / step), math.ceil(ymax / step)):
            windows.append((ix * step, iy * step, (ix + 1) * step, (iy + 1) * step))
    return windows


def reproject_bounds(x0, y0, x1, y1, source_epsg, target_epsg=4326):
    """Transform a projected bounding box to lon/lat, sampling its edges for curvature."""
    source = osr.SpatialReference()
    source.ImportFromEPSG(source_epsg)
    source.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    target = osr.SpatialReference()
    target.ImportFromEPSG(target_epsg)
    target.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    transform = osr.CoordinateTransformation(source, target)
    points = []
    for i in range(9):
        t = i / 8
        points.extend([(x0 + (x1 - x0) * t, y0), (x0 + (x1 - x0) * t, y1),
                       (x0, y0 + (y1 - y0) * t), (x1, y0 + (y1 - y0) * t)])
    converted = transform.TransformPoints(points)
    longitudes = [p[0] for p in converted]
    latitudes = [p[1] for p in converted]
    return min(longitudes), min(latitudes), max(longitudes), max(latitudes)


class LandMask:
    """Answers whether a lon/lat window holds any GEDTM land node.

    The mask is a coarse byte raster (1 = land) written next to each sea-filled
    fallback crop; sub-cell islets may be skipped, which only yields fallback
    (sea) heights there, never an error.
    """

    def __init__(self, mask_path):
        self.dataset = gdal.Open(str(mask_path))
        if self.dataset is None or self.dataset.RasterCount != 1:
            raise ValueError(f'land mask needs one band: {mask_path}')
        self.transform = self.dataset.GetGeoTransform()

    def has_land(self, west, south, east, north):
        origin_x, pixel, _, origin_y, _, signed_pixel = self.transform
        pixel_y = -signed_pixel
        x0 = max(0, int((west - origin_x) / pixel))
        y0 = max(0, int((origin_y - north) / pixel_y))
        x1 = min(self.dataset.RasterXSize, int(math.ceil((east - origin_x) / pixel)))
        y1 = min(self.dataset.RasterYSize, int(math.ceil((origin_y - south) / pixel_y)))
        if x1 <= x0 or y1 <= y0:
            return False
        sample = self.dataset.ReadAsArray(x0, y0, x1 - x0, y1 - y0)
        if sample is None:
            raise ValueError('land mask window read failed')
        return bool((sample != 0).any())
