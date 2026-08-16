//! Input types — the `Receiver` point (lat/lon/elevation/height) and altitude
//! helpers shared by every compute kernel.
use super::*;

/// Receiver point where noise is computed.
#[derive(Debug, Clone)]
pub struct Receiver {
    pub lat: f64,
    pub lon: f64,
    pub elevation_m: f64, // ground elevation from DEM
    pub height_m: f64,    // receiver height above ground (4.0m = END facade standard)
}

#[inline]
pub fn receiver_altitude_m(ground_elevation_m: f64, receiver_height_m: f64) -> f64 {
    ground_elevation_m + receiver_height_m
}

#[inline]
pub fn default_receiver_altitude_m(ground_elevation_m: f64) -> f64 {
    receiver_altitude_m(
        ground_elevation_m,
        crate::constants::DEFAULT_RECEIVER_HEIGHT,
    )
}

impl Receiver {
    pub fn new(lat: f64, lon: f64, elevation_m: f64) -> Self {
        Receiver {
            lat,
            lon,
            elevation_m,
            height_m: crate::constants::DEFAULT_RECEIVER_HEIGHT,
        }
    }

    /// Absolute altitude of receiver (ground + height).
    pub fn altitude_m(&self) -> f64 {
        receiver_altitude_m(self.elevation_m, self.height_m)
    }
}

#[cfg(test)]
mod tests {
    use super::{default_receiver_altitude_m, receiver_altitude_m, Receiver};

    #[test]
    fn receiver_altitude_helpers_match_receiver_struct() {
        let receiver = Receiver::new(50.0, 14.0, 123.5);
        assert_eq!(receiver.altitude_m(), default_receiver_altitude_m(123.5));
        assert_eq!(
            receiver.altitude_m(),
            receiver_altitude_m(123.5, receiver.height_m)
        );
        assert_eq!(receiver.altitude_m(), 127.5);
    }
}

/// Road microsegment (≤250m vertex pair) with pre-joined traffic.
#[derive(Debug, Clone)]
pub struct RoadSegment {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub road_class: u8, // 0=motorway..6=living_street, 7=service, 8=track, 9=unclassified, 10=motorway_link, 11=trunk_link, 12=primary_link
    pub speed_limit: u8, // km/h, 0=use default, 255=derestricted (maxspeed=none) → DERESTRICTED_SPEED_KMH
    // R7 taper: graded EFFECTIVE speed at a junction-free step (km/h, 0=none).
    // Own column so the OSM legal tag stays untouched and provenance stays
    // per-meaning: consulted only when speed_limit is 0, never a posted limit
    // (popup labels it "graded_transition"). Written by enrich-roads-taper.ts;
    // arrows without the column read as 0.
    pub speed_taper: u8,
    pub surface_type: u8, // 0=asphalt..4=gravel
    pub oneway: bool,
    pub lanes: u8,
    pub aadt_light: i32, // pre-joined traffic input (0=use defaults)
    pub aadt_medium: i32,
    pub aadt_heavy: i32,
    pub aadt_moto: i32,
    pub source_id: u16, // single source-of-truth stamp — see pipeline/lib/sources.ts
    pub name: String,   // OSM name tag (street/road name)
    pub road_ref: String, // OSM ref tag (D1, E55, I/35)
    pub bridge: bool,   // road on bridge/viaduct
    pub tunnel: bool,   // road in tunnel
    pub access: u8, // 0=default, 1=private, 2=no, 3=destination, 4=motor_vehicle=no (legacy), 5=permissive, 6=customers, 7=agricultural, 8=forestry
    pub junction: u8, // 0=default, 1=roundabout
    pub built_up: u8, // building-raster flag for untagged-speed legal defaults: 0=unknown, 1=rural, 2=urban
    // Pre-computed by source-reader:
    pub dist_m: f64, // horizontal distance to receiver
    pub cp_lat: f64, // closest point on segment
    pub cp_lon: f64,
    pub fraction: f64, // 0-1 position along segment
}

/// Railway microsegment (≤250m) with pre-joined traffic.
#[derive(Debug, Clone)]
pub struct RailSegment {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub rail_type: u8, // 0=rail, 1=tram, 2=light_rail, 3=narrow_gauge, 4=funicular
    pub usage: u8,     // 0=main, 1=branch, 2=industrial
    pub maxspeed: u16, // km/h (raw OSM value, 0 = none); u16 so 300+ km/h survives
    pub trains_passenger: f64, // effective daily count (post service/divisor scaling)
    pub trains_freight: f64, // effective daily count (post service/divisor scaling)
    pub speed_kmh: f64, // effective speed used by emission (resolved); f64 not u8 — high-speed rail resolves to 300 km/h, which u8 saturated to 255 (~1.4 dB too quiet)
    pub track_count: u8,
    pub name: String,     // OSM name tag (line name)
    pub rail_ref: String, // OSM ref tag (track number: "250", "340")
    pub bridge: bool,     // railway on bridge/viaduct → G=0
    pub tunnel: bool,     // railway in tunnel → skip (no outdoor noise)
    // Metadata preserved for popup display (normally zero/false for pipeline path):
    pub service: bool,               // service/yard track (2% of main-line traffic)
    pub highspeed: bool,             // high-speed rail flag
    pub parallel_divisor: u8,        // >1 = track was mapped as parallel OSM ways, divide traffic
    pub speed_source: u8,            // 0=osm_maxspeed, 1=highspeed_default, 2=type_default
    pub trains_passenger_source: u8, // 0=arrow, 1=default_by_type
    pub trains_freight_source: u8,   // 0=arrow, 1=default_by_type
    pub source_id: u16,              // single source-of-truth stamp — see pipeline/lib/sources.ts
    // Pre-computed:
    pub dist_m: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub fraction: f64,
}

/// Pre-discretized point source (building facade point, industrial grid point, wind turbine).
#[derive(Debug, Clone)]
pub struct PointSource {
    pub osm_id: i64,
    pub lat: f64,
    pub lon: f64,
    pub source_height_m: f32,
    pub source_type: u8, // building_type or industrial site_type or wind_turbine
    pub lw_day: [f32; NUM_BANDS], // emission bands day
    pub lw_evening: [f32; NUM_BANDS], // emission bands evening
    pub lw_night: [f32; NUM_BANDS], // emission bands night
    pub n_points: u16,   // total discretization points (for energy splitting: Lw - 10·log₁₀(N))
    pub name: String,    // OSM name or addr:street + housenumber
    pub polygon_wkb: String, // WKB hex for building polygon (empty if unavailable)
    // Self-screening exclusion radius: R = √(area/π). Screening within R of source
    // is the source's own footprint, not a real barrier (ISO 9613-2).
    pub exclusion_radius_m: f32,
    // Source audibility / fade-out radius. Buildings derive it from parent Lw;
    // industrial points derive it from their post-split loudest day band.
    pub max_radius_m: f64,
    pub source_id: u16, // single source-of-truth stamp — see pipeline/lib/sources.ts
    /// Building OSM tag `building:levels` (0 = absent → engine
    /// fell back to ceil(height / BUILDING_FLOOR_HEIGHT_M)). Carried through so the popup
    /// can display the canonical floor count alongside the
    /// derived `source_height_m`. 0 for industrial / wind turbines.
    pub floors: u8,
    /// Building footprint area (m²) computed at extract time from
    /// `polygon_wkb` via the cos(lat) Shoelace formula. 0 for
    /// industrial point sources without polygon coverage. Drives
    /// the Lw GFA-scaling per `settlement::building_lw` — when 0
    /// the engine falls back to 100 m².
    pub area_m2: f32,
    /// Wind turbine hub height (m). `None` for buildings + ordinary
    /// industrial sites. Carried so the popup
    /// `EmissionTrace::Industrial.hub_height_m` matches what the
    /// engine used for `source_height_m` on the turbine branch.
    pub hub_height_m: Option<f32>,
    /// Wind turbine rated power (kW). `None` outside the wind-
    /// turbine branch.
    pub rated_power_kw: Option<f32>,
    // Pre-computed:
    pub dist_m: f64,
}

/// Aircraft microsegment (Doc 29 format).
///
/// `flight_id` is the primary identity (real packed (icao24, ts32)
/// for ground / airborne, synth bucket id for cruise). v14 dropped
/// `cruise_flight_ids` — cruise dedup moved to per-row
/// `top_candidates` consumption in `compute/aircraft_v6/cruise.rs`.
#[derive(Debug, Clone)]
pub struct AircraftSegment {
    pub flight_id: u64,
    pub profile_idx: u8,
    pub is_departure: bool,
    pub on_ground: bool,
    pub period: u8, // 0=day, 1=evening, 2=night
    pub date_id: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub start_alt_m: f32,
    pub end_lat: f64,
    pub end_lon: f64,
    pub end_alt_m: f32,
    pub speed_kt: f32,
    pub segment_length_m: f32,
    pub count_weight: f32, // 1.0 = one observed flight segment; >1 = synthetic aggregated operations
    pub surface_model: bool, // synthetic airport-surface model contribution
    pub ground_context: u8, // 0=none, 1=airport_line
    pub ground_ops_kind: u8, // 0=none, 1=runway_roll, 2=taxi, 3=apron_movement
    pub source_id: u16, // ADS-B observational data — always 1 (adsb-planet); see pipeline/lib/sources.ts
}

/// Prepared airport area geometry from OSM aeroway/amenity data. The
/// v10 ground extractor consults `centroid_lat/lon` + `area_m2` for the
/// nearest-aerodrome lookup; the WKB stays as a passive byte string.
#[derive(Debug, Clone)]
pub struct AirportArea {
    pub osm_id: i64,
    pub aeroway_type: u8,
    pub name: String,
    pub airport_key: String,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub polygon_wkb: String,
    pub area_m2: f32,
}

impl AirportArea {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        osm_id: i64,
        aeroway_type: u8,
        name: String,
        airport_key: String,
        centroid_lat: f64,
        centroid_lon: f64,
        polygon_wkb: String,
        area_m2: f32,
    ) -> Self {
        Self {
            osm_id,
            aeroway_type,
            name,
            airport_key,
            centroid_lat,
            centroid_lon,
            polygon_wkb,
            area_m2,
        }
    }
}

/// Noise barrier microsegment — the wall polyline element as stored in
/// `barriers.arrow`, endpoints and all.
///
/// The endpoints are the screening geometry: `path_effects` intersects the
/// source→receiver ray with THIS segment (exact 2D ray×segment, the same
/// primitive the vector obstacle index uses for building edges) and turns the
/// hit into a dominant-edge candidate at its exact chainage. The midpoint is
/// only a proximity key — [`Self::midpoint`] derives it, so it can never
/// disagree with the geometry.
///
/// Slice contract (consumed by `path_effects::screening_attenuation[_with_meta]`):
/// the slice MUST be sorted ascending by `dist_m`, and `dist_m` MUST be a
/// LOWER BOUND on the true receiver→midpoint distance — the screening loop
/// early-breaks at the first barrier with
/// `dist_m > path_len + BARRIER_PATH_HORIZON_M` and a violated bound would
/// silently drop barriers that are actually on the path.
///
/// * Popup (`source-reader::query`): `dist_m` is the exact receiver→midpoint
///   distance (one receiver per query), sorted after the per-hex merge.
/// * Heatmap (`tile-painter::source_loader_barrier::BarrierData::for_tile`):
///   one slice serves every receiver pixel of a z13 tile, so
///   `dist_m = max(0, d(midpoint, tile_centre) − tile_half_diagonal)` — by the
///   triangle inequality a lower bound for EVERY pixel in the tile, keeping
///   the early-break conservative (it only ever scans more barriers, never
///   fewer, than the popup would for the same receiver).
#[derive(Debug, Clone, Copy)]
pub struct Barrier {
    pub osm_id: i64,
    /// Stable microsegment identity within the OSM element.
    pub segment_idx: i16,
    /// Height above local ground (m); extract defaults untagged walls to 3.0.
    pub height_m: f32,
    /// Segment endpoints, verbatim from `barriers.arrow`.
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    /// Lower bound on the receiver→midpoint distance (see struct docs).
    pub dist_m: f64,
}

impl Barrier {
    /// The segment midpoint — what `dist_m` is measured to, and the point the
    /// loaders filter and sort on.
    #[inline]
    pub fn midpoint(&self) -> (f64, f64) {
        (
            (self.start_lat + self.end_lat) * 0.5,
            (self.start_lon + self.end_lon) * 0.5,
        )
    }
}

/// Half the longest barrier microsegment `osm-extract` can emit: every linear
/// feature is split at 250 m (`microsegment::split(&coords, 250.0)`), so a wall
/// that CROSSES a source→receiver path carries its midpoint at most this far
/// past the crossing point.
pub const BARRIER_SEGMENT_MAX_HALF_LEN_M: f64 = 125.0;

/// How far past a path's own length the screening loop keeps scanning the
/// (ascending-`dist_m`) barrier slice before it early-breaks, and the matching
/// slack in `BarrierData::for_tile`'s reach filter.
///
/// A crossing point lies ON the path, hence within `path_len` of the receiver;
/// the crossing barrier's midpoint is at most
/// [`BARRIER_SEGMENT_MAX_HALF_LEN_M`] further, plus 50 m for the flat-earth
/// scale mismatch between the loaders' `geo::flat_dist` (pair mid-latitude) and
/// the kernel's ray frame (path mid-latitude). Exceeding this bound is what
/// silently drops a real crossing, so it is a correctness constant, not a tuning
/// knob — the mirrored CUDA literal in `scatter.cu` must move with it.
pub const BARRIER_PATH_HORIZON_M: f64 = BARRIER_SEGMENT_MAX_HALF_LEN_M + 50.0;
