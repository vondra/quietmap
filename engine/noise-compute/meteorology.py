"""Nord2000 occurrence statistics and ISO 9613-1 absorption for the ERA5 producer."""
import math
import numpy as np
from numba import njit, prange

# Eurasto (2006), VTT-R-02530-06 Tables 1–7; Nord2000 Road guide A.4.2.
W_EDGES = np.array([1., 3., 6., 10.])
U_STAR = np.array([0., .13, .30, .53, .87])
T_STAR = np.array([[-.4, -.2, -.1, -.05, 0], [-.2, -.1, -.05, 0, 0],
                   [0., 0, 0, 0, 0], [.2, .1, .05, 0, 0], [.3, .2, .1, .05, 0]])
INV_L = np.array([[-.08, -.05, -.02, -.01, 0], [-.05, -.02, -.01, 0, 0],
                  [0., 0, 0, 0, 0], [.04, .02, .01, 0, 0], [.06, .04, .02, .01, 0]])
A_EDGES = np.array([-.7, -.2, .2, .7])
B_EDGES = np.array([-.08, -.02, .02, .08])
A_REP = np.array([-1., -.4, 0, .4, 1.])
B_REP = np.array([-.12, -.04, 0, .04, .12])
SECTORS = 16
DIRECTIONS = np.arange(SECTORS) * (2 * np.pi / SECTORS)
FREQUENCIES = 1000 * 10. ** (.3 * np.arange(-4, 4))  # ISO 266 exact midbands.


@njit(cache=True)
def favourable(wind_class, stability, cosine):
    """Positive representative c(10 m)-c(0); dimensionally consistent A."""
    us = U_STAR[wind_class]
    ts = T_STAR[stability, wind_class]
    il = INV_L[stability, wind_class]
    k = 340. / (2 * 273.)
    a = us * cosine / .4 + k * .74 * ts / .4
    wc, tc = (1., .74) if stability < 3 else (4.7, 4.7)
    b = wc * us * cosine * il / .4 + k * (tc * ts * il / .4 - 9.81 / 1005.)
    ai = np.searchsorted(A_EDGES, a)
    bi = np.searchsorted(B_EDGES, b)
    return A_REP[ai] * math.log(1 + 10 / .025) + 10 * B_REP[bi] > 0


@njit(cache=True)
def alpha(frequency, temperature_k, humidity_percent, pressure_kpa):
    """ISO 9613-1:1993 equations (3)–(5), dB/km, actual surface pressure."""
    t = temperature_k / 293.15
    pressure = pressure_kpa / 101.325
    saturation = 10. ** (-6.8346 * (273.16 / temperature_k) ** 1.261 + 4.6151)
    h = humidity_percent * saturation / pressure
    oxygen = pressure * (24 + 4.04e4 * h * (.02 + h) / (.391 + h))
    nitrogen = pressure * t ** -.5 * (9 + 280 * h * math.exp(-4.170 * (t ** (-1 / 3) - 1)))
    return 8686 * frequency ** 2 * (1.84e-11 / pressure * t ** .5 + t ** -2.5 * (
        .01275 * math.exp(-2239.1 / temperature_k) / (oxygen + frequency ** 2 / oxygen)
        + .1068 * math.exp(-3352. / temperature_k) / (nitrogen + frequency ** 2 / nitrogen)))


@njit(cache=True)
def relative_humidity(temperature_k, dewpoint_k):
    """Liquid-water Magnus ratio, Alduchov & Eskridge (1996), 17.625 / 243.04 C."""
    t, d = temperature_k - 273.15, dewpoint_k - 273.15
    return min(100., max(0., 100 * math.exp(17.625 * d / (243.04 + d) - 17.625 * t / (243.04 + t))))


def solar_parameters(timestamp):
    """Low-order solar ephemeris used by the station pilot; horizon is geometric."""
    n = timestamp / 86400 + 2440587.5 - 2451545.
    mean = math.radians((280.460 + .9856474 * n) % 360)
    anomaly = math.radians((357.528 + .9856003 * n) % 360)
    longitude = mean + math.radians(1.915 * math.sin(anomaly) + .020 * math.sin(2 * anomaly))
    obliquity = math.radians(23.439 - 4e-7 * n)
    declination = math.asin(math.sin(obliquity) * math.sin(longitude))
    right_ascension = math.atan2(math.cos(obliquity) * math.sin(longitude), math.cos(longitude))
    angle = math.radians(((18.697374558 + 24.06570982441908 * n) % 24) * 15) - right_ascension
    return declination, angle


@njit(cache=True, parallel=True)
def accumulate(values, periods, daylight, histograms, counts, favourable_counts, means, m2):
    """One time step; disjoint cells own all writes and use Welford population moments."""
    for cell in prange(values.shape[1]):
        u, v, temperature, humidity, cloud, pressure = values[:, cell]
        speed = math.hypot(u, v)
        wind_class = np.searchsorted(W_EDGES, speed, side='right')
        okta = round(min(1., max(0., cloud)) * 8)
        stability = (0 if okta <= 2 else 1 if okta <= 5 else 2) if daylight[cell] else (3 if okta >= 5 else 4)
        period = periods[cell]
        # ERA5 u/v point downwind: their dot product is with source→receiver.
        direction = math.atan2(u, v) % (2 * math.pi)
        wind_bin = int((direction * 180 / math.pi + 10) % 360 / 20)
        histograms[cell, period, wind_class, stability, wind_bin] += 1
        counts[cell, period] += 1
        count = counts[cell, period]
        for sector in range(SECTORS):
            cosine = math.cos(direction - DIRECTIONS[sector])
            favourable_counts[cell, period, sector] += favourable(wind_class, stability, cosine)
        for band in range(8):
            value = alpha(FREQUENCIES[band], temperature, humidity, pressure)
            delta = value - means[cell, period, band]
            means[cell, period, band] += delta / count
            m2[cell, period, band] += delta * (value - means[cell, period, band])


@njit(cache=True, parallel=True)
def prepare_hour(raw, zone_periods, zone_indices, latitudes, longitudes, declination, angle):
    cells = raw.shape[1]
    daylight = np.empty(cells, dtype=np.bool_)
    periods = np.empty(cells, dtype=np.uint8)
    for cell in prange(cells):
        lat, lon = latitudes[cell] * math.pi / 180, longitudes[cell] * math.pi / 180
        daylight[cell] = math.sin(lat) * math.sin(declination) + math.cos(lat) * math.cos(declination) * math.cos(angle + lon) > 0
        periods[cell] = zone_periods[zone_indices[cell]]
        raw[3, cell] = relative_humidity(raw[2, cell], raw[3, cell])
        raw[5, cell] /= 1000  # Pa → kPa.
    return periods, daylight


def empty_state(cells):
    return dict(histograms=np.zeros((cells, 3, 5, 5, 18), dtype=np.uint32),
                counts=np.zeros((cells, 3), dtype=np.uint32),
                favourable_counts=np.zeros((cells, 3, SECTORS), dtype=np.uint32),
                means=np.zeros((cells, 3, 8)), m2=np.zeros((cells, 3, 8)))
