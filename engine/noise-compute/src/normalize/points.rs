//! Point-source normalization: building / industrial / leisure AREA rows →
//! discretised per-cell [`PreparedPoint`]s (GFA-scaled Lw, self-screening
//! exclusion, emission-derived cull radius) the popup and heatmap loaders share.

use crate::constants::BUILDING_HEIGHT_MAX_M;
use crate::emission::{industrial, leisure, settlement, ships, wind};
use crate::types::{PointSource, NUM_BANDS};

use super::{bands_to_f32, resolve_area_m2};

use grid::geo::M_PER_DEG_LAT;
use grid::poly::{ring_bbox_lonlat, ring_contains, snap_lonlat, GridRing};

use crate::constants::M_PER_DEG_LON_EQ;

#[derive(Debug, Clone, Copy)]
pub struct RawBuildingInput<'a> {
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub height_m: f32,
    pub floors: u8,
    pub area_source: bool,
    pub building_type: u8,
    pub area_m2: Option<f64>,
    /// Snapped z30 grid ring (empty = unavailable).
    pub polygon_grid: &'a [(i32, i32)],
}

#[derive(Debug, Clone, Copy)]
pub struct RawIndustrialInput<'a> {
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub source_type: u8,
    pub site_subtype: u8,
    pub hub_height_m: Option<f32>,
    pub rated_power_kw: Option<f32>,
    pub area_m2: Option<f64>,
    /// Snapped z30 grid ring (empty = unavailable).
    pub polygon_grid: &'a [(i32, i32)],
    pub nace_4digit: Option<u16>,
    /// Solar nameplate MW parsed from the row's `plant:output:electricity`
    /// tag. `None` = untagged → area ×
    /// [`industrial::SOLAR_MW_PER_HA_UNTAGGED`].
    pub plant_output_mw: Option<f64>,
    /// Substation total MVA: summed `rating` values of the class-15
    /// transformers inside the substation polygon (per-square spatial join).
    /// `None` = no rated transformers found → the
    /// [`industrial::substation_class_mva`] median.
    pub substation_mva: Option<f64>,
    /// Substation class derived from `voltage` / autotransformer evidence:
    /// 1 main, 2 auto, 3 distribution, 0 unknown. Only read when
    /// `substation_mva` is `None`.
    pub substation_class: u8,
}

/// One `leisure.arrow` row — a sports/play/open-air-hospitality/car-park source
/// (settlement v2 phase 2). `sport` selects the per-type level
/// (`leisure::leisure_profile`); the polygon `area_m2` is the only size driver
/// (unified area-law with buildings). No floors/height — leisure is an open-air
/// activity source at a fixed ~1.5 m, not a GFA-scaled building. A formula
/// class instead spreads its reader-resolved `formula` total over the row
/// geometry (area, line chain, or centroid point).
#[derive(Debug, Clone, Copy)]
pub struct RawLeisureInput<'a> {
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    /// `leisure::PITCH`/`PADEL`/… class id.
    pub sport: u8,
    /// Polygon footprint — the ONLY size driver (unified area-law with
    /// buildings). A leisure NODE with no polygon falls back to the profile's
    /// reference footprint in `prepare_leisure_points`.
    pub area_m2: Option<f64>,
    /// Snapped z30 grid ring — or the open/closed raceway chain of a line
    /// row (`is_line`; empty = unavailable).
    pub polygon_grid: &'a [(i32, i32)],
    /// Resolved formula emission for a class-10/11 row (`None` = area-law
    /// class, or a silenced sub-type). Resolved by the reader from the row's
    /// retained tags so both loaders share one call.
    pub formula: Option<leisure::FormulaEmission>,
    /// True for a raceway/track line row (`geometry_kind = 2`).
    pub is_line: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedPoint {
    pub lat: f64,
    pub lon: f64,
    pub source_height_m: f32,
    pub lw_day: [f32; NUM_BANDS],
    pub lw_evening: [f32; NUM_BANDS],
    pub lw_night: [f32; NUM_BANDS],
    pub n_points: u16,
    pub exclusion_radius_m: f32,
    pub max_radius_m: f64,
    /// Building floors used in the Lw GFA computation (resolved
    /// from OSM `building:levels` or derived from
    /// height_m / BUILDING_FLOOR_HEIGHT_M when the tag is absent).
    /// 0 for emission-only grounds, leisure, industrial and wind-turbine sources.
    pub floors: u8,
    /// Building polygon area (m²) used in the Lw GFA computation.
    /// 0 for industrial point sources without polygon coverage.
    pub area_m2: f32,
    /// Wind turbine hub height (m). `None` for buildings + ordinary
    /// industrial sites. Carried so the popup `EmissionTrace::
    /// Industrial.hub_height_m` matches what the engine used for
    /// `source_height_m`.
    pub hub_height_m: Option<f32>,
    /// Wind turbine rated power (kW). Same purpose as hub_height_m
    /// — `None` outside the wind-turbine branch.
    pub rated_power_kw: Option<f32>,
    /// Ship cell hours per month by class; `None` outside the ship layer.
    pub ship_hours: Option<[f32; 3]>,
}

const INDUSTRIAL_AREA_CELL_M: f64 = 75.0;
const BUILDING_AREA_CELL_M: f64 = 30.0;
/// Sub-cell sampling density for [`ring_area_grid_points`], shared by every
/// area source (building / industrial / leisure) — not industrial-specific.
const AREA_CELL_SAMPLES: usize = 5;
const INDUSTRIAL_AREA_THRESHOLD_M2: f64 = 5_000.0;
const BUILDING_AREA_THRESHOLD_M2: f64 = 2_000.0;

impl PreparedPoint {
    pub fn with_metadata(
        &self,
        osm_id: i64,
        source_type: u8,
        name: String,
        polygon_grid: GridRing,
        dist_m: f64,
    ) -> PointSource {
        PointSource {
            osm_id,
            lat: self.lat,
            lon: self.lon,
            source_height_m: self.source_height_m,
            source_type,
            lw_day: self.lw_day,
            lw_evening: self.lw_evening,
            lw_night: self.lw_night,
            n_points: self.n_points,
            name,
            polygon_grid,
            exclusion_radius_m: self.exclusion_radius_m,
            max_radius_m: self.max_radius_m,
            source_id: 0, // populated by downstream callers (building/industrial loaders)
            floors: self.floors,
            area_m2: self.area_m2,
            hub_height_m: self.hub_height_m,
            rated_power_kw: self.rated_power_kw,
            ship_hours: self.ship_hours,
            dist_m,
        }
    }
}

/// Geometry + popup metadata for one AREA source (building / industrial /
/// leisure footprint), consumed by [`discretize_area_source`].
struct AreaSource<'a> {
    polygon_grid: &'a [(i32, i32)],
    centroid_lat: f64,
    centroid_lon: f64,
    area_m2: f64,
    /// Footprint at/below which a single centroid point carries the full
    /// emission (building 2000 m², industrial/leisure 5000 m²).
    grid_threshold_m2: f64,
    /// Square-grid cell pitch (building 30 m, industrial/leisure 75 m).
    cell_m: f64,
    source_height_m: f32,
    reach: PointReach,
    /// Echoed to `PreparedPoint` for the popup trace (0 / None outside
    /// buildings & wind turbines).
    floors: u8,
    hub_height_m: Option<f32>,
    rated_power_kw: Option<f32>,
}

/// How one discretised point gets its receiver-enumeration radius.
/// Buildings and leisure retain their parent source's Lw-derived radius;
/// industrial area cells use their own post-split loudest day band because the
/// exact receiver gate tests that same value.
#[derive(Clone, Copy)]
enum PointReach {
    Fixed(f64),
    LoudestDayBand { cap_m: f64 },
}

fn loudest_day_band_db(lw_day: &[f32; NUM_BANDS]) -> f64 {
    lw_day.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64
}

impl PointReach {
    fn resolve(self, lw_day: &[f32; NUM_BANDS]) -> f64 {
        match self {
            Self::Fixed(radius_m) => radius_m,
            Self::LoudestDayBand { cap_m } => {
                grid::geo::point_source_audibility_radius(loudest_day_band_db(lw_day), cap_m)
            }
        }
    }
}

/// Derive the evening + night band arrays from the day bands by adding the
/// profile's flat per-period offset to every band. Shared by building /
/// industrial / leisure so the period derivation can't drift between them.
fn period_offset_bands(
    lw_day: [f32; NUM_BANDS],
    evening_offset: f64,
    night_offset: f64,
) -> ([f32; NUM_BANDS], [f32; NUM_BANDS]) {
    let mut evening = lw_day;
    let mut night = lw_day;
    for i in 0..NUM_BANDS {
        evening[i] += evening_offset as f32;
        night[i] += night_offset as f32;
    }
    (evening, night)
}

/// One weighted area cell: mean position of its inside samples + covered area.
struct AreaGridPoint {
    lat: f64,
    lon: f64,
    area_m2: f64,
}

/// Generate weighted square area cells inside a snapped z30 grid ring.
///
/// Same sampling contract the WKB-hex version had: each returned point
/// represents the covered ring area inside one `spacing_m` cell. Subsampling
/// keeps narrow footprints and clipped boundary cells from disappearing when
/// the cell center alone falls outside the ring. Rings carry outers only
/// (no holes), so containment is [`ring_contains`]. Falls back to the ring's
/// mean-vertex centroid (area `spacing_m²`) when no cell catches a sample.
fn ring_area_grid_points(
    ring: &[(i32, i32)],
    spacing_m: f64,
    samples_per_axis: usize,
) -> Vec<AreaGridPoint> {
    let Some([min_lat, min_lon, max_lat, max_lon]) = ring_bbox_lonlat(ring) else {
        return vec![];
    };
    if spacing_m <= 0.0 {
        return vec![];
    }

    let samples = samples_per_axis.max(1);
    let mid_lat = (min_lat + max_lat) / 2.0;
    let lat_step = spacing_m / M_PER_DEG_LAT;
    let lon_step = spacing_m / (M_PER_DEG_LON_EQ * mid_lat.to_radians().cos().max(0.1));
    let sample_area_m2 = spacing_m * spacing_m / (samples * samples) as f64;

    let mut points = Vec::new();
    let lat_start = (min_lat / lat_step).floor() * lat_step + lat_step / 2.0;
    let lon_start = (min_lon / lon_step).floor() * lon_step + lon_step / 2.0;
    let mut lat = lat_start;
    while lat <= max_lat {
        let mut lon = lon_start;
        while lon <= max_lon {
            let mut count = 0usize;
            let mut sum_lat = 0.0;
            let mut sum_lon = 0.0;
            for sy in 0..samples {
                let yoff = ((sy as f64 + 0.5) / samples as f64 - 0.5) * lat_step;
                let sample_lat = lat + yoff;
                for sx in 0..samples {
                    let xoff = ((sx as f64 + 0.5) / samples as f64 - 0.5) * lon_step;
                    let sample_lon = lon + xoff;
                    let (gx, gy) = snap_lonlat(sample_lon, sample_lat);
                    if ring_contains(ring, gx, gy) {
                        count += 1;
                        sum_lat += sample_lat;
                        sum_lon += sample_lon;
                    }
                }
            }
            if count > 0 {
                points.push(AreaGridPoint {
                    lat: sum_lat / count as f64,
                    lon: sum_lon / count as f64,
                    area_m2: sample_area_m2 * count as f64,
                });
            }
            lon += lon_step;
        }
        lat += lat_step;
    }

    if points.is_empty() {
        let (clat, clon) = ring_vertex_centroid(ring);
        points.push(AreaGridPoint {
            lat: clat,
            lon: clon,
            area_m2: spacing_m * spacing_m,
        });
    }

    points
}

/// Mean vertex position of a snapped ring — the fallback point when the grid
/// samples nothing (a footprint too small to catch a grid line).
fn ring_vertex_centroid(ring: &[(i32, i32)]) -> (f64, f64) {
    let (mut slat, mut slon, mut n) = (0.0f64, 0.0f64, 0usize);
    for &(gx, gy) in ring {
        let (x, y) = grid::grid_to_meters(gx, gy);
        let (lon, lat) = grid::poly::meters_to_lonlat(x, y);
        slat += lat;
        slon += lon;
        n += 1;
    }
    let n = n.max(1) as f64;
    (slat / n, slon / n)
}

/// Discretise an AREA source into per-cell [`PreparedPoint`]s — ONE source of
/// truth for building / industrial / leisure. Above `grid_threshold_m2` the
/// footprint is gridded at `cell_m` ([`ring_area_grid_points`]): each cell
/// carries its AREA FRACTION of the emission
/// (`Lw_cell = Lw − 10·log10(area_tot/area_cell)`, energy-conserving) and a
/// self-screening exclusion radius `√(area_cell/π)` so the source's own
/// footprint is neither a propagation barrier
/// (`path_effects::screening_attenuation`) nor a 1/r² singularity
/// (`geo::effective_area_source_dist`). Below the threshold (or no polygon) →
/// one centroid point with the full emission.
fn discretize_area_source(
    src: AreaSource<'_>,
    lw_day: [f32; NUM_BANDS],
    lw_evening: [f32; NUM_BANDS],
    lw_night: [f32; NUM_BANDS],
) -> Vec<PreparedPoint> {
    let weighted_points: Vec<(f64, f64, f64)> =
        if src.area_m2 > src.grid_threshold_m2 && !src.polygon_grid.is_empty() {
            let cells = ring_area_grid_points(src.polygon_grid, src.cell_m, AREA_CELL_SAMPLES);
            if cells.len() > 1 {
                let sampled_area = cells.iter().map(|p| p.area_m2).sum::<f64>().max(1.0);
                cells
                    .into_iter()
                    .map(|p| (p.lat, p.lon, p.area_m2 * src.area_m2 / sampled_area))
                    .collect()
            } else {
                vec![(src.centroid_lat, src.centroid_lon, src.area_m2)]
            }
        } else {
            vec![(src.centroid_lat, src.centroid_lon, src.area_m2)]
        };
    let n_points = weighted_points.len().min(u16::MAX as usize) as u16;

    weighted_points
        .into_iter()
        .map(|(lat, lon, point_area)| {
            let lw_split = 10.0 * ((src.area_m2 / point_area.max(1.0)) as f32).log10();
            let mut day = lw_day;
            let mut evening = lw_evening;
            let mut night = lw_night;
            for band in 0..NUM_BANDS {
                day[band] -= lw_split;
                evening[band] -= lw_split;
                night[band] -= lw_split;
            }
            PreparedPoint {
                lat,
                lon,
                source_height_m: src.source_height_m,
                lw_day: day,
                lw_evening: evening,
                lw_night: night,
                n_points,
                exclusion_radius_m: (point_area as f32 / std::f32::consts::PI).sqrt(),
                max_radius_m: src.reach.resolve(&day),
                floors: src.floors,
                area_m2: src.area_m2 as f32,
                hub_height_m: src.hub_height_m,
                rated_power_kw: src.rated_power_kw,
                ship_hours: None,
            }
        })
        .collect()
}

/// Discretise a raceway LINE source (an open or closed z30 chain) into
/// per-segment [`PreparedPoint`]s. The chain is walked in ~`cell_m` segments
/// (a two-node line is one segment); each point sits at its segment midpoint
/// and carries its LENGTH FRACTION of the total (`Lw_seg = Lw − 10·lg(n)`,
/// energy-conserving) with a half-segment exclusion radius — the line
/// analogue of the area cells above — and the post-split loudest-day-band
/// reach capped at the industrial horizon. A chain shorter than two vertices
/// emits from the centroid point with the full total.
#[allow(clippy::too_many_arguments)]
fn discretize_line_source(
    chain: &[(i32, i32)],
    centroid_lat: f64,
    centroid_lon: f64,
    cell_m: f64,
    source_height_m: f32,
    lw_day: [f32; NUM_BANDS],
    lw_evening: [f32; NUM_BANDS],
    lw_night: [f32; NUM_BANDS],
) -> Vec<PreparedPoint> {
    // Segment midpoints in lon/lat with their lengths; vertices densify long
    // legs so no segment spans more than one cell.
    let mut cells: Vec<(f64, f64, f64)> = Vec::new();
    if chain.len() >= 2 {
        let legs = chain.len() - 1;
        for leg in 0..legs {
            let (ax, ay) = chain[leg];
            let (bx, by) = chain[leg + 1];
            let grid_lonlat = |gx: i32, gy: i32| {
                let (x, y) = grid::grid_to_meters(gx, gy);
                grid::poly::meters_to_lonlat(x, y)
            };
            let (alon, alat) = grid_lonlat(ax, ay);
            let (blon, blat) = grid_lonlat(bx, by);
            let leg_m = grid::geo::flat_dist(alat, alon, blat, blon);
            let splits = ((leg_m / cell_m).ceil() as usize).max(1);
            for split in 0..splits {
                let (t0, t1) = (split as f64 / splits as f64, (split + 1) as f64 / splits as f64);
                cells.push((
                    alat + (blat - alat) * (t0 + t1) / 2.0,
                    alon + (blon - alon) * (t0 + t1) / 2.0,
                    leg_m / splits as f64,
                ));
            }
        }
    }
    if cells.is_empty() {
        cells.push((centroid_lat, centroid_lon, cell_m));
    }
    let n_points = cells.len().min(u16::MAX as usize) as u16;
    let lw_split = 10.0 * (cells.len() as f32).log10();
    let reach = PointReach::LoudestDayBand {
        cap_m: crate::constants::INDUSTRIAL_MAX_RADIUS,
    };
    cells
        .into_iter()
        .map(|(lat, lon, seg_m)| {
            let mut day = lw_day;
            let mut evening = lw_evening;
            let mut night = lw_night;
            for band in 0..NUM_BANDS {
                day[band] -= lw_split;
                evening[band] -= lw_split;
                night[band] -= lw_split;
            }
            PreparedPoint {
                lat,
                lon,
                source_height_m,
                lw_day: day,
                lw_evening: evening,
                lw_night: night,
                n_points,
                exclusion_radius_m: (seg_m / 2.0).max(1.0) as f32,
                max_radius_m: reach.resolve(&day),
                floors: 0,
                area_m2: 0.0,
                hub_height_m: None,
                rated_power_kw: None,
                ship_hours: None,
            }
        })
        .collect()
}

pub fn prepare_building_points(input: RawBuildingInput<'_>) -> Vec<PreparedPoint> {
    let actual_height = if input.area_source {
        0.0
    } else if input.height_m > 0.0 {
        input.height_m
    } else if input.floors > 0 {
        input.floors as f32 * crate::constants::BUILDING_FLOOR_HEIGHT_M as f32
    } else {
        crate::constants::BUILDING_DEFAULT_HEIGHT_M as f32
    }
    .min(BUILDING_HEIGHT_MAX_M as f32);
    let actual_floors = if input.area_source {
        0
    } else if input.floors > 0 {
        input.floors
    } else {
        (actual_height / crate::constants::BUILDING_FLOOR_HEIGHT_M as f32).ceil() as u8
    };
    let area = resolve_area_m2(input.area_m2, input.polygon_grid, 100.0);

    let profile = settlement::building_profile(input.building_type);
    // Shed-types scale on FOOTPRINT, not floors, so a tall single-story hall
    // isn't counted as `height/3` floors (rationale in `settlement::is_shed_type`).
    let lw_floors = if input.area_source || settlement::is_shed_type(input.building_type) {
        1
    } else {
        actual_floors
    };
    let lw = settlement::building_lw(&profile, area, lw_floors);
    if lw < 10.0 {
        return Vec::new();
    }

    let lw_day = bands_to_f32(settlement::building_emission_bands(&profile, lw));
    let (lw_evening, lw_night) =
        period_offset_bands(lw_day, profile.evening_offset, profile.night_offset);

    // Cull radius solved against the honest radiated lw (settlement v2 phase 1):
    // one scalar, one meaning (`building_max_dist` caps at 2 km internally).
    discretize_area_source(
        AreaSource {
            polygon_grid: input.polygon_grid,
            centroid_lat: input.centroid_lat,
            centroid_lon: input.centroid_lon,
            area_m2: area,
            grid_threshold_m2: BUILDING_AREA_THRESHOLD_M2,
            cell_m: BUILDING_AREA_CELL_M,
            source_height_m: if input.area_source {
                crate::constants::SOURCE_HEIGHT_LEISURE as f32
            } else {
                actual_height / 2.0
            },
            reach: PointReach::Fixed(settlement::building_max_dist(lw)),
            floors: if input.area_source { 0 } else { lw_floors },
            hub_height_m: None,
            rated_power_kw: None,
        },
        lw_day,
        lw_evening,
        lw_night,
    )
}

pub fn prepare_industrial_points(input: RawIndustrialInput<'_>) -> Vec<PreparedPoint> {
    // Dedicated power classes never fall through to the generic factory area
    // law. 11/12/15 are silent: a wind-plant outline's turbines emit, not the
    // fence (Cotton Wind Farm read 43.8 dB Lden as a generic factory); an
    // inactive facility is retired; a transformer's rating joins its
    // substation instead of emitting twice. Unknown future classes stay
    // silent until a model claims them.
    if matches!(
        input.source_type,
        industrial::SOURCE_WIND_OUTLINE
            | industrial::SOURCE_INACTIVE
            | industrial::SOURCE_TRANSFORMER
    ) || input.source_type > industrial::SOURCE_TRANSFORMER
    {
        return Vec::new();
    }
    if input.source_type == 10 {
        // Hub default 105 m = known-data median across our arrows (modern
        // fleet context: WindGuard DE 2024 average 143 m, LBNL US 2023
        // average 103.4 m — audit I-10b). Tag-error clamps per the same
        // audit: 4,792 OSM hubs >170 m and 23 rated powers ≥20 MW are tag
        // errors, not machines — hub clamps to 175 m, implausible power is
        // treated as unknown (2 MW default).
        let hub_height_m = input
            .hub_height_m
            .filter(|value| *value > 0.0)
            .map(|value| value.min(175.0))
            .unwrap_or(105.0);
        // Display keeps the assumed 2 MW for unknown ratings; the LUT gets the
        // 0 sentinel instead so a KNOWN 2.0 MW machine (V90/E-82 -> 104 dB) is
        // not conflated with unknown (-> 105) — Codex C7 review.
        let known_power = input
            .rated_power_kw
            .filter(|value| *value > 0.0 && *value <= 8000.0);
        let rated_power_kw = known_power.unwrap_or(2000.0);
        let (lw, bands) = wind::wind_turbine_emission(known_power.unwrap_or(0.0) as f64);
        if lw < 10.0 {
            return Vec::new();
        }
        let emission = bands_to_f32(bands);
        return vec![PreparedPoint {
            lat: input.centroid_lat,
            lon: input.centroid_lon,
            source_height_m: hub_height_m,
            lw_day: emission,
            lw_evening: emission,
            lw_night: emission,
            n_points: 1,
            exclusion_radius_m: 0.0,
            max_radius_m: grid::geo::point_source_audibility_radius(
                loudest_day_band_db(&emission),
                crate::constants::INDUSTRIAL_MAX_RADIUS,
            ),
            floors: 0,
            area_m2: 0.0,
            hub_height_m: Some(hub_height_m),
            rated_power_kw: Some(rated_power_kw),
            ship_hours: None,
        }];
    }

    let area = resolve_area_m2(input.area_m2, input.polygon_grid, 10000.0);
    // Solar farms (OSM class 13, or registry-confirmed 3599 — including the
    // served rows) and substations (class 14) carry their own physics: per-MW
    // / per-MVA levels, not the area law. `base_lw` is unused on these arms
    // (`lw` is computed directly); the spectrum + offsets still apply.
    let is_solar =
        input.source_type == industrial::SOURCE_SOLAR_FARM || input.nace_4digit == Some(industrial::SOLAR_NACE);
    let is_substation = input.source_type == industrial::SOURCE_SUBSTATION;
    let profile = if is_solar {
        industrial::IndustrialProfile {
            base_lw: 0.0,
            spectrum: industrial::SOLAR_SPECTRUM,
            evening_offset: -50.0, // day-only: inverters sleep at night
            night_offset: -50.0,
        }
    } else if is_substation {
        industrial::IndustrialProfile {
            base_lw: 0.0,
            spectrum: industrial::SUBSTATION_SPECTRUM,
            evening_offset: 0.0, // 24/7
            night_offset: 0.0,
        }
    } else {
        input
            .nace_4digit
            .and_then(industrial::nace_profile)
            .or_else(|| industrial::subtype_profile(input.site_subtype))
            .unwrap_or_else(|| industrial::industrial_profile(input.source_type))
    };
    let lw = if is_solar {
        // A solar generator unit (`generator:source=solar`) carries its
        // output in `rated_power_kw`, parsed by the extractor; a plant row
        // carries `plant:output:electricity` in its tags. Either beats the
        // area density for point rows, which have no footprint to scale.
        let mw = input
            .plant_output_mw
            .filter(|mw| *mw > 0.0)
            .or_else(|| {
                input
                    .rated_power_kw
                    .filter(|kw| *kw > 0.0)
                    .map(|kw| f64::from(kw) / 1000.0)
            });
        industrial::solar_farm_lw(mw, area)
    } else if is_substation {
        let mva = input
            .substation_mva
            .filter(|mva| *mva > 0.0)
            .unwrap_or_else(|| industrial::substation_class_mva(input.substation_class));
        industrial::substation_lw(mva)
    } else {
        let area_cap = industrial::sector_area_cap_m2(input.nace_4digit, input.site_subtype);
        industrial::industrial_lw(&profile, area, area_cap)
    };
    if lw < 10.0 {
        return Vec::new();
    }

    let lw_day = bands_to_f32(industrial::industrial_emission_bands(&profile, lw));
    let (lw_evening, lw_night) =
        period_offset_bands(lw_day, profile.evening_offset, profile.night_offset);

    let source_height_m = if is_solar {
        3.0 // central inverters + MV transformers
    } else if is_substation {
        5.0 // transformer tanks, radiators, fans
    } else if input.source_type == 1 {
        8.0
    } else {
        match input.nace_4digit.map(|n| n / 100) {
            // Heavy/tall sources: coal (05) + metal-ore (07) + other mining
            // & quarrying (08), coke/refining (19, stacks/flares),
            // cement/minerals (23), metallurgy (24), power generation (35).
            Some(5 | 7 | 8 | 19 | 23 | 24 | 35) => 10.0,
            _ => 5.0,
        }
    };
    discretize_area_source(
        AreaSource {
            polygon_grid: input.polygon_grid,
            centroid_lat: input.centroid_lat,
            centroid_lon: input.centroid_lon,
            area_m2: area,
            grid_threshold_m2: INDUSTRIAL_AREA_THRESHOLD_M2,
            cell_m: INDUSTRIAL_AREA_CELL_M,
            source_height_m,
            reach: PointReach::LoudestDayBand {
                cap_m: crate::constants::INDUSTRIAL_MAX_RADIUS,
            },
            floors: 0,
            hub_height_m: None,
            rated_power_kw: None,
        },
        lw_day,
        lw_evening,
        lw_night,
    )
}

/// One AIS vessel-density cell of the ships layer.
pub struct RawShipInput {
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub area_m2: f32,
    /// Mean vessel-hours per month: large ships, work boats, leisure craft.
    pub hours_per_month: [f32; 3],
}

/// Sub-cell pitch of a ship density cell: a quarter of the 1 km statistical cell, so a quay
/// receiver sees the nearest water at its true distance (≥ ~125 m) instead of the whole cell
/// clamped to its 564 m radius, while a cell costs 16 points, not 178 (the 75 m industrial pitch).
const SHIP_SUB_CELL_M: f64 = 250.0;

/// The ship cells of one water cell: the cell's mean ships present radiate the class sound
/// power spread over the cell's square footprint, gridded at [`SHIP_SUB_CELL_M`] by the shared
/// [`discretize_area_source`] (area share of the energy, self-screening exclusion √(A/π) per
/// sub-cell), 24 h flat, reaching at most [`ships::SHIP_MAX_RADIUS_M`]; plus the class carrying
/// most of the energy. `None` for a silent cell.
pub fn prepare_ship_points(input: RawShipInput) -> Option<(Vec<PreparedPoint>, ships::ShipClass)> {
    let (lw, loudest) = ships::ship_cell_lw(input.hours_per_month)?;
    if lw < 10.0 || input.area_m2.is_nan() || input.area_m2 <= 0.0 {
        return None;
    }
    let bands = bands_to_f32(ships::ship_emission_bands(lw));
    let half_m = f64::from(input.area_m2).sqrt() / 2.0;
    let d_lat = half_m / M_PER_DEG_LAT;
    let d_lon = half_m / grid::geo::m_per_deg_lon(input.centroid_lat.to_radians());
    let ring: GridRing = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|(sx, sy)| {
            snap_lonlat(
                grid::geo::normalize_longitude(input.centroid_lon + sx * d_lon),
                input.centroid_lat + sy * d_lat,
            )
        })
        .collect();
    let mut points = discretize_area_source(
        AreaSource {
            polygon_grid: &ring,
            centroid_lat: input.centroid_lat,
            centroid_lon: input.centroid_lon,
            area_m2: f64::from(input.area_m2),
            grid_threshold_m2: 0.0,
            cell_m: SHIP_SUB_CELL_M,
            source_height_m: loudest.source_height_m(),
            reach: PointReach::LoudestDayBand {
                cap_m: ships::SHIP_MAX_RADIUS_M,
            },
            floors: 0,
            hub_height_m: None,
            rated_power_kw: None,
        },
        bands,
        bands,
        bands,
    );
    for point in &mut points {
        point.ship_hours = Some(input.hours_per_month);
    }
    Some((points, loudest))
}

/// Leisure areas are LOCAL activity sources — cap reach like buildings (2 km),
/// never the 4 km industrial-plant reach. Motorsport and shooting are the
/// exception: a speedway at 136 dB carries kilometres, so formula classes
/// reach past the 2 km cap (industrial reach).
const LEISURE_MAX_RADIUS_M: f64 = 2_000.0;

/// Discretise one leisure source into per-cell [`PreparedPoint`]s — the
/// shared [`discretize_area_source`] (same area-weighted 75 m grid + self-screening
/// `√(cell_area/π)` exclusion as industrial), at ~1.5 m height (voices/rackets,
/// not roof plant) with an Lw-derived reach capped at 2 km. The level is the
/// AREA-scaled [`leisure::leisure_lw`] (UNIFIED with buildings — `settlement::area_lw`);
/// a node with no polygon falls back to the profile's reference footprint.
/// Formula classes (motorsport/shooting) instead spread their reader-resolved
/// class-TOTAL Lw over the row geometry — a raceway chain
/// ([`discretize_line_source`]), a range polygon, or the centroid point — with
/// industrial reach. Returns `[]` when the source is sub-audible.
pub fn prepare_leisure_points(input: RawLeisureInput<'_>) -> Vec<PreparedPoint> {
    if let Some(formula) = input.formula {
        let lw_day = bands_to_f32(leisure::leisure_formula_bands(&formula));
        let (lw_evening, lw_night) =
            period_offset_bands(lw_day, formula.evening_offset, formula.night_offset);
        // The total is geometry-independent; the geometry only sets the grid.
        // A geometry-less row still emits from its centroid point.
        if input.is_line {
            return discretize_line_source(
                input.polygon_grid,
                input.centroid_lat,
                input.centroid_lon,
                INDUSTRIAL_AREA_CELL_M,
                crate::constants::SOURCE_HEIGHT_LEISURE as f32,
                lw_day,
                lw_evening,
                lw_night,
            );
        }
        let area = resolve_area_m2(input.area_m2, input.polygon_grid, 10000.0);
        return discretize_area_source(
            AreaSource {
                polygon_grid: input.polygon_grid,
                centroid_lat: input.centroid_lat,
                centroid_lon: input.centroid_lon,
                area_m2: area,
                grid_threshold_m2: INDUSTRIAL_AREA_THRESHOLD_M2,
                cell_m: INDUSTRIAL_AREA_CELL_M,
                source_height_m: crate::constants::SOURCE_HEIGHT_LEISURE as f32,
                reach: PointReach::LoudestDayBand {
                    cap_m: crate::constants::INDUSTRIAL_MAX_RADIUS,
                },
                floors: 0,
                hub_height_m: None,
                rated_power_kw: None,
            },
            lw_day,
            lw_evening,
            lw_night,
        );
    }
    let profile = leisure::leisure_profile(input.sport);
    // An area-law line (an open non-motorised track) has no area to scale by;
    // it emits as a node of its class from its centroid — the chain is not
    // an area and the shoelace of an open chain is not its size.
    let ring = if input.is_line { &[][..] } else { input.polygon_grid };
    let area = resolve_area_m2(input.area_m2, ring, profile.ref_area_m2);
    let lw = leisure::leisure_lw(&profile, area);
    if lw < 10.0 {
        return Vec::new();
    }

    let lw_day = bands_to_f32(leisure::leisure_emission_bands(&profile, lw));
    let (lw_evening, lw_night) =
        period_offset_bands(lw_day, profile.evening_offset, profile.night_offset);

    let max_radius_m = settlement::building_max_dist(lw).min(LEISURE_MAX_RADIUS_M);
    discretize_area_source(
        AreaSource {
            polygon_grid: ring,
            centroid_lat: input.centroid_lat,
            centroid_lon: input.centroid_lon,
            area_m2: area,
            grid_threshold_m2: INDUSTRIAL_AREA_THRESHOLD_M2,
            cell_m: INDUSTRIAL_AREA_CELL_M,
            source_height_m: crate::constants::SOURCE_HEIGHT_LEISURE as f32,
            reach: PointReach::Fixed(max_radius_m),
            floors: 0,
            hub_height_m: None,
            rated_power_kw: None,
        },
        lw_day,
        lw_evening,
        lw_night,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapped z30 grid ring from `(lat, lon)` vertices.
    fn grid_ring(coords: &[(f64, f64)]) -> GridRing {
        coords
            .iter()
            .map(|&(lat, lon)| snap_lonlat(lon, lat))
            .collect()
    }

    #[test]
    fn emission_only_ground_has_no_floors_or_invented_facade() {
        for building_type in [1, 3, 7] {
            let prepare = |area_source, height_m, floors| {
                prepare_building_points(RawBuildingInput {
                    centroid_lat: 50.0,
                    centroid_lon: 14.0,
                    height_m,
                    floors,
                    area_source,
                    building_type,
                    area_m2: Some(1_000.0),
                    polygon_grid: &[],
                })
                .remove(0)
            };
            let ground = prepare(true, 0.0, 0);
            let tagged_ground = prepare(true, 24.0, 8);
            let building = prepare(false, 0.0, 0);
            let profile = settlement::building_profile(building_type);
            let expected_lw = settlement::building_lw(&profile, 1_000.0, 1);
            let bands = ground.lw_day.map(f64::from);
            let actual_lw = crate::propagation::iso9613::a_weighted_total(&bands);
            assert!((actual_lw - expected_lw).abs() < 1e-4);
            assert_eq!(ground.lw_day, tagged_ground.lw_day);
            assert_eq!(ground.source_height_m, tagged_ground.source_height_m);
            assert_eq!(ground.floors, 0);
            assert_eq!(
                ground.source_height_m,
                crate::constants::SOURCE_HEIGHT_LEISURE as f32
            );
            assert_eq!((building.floors, building.source_height_m), (3, 4.0));
        }
    }

    #[test]
    fn prepared_building_uses_radius_from_lw() {
        let points = prepare_building_points(RawBuildingInput {
            area_source: false,
            centroid_lat: 49.0,
            centroid_lon: 14.0,
            height_m: 12.0,
            floors: 4,
            building_type: 1,
            area_m2: Some(300.0),
            polygon_grid: &[],
        });
        assert_eq!(points.len(), 1);
        assert!(points[0].max_radius_m > 0.0);
    }

    /// The physical ceiling is applied after the complete height ladder, so
    /// explicit tag errors cannot lift a source into the stratosphere while
    /// valid skyscrapers and the largest representable floor count stay exact.
    #[test]
    fn prepared_building_clamps_resolved_height_to_physical_ceiling() {
        let source_height = |height_m, floors| {
            let points = prepare_building_points(RawBuildingInput {
                area_source: false,
                centroid_lat: 49.0,
                centroid_lon: 14.0,
                height_m,
                floors,
                building_type: 1,
                area_m2: Some(300.0),
                polygon_grid: &[],
            });
            assert_eq!(points.len(), 1);
            points[0].source_height_m
        };

        assert_eq!(source_height(31_231.0, 2), 414.0);
        // u8::MAX floors resolve to 765 m, which is honestly below the 828 m
        // ceiling and therefore remains 382.5 m at the facade midpoint.
        assert_eq!(source_height(0.0, u8::MAX), 382.5);
        assert!((source_height(827.8, 0) - 413.9).abs() < 1e-3);
        assert_eq!(source_height(300.0, 0), 150.0);
    }

    /// `prepare_building_points` must carry `floors` and `area_m2`
    /// through to `PreparedPoint` so the per-segment popup trace
    /// echoes what the engine actually used (else the popup shows
    /// "0 floors / 0 m²" even for fully-resolved buildings).
    #[test]
    fn prepared_building_carries_floors_and_area_to_segment_trace() {
        let points = prepare_building_points(RawBuildingInput {
            area_source: false,
            centroid_lat: 49.0,
            centroid_lon: 14.0,
            height_m: 12.0,
            floors: 4,
            building_type: 1,
            area_m2: Some(300.0),
            polygon_grid: &[],
        });
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].floors, 4, "OSM building:levels must propagate");
        assert!(
            (points[0].area_m2 - 300.0).abs() < 1e-3,
            "arrow area_m2 must propagate; got {}",
            points[0].area_m2,
        );
        // Wind-turbine fields stay None for ordinary buildings — they
        // belong to the wind-turbine branch of `prepare_industrial_points`.
        assert!(points[0].hub_height_m.is_none());
        assert!(points[0].rated_power_kw.is_none());
    }

    /// Wind-turbine branch (`source_type == 10`) must carry
    /// hub_height + rated_power through `PreparedPoint` so the popup
    /// `EmissionTrace::Industrial.{hub_height_m, rated_power_kw}`
    /// stops being hardcoded `None`.
    #[test]
    fn prepared_wind_turbine_carries_hub_and_power() {
        let points = prepare_industrial_points(RawIndustrialInput {
            centroid_lat: 49.0,
            centroid_lon: 14.0,
            source_type: 10,
            site_subtype: 0,
            nace_4digit: None,
            hub_height_m: Some(100.0),
            rated_power_kw: Some(3500.0),
            area_m2: None,
            polygon_grid: &[],
            plant_output_mw: None,
            substation_mva: None,
            substation_class: 0,
        });
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].hub_height_m, Some(100.0));
        assert_eq!(points[0].rated_power_kw, Some(3500.0));
        // Buildings-only fields stay 0 on turbines.
        assert_eq!(points[0].floors, 0);
        assert_eq!(points[0].area_m2, 0.0);
    }

    #[test]
    fn power_classes_use_per_mw_per_mva_physics_and_outlines_are_silent() {
        let prep = |source_type: u8, nace: Option<u16>, mw: Option<f64>, mva: Option<f64>, class: u8| {
            prepare_industrial_points(RawIndustrialInput {
                centroid_lat: 49.0,
                centroid_lon: 14.0,
                source_type,
                site_subtype: 0,
                nace_4digit: nace,
                hub_height_m: None,
                rated_power_kw: None,
                area_m2: Some(46_710.0), // Vienna airport farm footprint
                polygon_grid: &[],
                plant_output_mw: mw,
                substation_mva: mva,
                substation_class: class,
            })
        };
        let day_aw = |points: &[PreparedPoint]| {
            let day: [f64; NUM_BANDS] = std::array::from_fn(|i| points[0].lw_day[i] as f64);
            crate::propagation::iso9613::a_weighted_total(&day)
        };
        // Registry-confirmed solar (served 3599 rows): 24 MW → 96.8 day-only.
        let solar = prep(0, Some(3599), Some(24.0), None, 0);
        assert!((day_aw(&solar) - 96.8).abs() < 0.1, "solar {}", day_aw(&solar));
        assert_eq!(solar[0].source_height_m, 3.0);
        assert!((solar[0].lw_night[4] - solar[0].lw_day[4] + 50.0).abs() < 1e-3);
        // OSM solar class without a tag: area × 0.55 MW/ha.
        let untagged = prep(industrial::SOURCE_SOLAR_FARM, None, None, None, 0);
        let expected = 88.0 + 10.0 * (4.671 * 0.55f64).log10() - 5.0;
        assert!((day_aw(&untagged) - expected).abs() < 0.1);
        // Substation: joined MVA wins, else the class median (auto 160 MVA).
        let joined = prep(industrial::SOURCE_SUBSTATION, None, None, Some(50.0), 3);
        assert!((day_aw(&joined) - industrial::substation_lw(50.0)).abs() < 0.1);
        assert_eq!(joined[0].lw_day[4], joined[0].lw_night[4]); // 24/7
        assert_eq!(joined[0].source_height_m, 5.0);
        let median = prep(industrial::SOURCE_SUBSTATION, None, None, None, 2);
        assert!((day_aw(&median) - industrial::substation_lw(160.0)).abs() < 0.1);
        // Wind-plant outlines, inactive facilities and transformers emit
        // nothing, as do unknown future classes.
        for silent in [
            industrial::SOURCE_WIND_OUTLINE,
            industrial::SOURCE_INACTIVE,
            industrial::SOURCE_TRANSFORMER,
            16,
            u8::MAX,
        ] {
            assert!(
                prep(silent, None, None, None, 0).is_empty(),
                "class {silent} emits"
            );
        }
    }

    #[test]
    fn solar_generator_units_read_the_extractor_parsed_output() {
        // A `generator:source=solar` point row carries no plant tag; the
        // extractor parsed its `generator:output:electricity` into
        // `rated_power_kw`, which beats the area density.
        let points = prepare_industrial_points(RawIndustrialInput {
            centroid_lat: 49.0,
            centroid_lon: 14.0,
            source_type: industrial::SOURCE_SOLAR_FARM,
            site_subtype: 0,
            nace_4digit: None,
            hub_height_m: None,
            rated_power_kw: Some(5.0),
            area_m2: None,
            polygon_grid: &[],
            plant_output_mw: None,
            substation_mva: None,
            substation_class: 0,
        });
        let day: [f64; NUM_BANDS] = std::array::from_fn(|i| points[0].lw_day[i] as f64);
        let aw = crate::propagation::iso9613::a_weighted_total(&day);
        let expected = 88.0 + 10.0 * 0.005f64.log10() - 5.0;
        assert!((aw - expected).abs() < 0.1, "5 kW unit: {aw:.2}");
    }

    #[test]
    fn industrial_cull_radius_uses_each_points_post_split_loudest_day_band() {
        let ring = grid_ring(&[
            (50.000, 14.000),
            (50.000, 14.006),
            (50.004, 14.006),
            (50.004, 14.000),
            (50.000, 14.000),
        ]);
        let points = prepare_industrial_points(RawIndustrialInput {
            centroid_lat: 50.002,
            centroid_lon: 14.003,
            source_type: 0,
            site_subtype: 0,
            nace_4digit: None,
            hub_height_m: None,
            rated_power_kw: None,
            area_m2: Some(6_000.0),
            polygon_grid: &ring,
            plant_output_mw: None,
            substation_mva: None,
            substation_class: 0,
        });

        assert!(
            points.len() > 1,
            "test polygon must exercise area splitting"
        );
        for point in points {
            let expected = grid::geo::point_source_audibility_radius(
                loudest_day_band_db(&point.lw_day),
                crate::constants::INDUSTRIAL_MAX_RADIUS,
            );
            assert_eq!(point.max_radius_m, expected);
            assert!(
                point.max_radius_m < crate::constants::INDUSTRIAL_MAX_RADIUS,
                "fixture must exercise the solved reach rather than the cap"
            );
        }
    }

    /// Settlement v2 phase 1: the cull radius is the honest free-field
    /// audibility distance of the radiated lw — one scalar, no compensation
    /// (the pre-v2 pin 63.39 m guarded the W7 net-zero contract, now gone).
    #[test]
    fn building_cull_radius_matches_honest_lw() {
        let points = prepare_building_points(RawBuildingInput {
            area_source: false,
            centroid_lat: 49.0,
            centroid_lon: 14.0,
            height_m: 0.0,
            floors: 3,
            building_type: 0,
            area_m2: Some(200.0),
            polygon_grid: &[],
        });
        let p = settlement::building_profile(0);
        let expected = settlement::building_max_dist(settlement::building_lw(&p, 200.0, 3));
        assert!(
            (points[0].max_radius_m - expected).abs() < 1e-6,
            "cull radius {} != building_max_dist(lw) {}",
            points[0].max_radius_m,
            expected
        );
    }

    /// Audit I-10b wind-turbine input hygiene: hub default = 105 m
    /// (known-data median in our arrows), tag-error hubs clamp to 175 m,
    /// implausible rated power (>8 MW onshore) is treated as unknown.
    #[test]
    fn wind_turbine_hub_default_and_tag_error_clamps() {
        let prep = |hub: Option<f32>, power: Option<f32>| {
            prepare_industrial_points(RawIndustrialInput {
                centroid_lat: 49.0,
                centroid_lon: 14.0,
                source_type: 10,
                site_subtype: 0,
                nace_4digit: None,
                hub_height_m: hub,
                rated_power_kw: power,
                area_m2: None,
                polygon_grid: &[],
                plant_output_mw: None,
                substation_mva: None,
                substation_class: 0,
            })
        };
        // Missing hub → 105 m default, carried into source_height_m.
        let points = prep(None, Some(2000.0));
        assert_eq!(points[0].hub_height_m, Some(105.0));
        assert_eq!(points[0].source_height_m, 105.0);
        // Tag-error hub clamps to 175 m; plausible hubs pass through.
        assert_eq!(prep(Some(250.0), Some(2000.0))[0].hub_height_m, Some(175.0));
        assert_eq!(prep(Some(120.0), Some(2000.0))[0].hub_height_m, Some(120.0));
        // Implausible rated power = unknown → 2 MW default, and the emission
        // uses the 2 MW class (LwA 105), not the ≥5 MW class (106.5) — at the
        // annual operating level (max mode plus the wind duty), not max mode.
        let points = prep(None, Some(20_000.0));
        assert_eq!(points[0].rated_power_kw, Some(2000.0));
        let day_f64: [f64; NUM_BANDS] = std::array::from_fn(|i| points[0].lw_day[i] as f64);
        let aw = crate::propagation::iso9613::a_weighted_total(&day_f64);
        let expected = 105.0 + wind::wind_duty_db();
        assert!((aw - expected).abs() < 1e-3, "clamped-power turbine LwA: {aw}");
    }

    /// A leisure area source: 1.5 m height, Lw-derived reach, AREA scaling
    /// (unified with buildings), energy-conserving split over the area grid.
    #[test]
    fn prepared_leisure_padel_court_emits_at_15m_height() {
        let points = prepare_leisure_points(RawLeisureInput {
            centroid_lat: 50.0,
            centroid_lon: 14.0,
            sport: leisure::PADEL,
            area_m2: Some(200.0), // reference court footprint → centroid point
            polygon_grid: &[],
            formula: None,
            is_line: false,
        });
        assert_eq!(points.len(), 1);
        assert!((points[0].source_height_m - 1.5).abs() < 1e-6);
        assert!(points[0].max_radius_m > 0.0 && points[0].max_radius_m <= 2_000.0);
        assert_eq!(points[0].floors, 0);
        // Day A-sum equals the annualized padel anchor (~81 dB at the 200 m²
        // reference: active 90 − 9 dB season/duty).
        let day: [f64; NUM_BANDS] = std::array::from_fn(|i| points[0].lw_day[i] as f64);
        let aw = crate::propagation::iso9613::a_weighted_total(&day);
        assert!((aw - 81.0).abs() < 0.2, "padel day LwA: {aw}");
    }

    /// Formula classes spread their class-TOTAL Lw (geometry-independent)
    /// with industrial reach: a speedway point reaches past the 2 km leisure
    /// cap, day-only.
    #[test]
    fn prepared_leisure_speedway_reaches_past_2km() {
        let formula = leisure::motorsport_emission(leisure::MotorsportSubtype::Speedway).unwrap();
        let mk = |area: f64| {
            prepare_leisure_points(RawLeisureInput {
                centroid_lat: 50.0,
                centroid_lon: 14.0,
                sport: leisure::MOTORSPORT,
                area_m2: Some(area),
                polygon_grid: &[],
                formula: Some(formula),
                is_line: false,
            })
        };
        let points = mk(20_000.0);
        assert_eq!(points.len(), 1);
        let day: [f64; NUM_BANDS] = std::array::from_fn(|i| points[0].lw_day[i] as f64);
        let aw = crate::propagation::iso9613::a_weighted_total(&day);
        // 139 + 10·lg4 + 10·lg(600/4380) = 136.4, whatever the footprint.
        assert!((aw - 136.4).abs() < 0.1, "speedway day LwA: {aw}");
        assert!((mk(5_000.0)[0].lw_day[4] - points[0].lw_day[4]).abs() < 1e-3);
        assert_eq!(points[0].max_radius_m, crate::constants::INDUSTRIAL_MAX_RADIUS);
        assert!((points[0].lw_night[4] - points[0].lw_day[4] + 50.0).abs() < 1e-3);
    }

    /// A raceway line spreads the formula total over its segments
    /// (energy-conserving): a 300 m two-node kart line becomes four ~75 m
    /// cells; a short line is one midpoint point.
    #[test]
    fn prepared_leisure_raceway_line_splits_energy_over_segments() {
        let formula = leisure::motorsport_emission(leisure::MotorsportSubtype::Kart).unwrap();
        let line = grid_ring(&[(50.0, 14.0), (50.0, 14.004)]); // ~286 m
        let points = prepare_leisure_points(RawLeisureInput {
            centroid_lat: 50.0,
            centroid_lon: 14.002,
            sport: leisure::MOTORSPORT,
            area_m2: None,
            polygon_grid: &line,
            formula: Some(formula),
            is_line: true,
        });
        assert_eq!(points.len(), 4);
        let energy: f64 = points
            .iter()
            .map(|p| 10f64.powf(f64::from(p.lw_day[4]) / 10.0))
            .sum();
        let total = 10f64.powf(leisure::leisure_formula_bands(&formula)[4] / 10.0);
        assert!((10.0 * (energy / total).log10()).abs() < 1e-3, "splits {energy} vs {total}");
        for point in &points {
            assert!((point.lat - 50.0).abs() < 1e-4);
            assert!(point.exclusion_radius_m > 30.0 && point.exclusion_radius_m < 45.0);
        }
        // A two-node line shorter than one cell: one midpoint point.
        let short = grid_ring(&[(50.0, 14.0), (50.0, 14.0005)]); // ~36 m
        let points = prepare_leisure_points(RawLeisureInput {
            centroid_lat: 50.0,
            centroid_lon: 14.00025,
            sport: leisure::MOTORSPORT,
            area_m2: None,
            polygon_grid: &short,
            formula: Some(formula),
            is_line: true,
        });
        assert_eq!(points.len(), 1);
        assert!((points[0].lon - 14.00025).abs() < 1e-6);
    }

    /// An area-law line (an open non-motorised track) emits as a node of its
    /// class — the open chain contributes no area.
    #[test]
    fn prepared_leisure_area_law_line_emits_as_a_node() {
        let line = grid_ring(&[(50.0, 14.0), (50.001, 14.001)]);
        let mk = |is_line: bool| {
            prepare_leisure_points(RawLeisureInput {
                centroid_lat: 50.0005,
                centroid_lon: 14.0005,
                sport: leisure::PITCH,
                area_m2: None,
                polygon_grid: &line,
                formula: None,
                is_line,
            })
        };
        let as_line = mk(true);
        let as_node = prepare_leisure_points(RawLeisureInput {
            centroid_lat: 50.0005,
            centroid_lon: 14.0005,
            sport: leisure::PITCH,
            area_m2: None,
            polygon_grid: &[],
            formula: None,
            is_line: false,
        });
        assert_eq!(as_line.len(), 1);
        assert_eq!(as_line[0].lw_day, as_node[0].lw_day);
    }

    /// Unified AREA scaling (replaces the old per-seat capacity build-up): a
    /// leisure source scales with its polygon area — 800 m² vs 200 m² = +6 dB.
    #[test]
    fn prepared_leisure_outdoor_seating_scales_area() {
        let mk = |area: f64| {
            prepare_leisure_points(RawLeisureInput {
                centroid_lat: 50.0,
                centroid_lon: 14.0,
                sport: leisure::OUTDOOR_SEATING,
                area_m2: Some(area),
                polygon_grid: &[],
                formula: None,
                is_line: false,
            })
        };
        let small = mk(200.0);
        let big = mk(800.0);
        let aw = |p: &PreparedPoint| {
            let d: [f64; NUM_BANDS] = std::array::from_fn(|i| p.lw_day[i] as f64);
            crate::propagation::iso9613::a_weighted_total(&d)
        };
        // 800 vs 200 m² = ×4 area = +6 dB (10·log10(4)).
        assert!((aw(&big[0]) - aw(&small[0]) - 6.0206).abs() < 1e-2);
    }
}
