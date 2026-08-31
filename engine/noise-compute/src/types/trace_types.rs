//! Per-segment trace types (popup-only) — per-period triples and the
//! propagation-variant Lden breakdown shown in the "what if this effect were off" table.
use super::*;

// Per-segment traces populated only when the popup requests them; pipeline
// passes `None` containers to `compute_*` so the hot path branch-predicts out.

/// Day/evening/night triple for per-period values (emission or received).
#[derive(Debug, Clone, Serialize)]
pub struct PerPeriod<T> {
    pub day: T,
    pub evening: T,
    pub night: T,
}

/// Lden under each propagation-variant hypothesis — one number per column in
/// the popup's "what if this effect were off" comparison. `no_ground` is the
/// only SIGNED delta — over soft ground at 63/125 Hz the ground term can boost
/// LF (CF[i] < 0), so `no_ground < full` is possible.
#[derive(Debug, Clone, Serialize)]
pub struct LdenVariants {
    pub full: f64,
    /// Divergence + atmospheric + ground only, no path effects.
    pub free_field: f64,
    pub no_terrain: f64,
    pub no_screening: f64,
    pub no_vegetation: f64,
    pub no_ground: f64,
    pub no_atmospheric: f64,
}

/// One contiguous forested interval along a source→receiver path.
#[derive(Debug, Clone, Serialize)]
pub struct ForestRun {
    pub t_start: f64,
    pub t_end: f64,
    /// DENSITY-WEIGHTED depth in metres (`Σ Δlen × forest/100`, geodata-v2
    /// 2a) — equals the physical extent on binary rasters; geometry lives
    /// in `t_start`/`t_end`.
    pub len_m: f64,
}

/// Sampled path profile from source→receiver — mirror of
/// [`crate::propagation::PathProfile`] minus the scratch buffer.
#[derive(Debug, Clone, Serialize)]
pub struct PathProfileTrace {
    pub t: Vec<f64>,
    pub elevation_m: Vec<f32>,
    pub forest_u8: Vec<u8>,
    pub imd_u8: Vec<u8>,
    pub dist_m: f64,
    pub step_m_med: f32,
    pub src_lat: f64,
    pub src_lon: f64,
    pub rcv_lat: f64,
    pub rcv_lon: f64,
    pub src_alt_m: f64,
    pub rcv_alt_m: f64,
}

/// One diffraction edge found on the bare-earth elevation profile.
/// `elevation_m` is the DEM value at that sample (in metres ASL).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct EdgePoint {
    pub t: f64,
    pub elevation_m: f64,
}

/// Per-segment terrain diffraction trace (engine's `path_effects` output).
/// Scalar summary lives on [`Contributor::terrain_impact_db`] as A-weighted ΔL_A.
///
/// Single-edge model (SPEC §4.6): the algorithm picks ONE
/// edge — the candidate with the largest path-length difference δ — so
/// `n_edges` is 0 or 1 and `edges` holds at most that dominant edge.
#[derive(Debug, Clone, Serialize)]
pub struct TerrainTrace {
    pub delta_m: f64,
    pub attenuation_bands: [f64; NUM_BANDS],
    /// Edge vertices — empty (clear path) or exactly the dominant edge.
    pub edges: Vec<EdgePoint>,
    /// CNOSSOS §2.5.6(c) Rayleigh δ\* — path difference over the dominant
    /// edge with mirror source/receiver reflected across OLS-fitted per-side
    /// mean ground planes. Zero when there is no obstruction. Used as
    /// `δ ≤ λ/4 − δ*` per-band gate.
    pub delta_star_m: f64,
}

impl TerrainTrace {
    /// δ of the bare-earth dominant edge, or `None` when the path is clear.
    ///
    /// [`screening_attenuation`](crate::propagation::path_effects::screening_attenuation)
    /// races vector obstacle candidates against this δ. `edges` holds exactly the
    /// dominant edge or nothing, so its emptiness IS the "no terrain edge" answer.
    ///
    /// `None` here means exactly what the deleted composite pass's `None` meant:
    /// `compute_terrain_diffraction` runs `single_edge_atten` over the same
    /// clamped scratch and propagates its `None` through `diff?`, and that
    /// `None` is `max_delta_idx` finding no sample above the line of sight —
    /// the same test, on the same numbers. A candidate that used to have to beat
    /// a below-line edge still does; one that faced no edge still faces none.
    pub fn dominant_delta_m(&self) -> Option<f64> {
        (!self.edges.is_empty()).then_some(self.delta_m)
    }
}

/// Per-segment building / barrier screening trace. Scalar summary lives on
/// [`Contributor::screening_impact_db`] as A-weighted ΔL_A.
#[derive(Debug, Clone, Serialize)]
pub struct ScreeningTrace {
    pub attenuation_bands: [f64; NUM_BANDS],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obstacle: Option<ScreeningObstacleTrace>,
}

/// Per-segment vegetation trace (forest depth + run intervals). Scalar summary
/// lives on [`Contributor::vegetation_impact_db`] as A-weighted ΔL_A.
#[derive(Debug, Clone, Serialize)]
pub struct VegetationTrace {
    pub forest_depth_m: f64,
    pub sampled_path_m: f64,
    pub attenuation_bands: [f64; NUM_BANDS],
    pub forest_runs: Vec<ForestRun>,
}

/// Per-segment ground-effect trace. `attenuation_bands` can be negative at
/// low frequencies over soft ground — standard CNOSSOS behaviour. Scalar
/// summary lives on [`Contributor::ground_impact_db`] (signed — soft ground
/// at 63/125 Hz can boost LF energy).
#[derive(Debug, Clone, Serialize)]
pub struct GroundTrace {
    pub factor_g: f64,
    pub attenuation_bands: [f64; NUM_BANDS],
}

/// Per-segment baseline terms the engine computes alongside propagation.
/// Carries divergence, atmospheric spectrum, ground G, source height, FLC,
/// and the per-receiver reflection boost. Note: the internal `free_field`
/// propagation variant (iso9613.rs) covers divergence + atmospheric +
/// ground + FLC only — reflection is added to `full` but NOT to
/// `free_field`, so `full − free_field` includes A_refl whenever it is
/// non-zero. Obstruction attenuations (`A_bar`, `A_fol`) live separately
/// on `TerrainTrace` / `ScreeningTrace` / `VegetationTrace`.
#[derive(Debug, Clone, Serialize)]
pub struct BaselineTrace {
    pub geometric_db: f64,
    pub atmospheric_bands: [f64; NUM_BANDS],
    pub ground_factor_g: f64,
    pub source_height_m: f64,
    pub finite_line_corr_db: f64,
    /// Urban reflection boost (ISO 9613-2 §7.5 / CNOSSOS-EU §2.5.18) —
    /// per-receiver A_refl from local building enclosure, 0/1.5/3 dB.
    /// Same scalar for every segment at this receiver.
    pub reflection_boost_db: f64,
}

/// Per-segment propagation breakdown — model-specific structure for the
/// noise-model that emitted this segment.
///
/// CNOSSOS-EU / ISO 9613-2 surface sources (road / railway / aircraft
/// ground ops / building / industrial) use [`Cnossos`]: per-band Lw, terrain
/// shielding, building screening, vegetation, ground attenuation. ICAO
/// Doc 29 4th Ed. aircraft airborne sub-segments + cruise hexes use
/// [`Doc29`]: Eq. 4-8b decomposition (sel_npd + ΔV + ΔI − Λ + ΔF) — Doc 29
/// does NOT model terrain shielding, building screening, vegetation, or
/// octave bands; the previous unified struct populated those slots with
/// zeros for every aircraft trace (~690 B JSON noise per trace).
///
/// `Box<CnossosBreakdown>` keeps `size_of::<PropagationBreakdown>()` small
/// (`size_of::<Doc29Breakdown>() + tag = ~120 B`) — without the box the
/// enum size = largest variant (~600 B), allocating that on every aircraft
/// trace would defeat the purpose of the split.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "model", rename_all = "snake_case")]
pub enum PropagationBreakdown {
    Cnossos(Box<CnossosBreakdown>),
    Doc29(Doc29Breakdown),
}

/// Surface-source (road / rail / ground-ops / building / industrial)
/// propagation breakdown — per ISO 9613-2 + CNOSSOS-EU.
#[derive(Debug, Clone, Serialize)]
pub struct CnossosBreakdown {
    pub baseline: BaselineTrace,
    pub path_profile: PathProfileTrace,
    pub terrain: TerrainTrace,
    pub screening: ScreeningTrace,
    pub vegetation: VegetationTrace,
    pub ground: GroundTrace,
    pub lw_bands: PerPeriod<[f64; NUM_BANDS]>,
    pub lw_db_a: PerPeriod<f64>,
    pub received_bands: PerPeriod<[f64; NUM_BANDS]>,
}

/// Aircraft (airborne sub-segment + cruise hex) propagation breakdown —
/// per ICAO Doc 29 4th Ed. Eq. 4-8b:
///
/// ```text
/// SEL_seg = sel_npd_db + delta_v_db + delta_i_db − lambda_db + delta_f_db
/// ```
///
/// On the CFFK fast path (slant > 7.62 km), `lambda_db` and `delta_i_db`
/// collapse to 0.0 per Doc 29 §A.2.7 (lateral attenuation and installation
/// directivity become negligible at long slant).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Doc29Breakdown {
    pub sel_npd_db: f64,
    pub delta_v_db: f64,
    pub delta_i_db: f64,
    pub lambda_db: f64,
    pub delta_f_db: f64,
    pub d_p_m: f64,
    pub lateral_m: f64,
    /// Elevation angle (β) in degrees — receiver-to-source aspect for
    /// lateral attenuation. CFFK fast path sets this to 90.0 sentinel
    /// (lambda not used at long slant).
    pub beta_deg: f64,
    pub seg_len_m: f64,
    pub d_bar_m: f64,
    /// `wing` | `fuselage` | `propeller` — engine installation per Doc 29 §A.3.
    pub installation: &'static str,
    pub cffk_fast_path: bool,
}

/// Kind-specific emission inputs for trace visibility. Mirrors the raw inputs
/// the engine handed to `emission::*::compute` for the segment — popup UI
/// renders these verbatim so users can reconstruct the Lw calculation.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmissionTrace {
    Road {
        aadt_light: f64,
        aadt_medium: f64,
        aadt_heavy: f64,
        aadt_moto: f64,
        speed_kmh: f64,
        surface_corr_db: f64,
        surface: &'static str, // "asphalt" | "paving" | "concrete" | "unpaved" | "gravel"
        traffic_source: &'static str, // "matched_external" | "estimated_service_tree" | "default_by_class"
        source_id: u16,
        /// Dataset attribution for the popup display. Resolved from
        /// `source_id` in the Rust builders (was previously a Node-side
        /// lookup but the field-rename `dataset_id` → `source_id` left
        /// the enrichment a silent no-op).
        #[serde(skip_serializing_if = "Option::is_none")]
        provenance: Option<crate::sources::DatasetMeta>,
        road_class: &'static str,
        bridge: bool,
        tunnel: bool,
        oneway: bool,
        lanes: u8,
    },
    Railway {
        trains_passenger: f64,
        trains_freight: f64,
        trains_passenger_source: &'static str, // "arrow" | "default_by_type"
        trains_freight_source: &'static str,
        source_id: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        provenance: Option<crate::sources::DatasetMeta>,
        speed_kmh: f64,
        bridge: bool,
        highspeed: bool,
        rail_type: &'static str,
        service: bool,
    },
    AircraftGround {
        class: &'static str, // "runway" | "taxi" | "apron"
        observed_movements: f64,
        modeled_movements: f64,
        /// Per-microsegment unique-rotation counts derived from
        /// the row's `microseg_unique_*` scalars (Stage 2C v5 — UNION
        /// across all rows of this microsegment, row-replicated).
        /// Three disjoint slices: `arr` = runway-roll arrivals,
        /// `dep` = runway-roll departures, `gse` = ground-support
        /// equipment movements per GSE class
        /// (LIGHT/MEDIUM/HEAVY), divided by `n_days`.
        arrivals_per_day: f64,
        departures_per_day: f64,
        gse_per_day: [f64; 3],
        /// Top aircraft classes by energy share AT THIS microsegment
        /// (not airport-aggregate). Lets the popup show "this taxi
        /// is dominated by A320s" vs "this apron mostly sees props".
        class_mix: Vec<ProfileMixEntry>,
        /// OSM aeroway `ref` tag (e.g. "06/24" for runway "06/24",
        /// "A" for taxiway A). `None` for synth strips and OSM ways
        /// without a `ref` tag.
        osm_ref: Option<String>,
    },
    /// One sub-segment of a Doc 29 SEL airborne computation (the popup's
    /// actual compute unit). `class` is the noise-class name of the
    /// flight's `profile_idx`; the packed `flight_id` is opaque to the
    /// frontend, so we surface its decoded `icao_hex` + `start_unix`.
    AircraftAirborne {
        class: &'static str,
        callsign: String,
        aircraft_type: String,
        cpa_distance_m: f64,
        altitude_m_at_cpa: f64,
        /// Set from Stage 2A sub-segment `flags & 0b001`
        /// (`classify_is_departure_per_sample`, smoothed ROCD median
        /// with +500 fpm threshold). Departure vs arrival is a real
        /// observable here, not a guess.
        is_departure: bool,
        /// Empty for synthetic (anonymous-transponder) fids.
        icao_hex: String,
        /// `None` for synthetic fids; frontend converts to YYYY-MM-DD
        /// for the globe.adsb.lol trace deep-link.
        start_unix: Option<u32>,
    },
    /// One R7 hex's cruise-bucket aggregate (per FL × class × period
    /// breakdown lives in `SegmentTrace.cruise_buckets`).
    AircraftCruise {
        r7_hex: String, // h3o lowercase string
        n_unique_flights: u32,
        rep_alt_m: f32,
    },
    Building {
        building_type: &'static str,
        height_m: f32,
        floors: u8,
        area_m2: f64,
    },
    Industrial {
        source_type: &'static str,
        area_m2: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        nace: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hub_height_m: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rated_power_kw: Option<f32>,
        effective_area_source_dist_m: f64,
    },
}

/// Full per-segment trace for an ISO 9613-2 source (road / rail / aircraft
/// ground ops / building / industrial) plus the aircraft sub-types.
/// One trace per contributing segment — every segment has its own trace,
/// no dB threshold. The popup renders one row per trace.
///
/// Aircraft-specific extensions: `aircraft_subtype` discriminates
/// 1 = ground path, 2 = airborne sub-segment, 3 = cruise R7 hex
/// (0 = non-aircraft / unset). `polyline` carries the ADS-B trajectory
/// for ground paths (`MultiLineString`-friendly); `hex_polygon` carries
/// the closed boundary ring for cruise hexes; `cruise_buckets` carries
/// the per-bucket breakdown inside a cruise hex; `length_m_per_kind`
/// carries `[runway, taxi, apron]` lengths for ground paths.
#[derive(Debug, Clone, Serialize)]
pub struct SegmentTrace {
    pub kind: LayerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub osm_id: Option<i64>,
    pub segment_idx: i16,
    pub name: String,
    pub subtype: String,
    /// True when this segment was the dominant contributor inside its grouped
    /// Contributor (highest received energy). One-per-group.
    pub is_dominant_of_group: bool,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub length_m: f64,
    pub dist_m: f64,
    pub d_slant_m: f64,
    pub bridge: bool,
    pub tunnel: bool,
    pub emission: EmissionTrace,
    /// Tagged-enum propagation breakdown. `Cnossos` for surface sources
    /// (road / rail / aircraft ground ops / building / industrial) —
    /// per-band Lw, terrain shielding, building screening, vegetation,
    /// ground attenuation per ISO 9613-2. `Doc29` for aircraft airborne
    /// sub-segments + cruise hexes — Eq. 4-8b decomposition
    /// (sel_npd + ΔV + ΔI − Λ + ΔF). Doc 29 does not model terrain
    /// shielding / building screening / vegetation / octave bands; the
    /// previous always-zero CNOSSOS fields on aircraft traces were
    /// ~690 B JSON noise per trace × millions of traces / popup.
    pub propagation: PropagationBreakdown,
    /// Receiver-side per-segment Lden contribution. Same semantics
    /// across all `aircraft_subtype` values and non-aircraft layers:
    /// the `full` slot is the segment's contribution to receiver Lden
    /// (energy-summing the displayed segments approaches the source
    /// aggregate, modulo top-K cap drift). `free_field` /
    /// `no_terrain` / `no_screening` / `no_vegetation` carry path-effect
    /// breakdowns for road / rail / building / industrial / aircraft
    /// ground (subtype 1); they're zero for airborne (subtype 2) and
    /// cruise (subtype 3) where we don't track per-band path effects.
    pub received_lden: LdenVariants,
    /// 0 = non-aircraft / unset, 1 = aircraft ground path, 2 = aircraft
    /// airborne sub-segment, 3 = aircraft cruise R7 hex. Allows the
    /// frontend to dispatch geometry rendering without inspecting `kind`
    /// + sniffing other fields.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub aircraft_subtype: u8,
    /// Polyline of `(lat, lon)` pairs for ground paths. `None` for
    /// airborne sub-segments and cruise hexes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polyline: Option<Vec<(f64, f64)>>,
    /// Closed polygon ring (last vertex = first vertex) for cruise R7
    /// hexes. `None` for ground paths and airborne sub-segments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex_polygon: Option<Vec<(f64, f64)>>,
    /// Per-bucket breakdown of a cruise R7 hex (FL × class × period).
    /// Sorted by `received_lden` desc. `None` for ground / airborne traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cruise_buckets: Option<Vec<CruiseBucketBreakdown>>,
    /// Top contributing flights inside a cruise R7 hex, descending by
    /// peak Lmax. Capped at 5; only present for cruise traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cruise_top_flights: Option<Vec<CruiseHexTopFlight>>,
    /// Ground path length breakdown: `[runway, taxi, apron]` in meters.
    /// `None` for airborne / cruise rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_m_per_kind: Option<[f32; 3]>,
}

#[inline]
fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

/// One bucket inside a cruise R7 hex aggregate trace.
///
/// `is_dep` was dropped in v16 — it was always `true` per Doc 29 §A.3.2
/// (en-route flights use the Departure NPD family; no cruise NPD set
/// is published). The popup kernel hardcodes `is_departure: true` on
/// every cruise segment it scatters.
#[derive(Debug, Clone, Serialize)]
pub struct CruiseBucketBreakdown {
    pub class: u8,
    pub fl_bin: u8,
    pub period: u8,
    pub n_flights: u32,
    pub received_lden: f64,
}

/// One contributing flight inside a cruise R7 hex aggregate trace.
/// Carries the peak encounter (loudest cell crossing of this fid in
/// this hex) plus per-fid metadata captured at extract time.
#[derive(Debug, Clone, Serialize)]
pub struct CruiseHexTopFlight {
    pub lmax_db: f64,
    pub altitude_m: f64,
    pub date: String,
    pub time_utc: String,
    pub icao_hex: String,
    pub aircraft_type: String,
    pub callsign: String,
    pub class_name: String,
}

/// Optional collector passed into `compute_at_point_with_traces` when the
/// popup wants per-segment traces. Callers allocate once, pass `Some(&mut)`,
/// and inspect the filled Vec after the call. Pipeline callers pass `None`
/// and the branch predictor elides the push-site overhead.
///
/// Aircraft sub-types (ground path / airborne sub-segment / cruise R7 hex)
/// all live in `segments`, discriminated by `SegmentTrace.aircraft_subtype`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TraceCollector {
    pub segments: Vec<SegmentTrace>,
    /// Total airborne sub-segments seen above `AIRBORNE_TRACE_CUTOFF_DB`
    /// across the whole popup query. Used as the "N visible" denominator
    /// on the frontend's "shown / N" airborne tab caption. Maintained by
    /// `airborne::scatter` because the heap (bounded top-K) drops most
    /// candidates before they reach `segments`, so the Vec length is no
    /// longer a valid denominator.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub airborne_above_cutoff: u32,
}

#[inline]
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

impl TraceCollector {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Per-kind counts of segments seen at the query point, plus whether the popup
/// truncated the response (top-K cap reached).
///
/// `*_count` = number of traces actually returned to the client (post top-K cap).
/// `*_total` = number computed for this kind before the cap, i.e. how many
/// segments contributed to Lden. Frontends use the pair to render
/// "shown / total" so users see how aggressively the response was truncated;
/// the Lden values themselves always reflect the full set.
/// Aircraft splits into three sub-buckets: ground path / airborne
/// sub-segment / cruise R7 hex. Each is capped separately by
/// `apply_segment_top_k_with_cap` so a busy airport doesn't drown one
/// tab in another's truncation. The frontend renders three sub-tabs
/// keyed off these counters directly — no fold-out re-derivation.
///
/// Contract note (Opt B, 2026-05): `aircraft_airborne_total` counts
/// sub-segments with `Lmax >= AIRBORNE_TRACE_CUTOFF_DB` (25 dB), not
/// every Doc 29 contribution. Sub-segments below the cutoff still
/// accumulate energy into `period_energy` / `peak_lmax` (Lden parity
/// preserved) but no `SegmentTrace` is built. At LKPR popup the
/// hidden delta is ~570 k of ~600 k sub-segs — all acoustically
/// irrelevant (≤ -19 dB Lden contribution), well below the top-150
/// rank floor (~40 dB Lden). Frontend `shown / total` therefore
/// reads "150 of N visible" where N = sub-segs above 25 dB Lmax.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SegmentTracesSummary {
    pub total_count: u32,
    pub truncated: bool,
    pub road_count: u32,
    pub railway_count: u32,
    pub aircraft_ground_count: u32,
    pub aircraft_airborne_count: u32,
    pub aircraft_cruise_count: u32,
    pub building_count: u32,
    pub industrial_count: u32,
    pub road_total: u32,
    pub railway_total: u32,
    pub aircraft_ground_total: u32,
    pub aircraft_airborne_total: u32,
    pub aircraft_cruise_total: u32,
    pub building_total: u32,
    pub industrial_total: u32,
}
