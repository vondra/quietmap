"""The one screening-height ladder, the demand storey count derived from it, and wall defaults."""

import math

from structure_contract import (
    HEIGHT_SOURCE_OSM_HEIGHT, HEIGHT_SOURCE_FLOORS, HEIGHT_SOURCE_AREA_TYPOLOGY,
    HEIGHT_SOURCE_REGIONAL_MEASURED, HEIGHT_SOURCE_GHSL, HEIGHT_SOURCE_OVERTURE_HEIGHT,
    HEIGHT_SOURCE_WALL_DEFAULT, STOREYS_SOURCE_FLOORS, STOREYS_SOURCE_LADDER_HEIGHT,
    STOREYS_SOURCE_SINGLE_LEVEL,
)

# == noise_compute::constants::BUILDING_FLOOR_HEIGHT_M.
STOREY_HEIGHT_M = 3.0
# Screening height is the mean roof height. IPR Praha LiDAR mean heights against OSM floors
# (105,957 Prague buildings, 2026-09-24): reference minus floors x 3 m has median 3.0 m;
# floors x 3 m + 3 m gives median error 0.0 m and MAE 2.84 m (+2 m: 3.01 m, +0 m: 3.96 m).
ROOF_ALLOWANCE_M = 3.0
# Machine-learned Overture heights below one low storey are artefacts: 1,002,248 footprints
# screened at 0 m in r260919 (US 352,812, AU 324,692), 13,211 at exactly 1.0 m in one
# Queensland tile.
OVERTURE_MIN_HEIGHT_M = 2.5
# GHS-BUILT-H ANBH R2023A bottoms out at 2.5 m (89-95 % of built cells in rural CZ, PL, IN
# and KE windows), and its values below 3.5 m, which the former 3-100 m clamp stored as 3 m,
# are no better: 475 footprints of at least 30 m2 under them in Legden, Eslohe, Oosterwolde and
# Auneau have median reference heights of 6.1-8.4 m (NRW LoD1, 3DBAG, BD TOPO, 2026-09-24).
GHSL_NO_INFORMATION_BELOW_M = 3.5
GHSL_MAX_M = 100.0
REGIONAL_CLAMP_M = (2.5, 250.0)
# A 100 m cell average says nothing about a shed: reference heights of footprints under
# 30 m2 have median 2.9 m and 75th percentile 4 m (NRW LoD1, BD TOPO, 3DBAG; 3,947 rows in
# seven EU pilot windows), yet GHSL gave them 9.0 m (median, n = 3,067).
SMALL_FOOTPRINT_M2 = 30.0
SMALL_FOOTPRINT_GHSL_CAP_M = 4.0
# Median reference height by footprint area for footprints without per-building height
# (12,179 rows of seven EU pilot windows: NRW LoD1 Legden/Eslohe, BD TOPO Saint-Maur/Paris
# 11e/Auneau, 3DBAG Amersfoort/Oosterwolde, 2026-09-24). Leave-one-window-out with the small
# footprint cap: pooled MAE 3.42 -> 2.44 m. Untested outside Europe.
AREA_TYPOLOGY_HEIGHT_M = ((30.0, 2.9), (60.0, 5.4), (150.0, 7.4), (500.0, 8.8), (math.inf, 10.6))

# Mean height of unmapped noise walls where a national statistic exists, else 3 m:
# DE BMDV/FBA "Laermschutz an Bundesfernstrassen 2022" (10.0 M m2 of walls over 2,576 km);
# US FHWA Noise Barrier Inventory 2022 (8,783 barriers, length-weighted); AT ASFINAG
# (about 5 km2 over about 1,406 km, end 2022). A mapped OSM height always wins.
WALL_DEFAULT_HEIGHT_M_BY_COUNTRY = {"DE": 3.88, "US": 4.45, "AT": 3.6}
WALL_DEFAULT_HEIGHT_M = 3.0


def finite_positive(value):
    return value is not None and math.isfinite(value) and value > 0


def area_typology_height_m(footprint_m2):
    return next(height for upper, height in AREA_TYPOLOGY_HEIGHT_M if footprint_m2 < upper)


def screening_height_and_source(regional_m, osm_height_m, floors, overture_height_m,
                                ghsl_m, footprint_m2):
    """First available rung wins; every footprint gets exactly one height and its source."""
    if regional_m is not None:
        return min(max(regional_m, REGIONAL_CLAMP_M[0]), REGIONAL_CLAMP_M[1]), \
            HEIGHT_SOURCE_REGIONAL_MEASURED
    if finite_positive(osm_height_m):
        return float(osm_height_m), HEIGHT_SOURCE_OSM_HEIGHT
    if floors:
        return floors * STOREY_HEIGHT_M + ROOF_ALLOWANCE_M, HEIGHT_SOURCE_FLOORS
    if overture_height_m is not None and math.isfinite(overture_height_m) \
            and overture_height_m >= OVERTURE_MIN_HEIGHT_M:
        return float(overture_height_m), HEIGHT_SOURCE_OVERTURE_HEIGHT
    if ghsl_m is not None and math.isfinite(ghsl_m) and ghsl_m >= GHSL_NO_INFORMATION_BELOW_M:
        height = min(float(ghsl_m), GHSL_MAX_M)
        if footprint_m2 < SMALL_FOOTPRINT_M2:
            height = min(height, SMALL_FOOTPRINT_GHSL_CAP_M)
        return height, HEIGHT_SOURCE_GHSL
    return area_typology_height_m(footprint_m2), HEIGHT_SOURCE_AREA_TYPOLOGY


def needs_ghsl(osm_height_m, floors, overture_height_m):
    """GHSL is sampled only where no per-building rung below the regional survey can answer."""
    return not (finite_positive(osm_height_m) or floors or (
        overture_height_m is not None and math.isfinite(overture_height_m)
        and overture_height_m >= OVERTURE_MIN_HEIGHT_M))


def demand_storeys_and_source(floors, ladder_height_m):
    """Storeys the demand model reads: mapped or national floors, else the inverse of the
    floors rung on the ladder height, else (no screening height) one level."""
    if floors:
        return min(int(floors), 255), STOREYS_SOURCE_FLOORS
    if ladder_height_m is None:
        return 1, STOREYS_SOURCE_SINGLE_LEVEL
    storeys = math.floor((ladder_height_m - ROOF_ALLOWANCE_M) / STOREY_HEIGHT_M + 0.5)
    return min(max(storeys, 1), 255), STOREYS_SOURCE_LADDER_HEIGHT


def wall_height_and_source(mapped_height_m, mapped, country_iso):
    """Mapped OSM wall height, else the country's mean wall height."""
    if mapped:
        return float(mapped_height_m), HEIGHT_SOURCE_OSM_HEIGHT
    country = (chr(country_iso & 255) + chr(country_iso >> 8)) if country_iso else ""
    return WALL_DEFAULT_HEIGHT_M_BY_COUNTRY.get(country, WALL_DEFAULT_HEIGHT_M), \
        HEIGHT_SOURCE_WALL_DEFAULT
