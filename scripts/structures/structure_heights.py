"""The one screening-height ladder, the demand storey count derived from it, and wall defaults."""

import math

from structure_contract import (
    HEIGHT_SOURCE_OSM_HEIGHT, HEIGHT_SOURCE_FLOORS, HEIGHT_SOURCE_AREA_TYPOLOGY,
    HEIGHT_SOURCE_REGIONAL_MEASURED, HEIGHT_SOURCE_OVERTURE_HEIGHT,
    HEIGHT_SOURCE_WALL_DEFAULT, STOREYS_SOURCE_FLOORS, STOREYS_SOURCE_LADDER_HEIGHT,
    STOREYS_SOURCE_SINGLE_LEVEL,
)

# == noise_compute::constants::BUILDING_FLOOR_HEIGHT_M.
STOREY_HEIGHT_M = 3.0
# Machine-learned Overture heights below one low storey are artefacts: 1,002,248 footprints
# screened at 0 m in r260919 (US 352,812, AU 324,692), 13,211 at exactly 1.0 m in one
# Queensland tile.
OVERTURE_MIN_HEIGHT_M = 2.5
REGIONAL_CLAMP_M = (2.5, 250.0)
# Median mean-roof height by footprint area for footprints without per-building
# height, over no-information rows only (2026-09-25): NRW LoD1 is already a
# mean roof (Geobasis NRW: "mittlere Dachhoehe des Gebaeudes im LoD2"), BD TOPO
# mean is (eave + ridge) / 2, 3DBAG is the 70th percentile. German isolated
# farmhouses (60-150 m2: 9.1 m, n = 282) and French ones (6.5 m, n = 108)
# bracket the middle bin; 7.4 splits that error. A 100 m cell average loses to
# this typology on mean-roof references in every testable window, so the ladder
# keeps no satellite rung: rows without per-building evidence stop here.
AREA_TYPOLOGY_HEIGHT_M = ((30.0, 2.9), (60.0, 3.5), (150.0, 7.4), (500.0, 8.0), (math.inf, 9.0))

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


def floors_height_m(floors):
    """Mean roof height from a floor count. One floor counts its attic (floor
    counts exclude it: +4.0 m median residual in Prague); two and three floors
    sit at one storey each; taller blocks gain 2 m of roof. Fit on mean-roof
    references, never on stations (2026-09-25): IPR Praha LiDAR zonal mean vs
    OSM floors (105,409 buildings), BD TOPO (eave + ridge) / 2 vs
    nombre_d_etages (Saint-Maur 2,022, Auneau 787), 3DBAG 70th percentile vs
    bouwlagen (Amersfoort 1,666, Oosterwolde 1,586)."""
    if floors <= 1:
        return STOREY_HEIGHT_M + 3.0
    if floors <= 3:
        return floors * STOREY_HEIGHT_M
    return floors * STOREY_HEIGHT_M + 2.0


def screening_height_and_source(regional_m, osm_height_m, floors, overture_height_m,
                                footprint_m2):
    """First available rung wins; every footprint gets exactly one height and its source."""
    if regional_m is not None:
        return min(max(regional_m, REGIONAL_CLAMP_M[0]), REGIONAL_CLAMP_M[1]), \
            HEIGHT_SOURCE_REGIONAL_MEASURED
    if finite_positive(osm_height_m):
        return float(osm_height_m), HEIGHT_SOURCE_OSM_HEIGHT
    if floors:
        return floors_height_m(floors), HEIGHT_SOURCE_FLOORS
    if overture_height_m is not None and math.isfinite(overture_height_m) \
            and overture_height_m >= OVERTURE_MIN_HEIGHT_M:
        return float(overture_height_m), HEIGHT_SOURCE_OVERTURE_HEIGHT
    return area_typology_height_m(footprint_m2), HEIGHT_SOURCE_AREA_TYPOLOGY


def demand_storeys_and_source(floors, ladder_height_m):
    """Storeys the demand model reads: mapped or national floors, else the height
    estimator below, else (no screening height) one level. The estimator inverts
    to registry floor counts (mean height minus 1 m over 3 m per storey: MAE 0.41
    storeys, unbiased, on 6,061 registry-counted buildings in five windows)."""
    if floors:
        return min(int(floors), 255), STOREYS_SOURCE_FLOORS
    if ladder_height_m is None:
        return 1, STOREYS_SOURCE_SINGLE_LEVEL
    storeys = math.floor((ladder_height_m - 1.0) / STOREY_HEIGHT_M + 0.5)
    return min(max(storeys, 1), 255), STOREYS_SOURCE_LADDER_HEIGHT


def wall_height_and_source(mapped_height_m, mapped, country_iso):
    """Mapped OSM wall height, else the country's mean wall height."""
    if mapped:
        return float(mapped_height_m), HEIGHT_SOURCE_OSM_HEIGHT
    country = (chr(country_iso & 255) + chr(country_iso >> 8)) if country_iso else ""
    return WALL_DEFAULT_HEIGHT_M_BY_COUNTRY.get(country, WALL_DEFAULT_HEIGHT_M), \
        HEIGHT_SOURCE_WALL_DEFAULT
