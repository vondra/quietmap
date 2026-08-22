//! Popup compute — consumes `airport_traffic.arrow` rows: each is
//! per-band daily-total linear Z-weighted energy at 25 m perpendicular
//! from one OSM microsegment for one period (Stage 2C writer Σ
//! per-event SEL across n_days ÷ n_days at emission).
//!
//! Per-microsegment-per-row receiver chain (CNOSSOS path effects via
//! the `propagation::path_effects` building blocks the road compute
//! uses):
//!
//! ```text
//! prop_band[i] = 10^((geo_rel_db                                  // 25m → d divergence
//!                   + ALPHA_ATM[i] · (d - 25)/1000               // ISO 9613-2 atm
//!                   + terrain_atten_db[i]                         // DEM-derived
//!                   + screening_atten_db[i]                       // building diffraction
//!                   + veg_atten_db[i]                             // forest mask
//!                   + aircraft_ground_atten_db(i, ground_g)       // frozen aircraft ground
//!                  ) / 10)
//! received_band_lin[i] = row.band_energy_lin[i] × prop_band[i]
//! aw_band_lin[i]       = received_band_lin[i] × 10^(A_WEIGHTING[i] / 10)
//! period_leq_db        = 10·log10(Σ_i aw_band_lin[i] / (n_days × period_seconds))
//! ```
//!
//! `row.band_energy_lin` is the raw Σ over n_days of Z-weighted band
//! energy at 25 m from this microsegment in this period. `period_leq`
//! divides by `n_days × period_seconds` to recover Leq.
//!
//! Path-effect terms (terrain / screening / vegetation / ground) are
//! computed ONCE per unique microsegment (cached in
//! `MicrosegPathCache`) and applied per row. At LKPR ~78k rows
//! collapse to ~3.7k unique `(osm_id, segment_idx)` geometries — same
//! granularity road compute pays for, but amortised across all rows
//! at the microsegment.
//!
//! Per-effect impact deltas (terrain_impact_db, screening_impact_db,
//! etc.) come from 5 variant energy accumulators per period (full,
//! no_terrain, no_screening, no_vegetation, no_atmospheric), folded
//! into airport-aggregate Lden deltas at output time. Each variant
//! re-runs the band-energy multiply with one path-effect term
//! zeroed; the delta in dB between the receiver Lden of `full` and
//! `no_X` is the A-weighted ΔL_A attributable to effect X across
//! all contributing microsegments at this receiver.
//!
//! Rows are bucketed per `airport_key` to produce one popup Contributor
//! per airport, with a `MultiLineString` covering every microsegment
//! that contributed energy.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::compute::aircraft_v6::views::AirportTrafficRowView;
use crate::constants::ALPHA_ATM;
use crate::emission::aircraft::{
    self, GROUND_OPS_KIND_APRON_MOVEMENT, GROUND_OPS_KIND_RUNWAY_ROLL, GROUND_OPS_KIND_TAXI,
    GROUND_OPS_REF_OFFSET_M, GROUND_OPS_SOURCE_HEIGHT_M,
};
use crate::emission::gse::NUM_GSE_CLASSES;
use crate::emission::profiles_generated::NUM_CLASSES;
use crate::periods;
use crate::propagation::geo::point_to_segment_full;
use crate::propagation::iso9613::aircraft_ground_atten_db;
use crate::propagation::path_effects;
use crate::propagation::PathProfile;
use crate::types::{
    AircraftGroundOpsClassDetail, AircraftGroundOpsDetail, AircraftMetadata, Barrier, Contributor,
    LayerKind, NoisePeriods, ProfileMixEntry, PropagationBaseline, RasterSampler, Receiver,
    SourceMetadata,
};

use super::NUM_BANDS;

/// Top-N aircraft class count exposed via `profile_mix`. Must match
/// the frontend `PROFILE_MIX_TOP_N` constant in `DetailPopup.tsx` —
/// the FE renders a deterministic "Other" rollup as
/// `1 - Σ shares`, which only sums to 1 if both sides agree on the
/// cut. Three is consistent with `types.rs:573` ("Top-3 noise
/// classes by linear received energy at this airport").
const PROFILE_MIX_TOP_N: usize = 3;

// Receiver-side reach is per-ops_kind via `constants::ground_ops_max_radius`
// (RUNWAY 5 km / TAXI 3 km / APRON 1.5 km). Heatmap-side mirror in
// `ground_ops.rs` uses the same helper for parity. (Plain `//` instead of
// `///` — this block is module-level prose, not a docstring for the next
// `popup_pixel_floor_m` item.)

/// Half a base-level Mercator pixel at `lat`, in metres. The HM3 ground
/// heatmap is rendered from the base tiles (z12 with 512-px tiles since
/// the 2026-07 shift — the same physical lattice as the old z13@256),
/// and its kernel floors `d_perp` / `d_endpoint` at this value to
/// anti-alias the line-source `1/d` singularity
/// (`ground_ops.rs::scatter_tile`). The popup uses the same floor so the
/// numbers it shows match the HM3 pixel under the cursor on near-line
/// receivers.
fn popup_pixel_floor_m(lat: f64) -> f64 {
    // 78_271.516… is the WGS84 equatorial metres-per-pixel at z=0 with
    // 512-px tiles; /4096 = the z12 base. Mirror of
    // `raster_reader::fused_tile_z13::tile_pixel_size_m(12, lat)`, inlined
    // to avoid a cross-crate dep for one constant. The VALUE is identical
    // to the pre-shift 156_543.033…/8192 form — the physical finest pixel
    // (~19.1 m equatorial) did not change, so popup numbers are invariant.
    const BASE_EQUATORIAL_HALF_PX_M: f64 = 78_271.516_964_020_5 / 4096.0 * 0.5;
    BASE_EQUATORIAL_HALF_PX_M * lat.to_radians().cos()
}

/// `10^(A_WEIGHTING[i] / 10)` for each of the eight bands. Precomputing
/// turns the per-row hot loop into a dot-product over `band_energy_lin`,
/// dropping eight `powf` calls per row.
const A_WEIGHT_LIN: [f64; NUM_BANDS] = {
    let mut t = [0.0; NUM_BANDS];
    let mut i = 0;
    while i < NUM_BANDS {
        // `f64::powf` isn't `const`; expand 10^x as exp(x·ln 10) via
        // an inlined Taylor + look-up — but for our fixed-table case
        // it's simpler to inline numeric literals from the standard
        // ISO 226 A-weighting (63 Hz … 8 kHz centres):
        //   A = [-26.2, -16.1, -8.6, -3.2, 0.0, 1.2, 1.0, -1.1]
        //   10^(A/10) = [0.002399, 0.024547, 0.138038, 0.478630,
        //                1.0, 1.318257, 1.258925, 0.776247]
        t[i] = [
            0.002398832919019,
            0.024547089156851,
            0.138038426460289,
            0.478630092322638,
            1.0,
            1.318256738556407,
            1.258925411794167,
            0.776247116628692,
        ][i];
        i += 1;
    }
    t
};

/// Map an `airport_key` to a popup-friendly display string.
/// - Real OSM airports keep their key verbatim (e.g. "LKPR" →
///   "LKPR"; popup wraps to "Aircraft - LKPR ground ops").
/// - Synthetic auto-discovered airfields use a `auto-<H3-R11>`
///   key. Parse the H3 cell, format the centroid coordinates so
///   users see something meaningful instead of opaque hex
///   (e.g. "Auto airfield 50.04,14.26"). Stage 1.5 stores a
///   richer name in `synth_airport_areas.arrow` (with length + visit
///   count) but the popup doesn't currently load that sidecar; the
///   coordinate prefix is the minimum useful surface.
/// - Strip orphan fallback `strip:<R7>` is kept as-is (popup
///   already labels these "strip cluster").
fn synth_airport_display_name(airport_key: &str) -> String {
    use h3o::{CellIndex, LatLng};
    use std::str::FromStr;
    if let Some(hex) = airport_key.strip_prefix("auto-") {
        if let Ok(cell) = CellIndex::from_str(hex) {
            let ll = LatLng::from(cell);
            return format!("Auto airfield {:.2},{:.2}", ll.lat(), ll.lng());
        }
    }
    airport_key.to_string()
}

#[inline]
fn db_to_lin(db: f64) -> f64 {
    (db * std::f64::consts::LN_10 * 0.1).exp()
}

/// Run the full ISO 9613-2 / CNOSSOS path-effect kernel for one
/// microsegment. Output is cached per `(osm_id, segment_idx)` for
/// downstream rows. Cost ~80-150 µs per call at typical raster
/// density (matches road compute's per-segment path cost).
#[allow(clippy::too_many_arguments)]
fn compute_microseg_path(
    rasters: &dyn RasterSampler,
    barriers: &[Barrier],
    obstacles: Option<&crate::propagation::obstacle_index::ObstacleSet>,
    cand_scratch: &mut Vec<crate::propagation::obstacle_index::CrossingCandidate>,
    src_lat: f64,
    src_lon: f64,
    rcv_lat: f64,
    rcv_lon: f64,
    d_to_recv: f64,
    rcv_alt: f64,
) -> MicrosegPath {
    // CNOSSOS heavy-vehicle source height (4 m) — same as road.
    let src_alt = rasters.elevation(src_lat, src_lon) + GROUND_OPS_SOURCE_HEIGHT_M;
    let mut path_profile = PathProfile::new();
    rasters.build_path_profile(
        src_lat,
        src_lon,
        rcv_lat,
        rcv_lon,
        d_to_recv,
        &mut path_profile,
    );

    let ground_g = path_effects::ground_g_from_profile(&path_profile);
    let (terrain, _terrain_profile_points) =
        path_effects::terrain_attenuation_with_meta(&mut path_profile, src_alt, rcv_alt);
    let obstacle_input = crate::obstacle_input_for_ray(
        obstacles,
        cand_scratch,
        src_lat,
        src_lon,
        rcv_lat,
        rcv_lon,
        None,
    );
    let (screening_atten, _obstacle_trace) = path_effects::screening_attenuation_with_meta(
        &mut path_profile,
        barriers,
        obstacle_input,
        src_alt,
        rcv_alt,
        0.0, // no exclusion radius — airport ground source is point-like
        &terrain.attenuation_bands,
    );
    let vegetation_atten = path_effects::vegetation_attenuation_path(&path_profile);

    MicrosegPath {
        terrain_atten_db: terrain.attenuation_bands,
        screening_atten_db: screening_atten,
        vegetation_atten_db: vegetation_atten,
        ground_g,
    }
}

/// Cached per-microsegment path-effect bands. Stage 2C airport_traffic
/// rows share `(osm_id, segment_idx)` across many ops_kind / class /
/// period dimensions, so computing CNOSSOS path effects once per
/// microsegment and reusing across rows cuts the path-effect kernel
/// from 78k calls at LKPR to ~3.7k.
///
/// All four arrays hold per-band dB attenuation (negative for losses).
/// `ground_g` is the scalar ground factor [0, 1] consumed by the shared
/// `iso9613::aircraft_ground_atten_db` term in the per-band propagation.
#[derive(Clone, Copy)]
struct MicrosegPath {
    terrain_atten_db: [f64; NUM_BANDS],
    screening_atten_db: [f64; NUM_BANDS],
    vegetation_atten_db: [f64; NUM_BANDS],
    ground_g: f64,
}

/// Per-microsegment energy + geometry accumulator. Stage 2C emits
/// multiple `airport_traffic.arrow` rows for the same `(osm_id,
/// segment_idx)` across period / class / ops_kind / is_dep /
/// veh_kind dimensions. The popup needs ONE `SegmentTrace` per
/// microsegment with the rolled-up energy (so the Noise Segments
/// tab shows e.g. "LKPR runway-roll 250 m 54 dB" instead of
/// thousands of duplicates).
struct MicrosegAcc {
    airport_key: String,
    ops_kind: u8,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    length_m: f64,
    /// A-weighted linear energy per period across the 6 propagation
    /// variants (full / no_terrain / no_screening / no_vegetation /
    /// no_atmospheric / no_ground).
    period_energy_full: [f64; 3],
    period_energy_no_terrain: [f64; 3],
    period_energy_no_screening: [f64; 3],
    period_energy_no_vegetation: [f64; 3],
    period_energy_no_atmospheric: [f64; 3],
    period_energy_no_ground: [f64; 3],
    /// Z-weighted linear source band energy summed across all rows
    /// touching this microsegment, per period × band.
    band_energy_lin_per_period: [[f64; NUM_BANDS]; 3],
    /// Post-propagation A-weighted linear band energy at the
    /// receiver, per period × band.
    received_bands_lin_per_period: [[f64; NUM_BANDS]; 3],
    /// v5: per-microsegment UNION counts come straight from the row
    /// (`microseg_unique_*` is row-replicated). Captured from the
    /// FIRST row of a microsegment; all subsequent rows carry the
    /// same value by construction in Stage 2C.
    unique_count: u32,
    unique_arr_count: u32,
    unique_dep_count: u32,
    unique_gse_count_per_class: [u32; NUM_GSE_CLASSES],
    /// v9 GA-class split of the three counts above, so the trace divides
    /// `non_ga / n_days + ga / ga_n_days`.
    unique_ga_count: u32,
    unique_ga_arr_count: u32,
    unique_ga_dep_count: u32,
    /// Per-aircraft-class energy share at THIS microsegment — used
    /// for the class_mix display (top-N "what dominates here").
    class_energy: [f64; NUM_CLASSES],
}

struct AirportAcc {
    name: String,
    /// Per-period Σ A-weighted band-linear energy (linear sum across
    /// bands at receiver, in W·s equivalent units).
    period_energy: [f64; 3],
    /// Variant accumulators for popup ΔL_A breakdown.
    period_energy_no_terrain: [f64; 3],
    period_energy_no_screening: [f64; 3],
    period_energy_no_vegetation: [f64; 3],
    period_energy_no_atmospheric: [f64; 3],
    period_energy_no_ground: [f64; 3],
    /// Linear A-weighted received energy keyed by aircraft class_idx
    /// (`veh_kind = 0` only). Used to compute `profile_mix` top-N.
    class_energy: [f64; NUM_CLASSES],
    /// Per-ops-kind period_energy buckets so the popup can show
    /// per-class (runway / taxi / apron) Lden breakdown.
    runway_period_energy: [f64; 3],
    taxi_period_energy: [f64; 3],
    apron_period_energy: [f64; 3],
    /// Energy-weighted receiver distance.
    sum_energy: f64,
    sum_energy_x_dist: f64,
    /// Σ of pre-propagation A-weighted band-energy at the 25 m
    /// reference across rows.
    sum_energy_25m: f64,
}

/// HashMap keyed by `airport_key` → global UNION counts across all
/// R4s. Source-reader builds this from `airport_summary.arrow` ONCE per
/// process (it lives inside the mtime-keyed `AirportSummaryAccum` cache)
/// and hands a borrow to [`run`]. **Missing entry** (or missing summary
/// file) → popup MUST refuse to compute airport arr/dep counts (returns
/// `None`); there is no fallback to per-row sums.
///
/// Owned `String` keys rather than `&str` borrowed from a parallel
/// `Vec<String>`: the borrowed form forced the map to be rebuilt on every
/// query because it could not outlive a single call, and the global
/// sidecar carries ~50 k airports — measured at tens of ms per click, for
/// a table whose contents never change between clicks. Lookups still take
/// `&str` (`String: Borrow<str>`), so call sites are unchanged.
pub type AirportSummaryLookup = std::collections::HashMap<String, AirportSummaryEntry>;

/// One row of `airport_summary.arrow`. Mirrors the popup's read-side
/// view but owned for the duration of the popup query.
#[derive(Clone, Copy, Debug, Default)]
pub struct AirportSummaryEntry {
    /// NON-GA-class window counts (airline 12-day). v9 split.
    pub arr_count: u32,
    pub dep_count: u32,
    pub gse_count_per_class: [u32; NUM_GSE_CLASSES],
    /// Index 0=runway, 1=taxi, 2=apron — VEH_KIND=0 only.
    pub ops_count_per_kind: [u32; 3],
    /// GA-class split of arr/dep/ops. The popup divides
    /// `non_ga / n_days + ga / ga_n_days`. GSE has no GA split
    /// (airline-pass only).
    pub ga_arr_count: u32,
    pub ga_dep_count: u32,
    pub ga_ops_count_per_kind: [u32; 3],
}

/// Run the airport_traffic.arrow popup compute path. `n_days` flows
/// from the arrow metadata via `source-reader/PointQueryData.n_days`
/// and divides into the aggregated unique counts to yield the popup's
/// `arrivals_per_day` / `departures_per_day` / `gse_per_day` counts.
///
/// `airport_summary` is the global UNION lookup (v5 sidecar) keyed by
/// `airport_key`. When `None` or when an airport is missing from the
/// lookup, the popup returns `None` for that airport's arr/dep counts.
/// Per-row sums are forbidden because they would
/// over-count rotations crossing N microsegments by ~N×.
///
/// `osm_ref_lookup` maps `osm_id` → OSM `ref` tag (e.g. "06/24") for
/// real-OSM aeroway lines. Synthetic osm_ids (top bit set, from
/// Stage 1.5 DBSCAN) are never in this lookup.
#[allow(clippy::too_many_arguments)]
pub fn run(
    receiver: &Receiver,
    rows: &[AirportTrafficRowView<'_>],
    n_days: u16,
    // GA hybrid per-class weight LUT. Applied to received Lden energy and
    // movement counts of `veh_kind == 0` (aircraft) rows only — GSE
    // (`veh_kind == 1`) rows always weight 1.0 (their `class_idx` indexes
    // the GSE class space and GSE is an airline-pass artifact). The
    // per-event source Lw display (`emission_db`, `lw_bands`) stays
    // unweighted — per-event physics, like the airborne SEL.
    class_weights: &crate::emission::aircraft::ClassWeights,
    rasters: &dyn RasterSampler,
    barriers: &[Barrier],
    // Vector obstacles (geodata-v2): ground-ops screening takes the same
    // exact building crossings as every other popup surface kernel; `None`
    // keeps the raster path byte-identical.
    obstacles: Option<&crate::propagation::obstacle_index::ObstacleSet>,
    osm_ref_lookup: &HashMap<u64, String>,
    airport_summary: Option<&AirportSummaryLookup>,
    traces: Option<&mut crate::types::TraceCollector>,
) -> Vec<Contributor> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut cand_scratch: Vec<crate::propagation::obstacle_index::CrossingCandidate> = Vec::new();
    let n_days_f = (n_days as f64).max(1.0);
    // GA-window divisor for the split-union movement counts: GA-class fids
    // use the GA window, so the popup divides them by THIS while
    // non-GA counts divide by `n_days`.
    let ga_n_days_f = (class_weights.ga_n_days() as f64).max(1.0);
    let recv_lat = receiver.lat;
    let recv_lon = receiver.lon;
    let rcv_alt = receiver.altitude_m();
    // Urban-reflection boost is a receiver-only property — same value
    // across every microsegment for this popup. Sample once and reuse
    // in both the hot loop and the segment-trace emission. In vector
    // mode the sampler is the 1.4b wrapper (VectorReflectionSampler),
    // so this probe answers from exact footprints like every kernel.
    let refl_db = rasters.building_enclosure(recv_lat, recv_lon);
    // Heatmap-parity divergence floor: the user reads popup numbers
    // off a base HM3 pixel, so the popup uses the same half-pixel floor on
    // `d_perp`/`d_endpoint` as `tile_painter::ground_ops::scatter_tile`.
    // Without it, the popup
    // reports ~5-8 dB louder than the underlying pixel on near-line
    // receivers (line-source 1/d singularity sampled at a point).
    let pixel_floor_m = popup_pixel_floor_m(recv_lat);

    let mut by_airport: HashMap<String, AirportAcc> = HashMap::new();
    let mut microseg_cache: HashMap<(u64, u16), MicrosegPath> = HashMap::new();
    // Per-microsegment energy accumulator for SegmentTrace emission.
    // Key (osm_id, segment_idx). Rows sharing the same microsegment
    // (different period / class / ops_kind / etc) fold into one trace
    // row, emitted at the end as `aircraft_subtype = 1` in the Noise
    // Segments tab.
    let mut by_microseg: HashMap<(u64, u16), MicrosegAcc> = HashMap::new();
    for row in rows {
        // Three-semantic decomposition:
        //  - `d_endpoint`: Euclidean to nearest endpoint, used for the
        //    per-ops_kind reach prune (max 5 km), atmospheric absorption integration, and the
        //    path-profile sampling length (source = clamped foot on
        //    segment, receiver = popup point).
        //  - `pts.d_perp_m`: perpendicular to the EXTENDED line, used by
        //    the aircraft line-source receiver formula
        //    `+ 10·log10(θ / d_perp)` per CNOSSOS-EU §2.5.5.
        //  - `pts.fraction`: signed unclamped along-line position, used
        //    by the subtended-angle math so receivers past the segment
        //    endpoints get the correct (small) θ via signed atan.
        let pts = point_to_segment_full(
            recv_lat,
            recv_lon,
            row.start_lat as f64,
            row.start_lon as f64,
            row.end_lat as f64,
            row.end_lon as f64,
        );
        let cp_lat = pts.cp_lat;
        let cp_lon = pts.cp_lon;
        let d_to_recv = pts.d_endpoint_m.max(pixel_floor_m);
        if d_to_recv > crate::constants::ground_ops_max_radius(row.ops_kind) {
            continue;
        }
        // A_refl reuses the per-receiver value sampled once above —
        // CNOSSOS-EU §2.5 multi-bounce approximation, the same
        // sampler-backed 0/1.5/3 dB receiver value every surface kernel
        // applies (SPEC §3.8; raster probe, or exact footprints under
        // the 1.4b wrapper).

        // Per-microsegment path effects, cached. Apply at ALL
        // distances inside the per-ops_kind reach (5/3/1.5 km) — same
        // shape as road compute at `lib.rs:398-468`, which has no
        // distance gate beyond max-reach prune. The previous 6 km
        // gate created a `+8 dB` discontinuity in soft-ground
        // receivers right around that boundary (microsegments at
        // 5.999 km had full ground attenuation; ones at 6.001 km
        // had ZERO_PATH). Road kernel doesn't gate, so neither do
        // we — popup latency stays inside budget at LKPR
        // (~3 k microsegments × ~100 µs ≈ 300 ms within the prune
        // radius; most are at <5 km from receiver-near popups).
        let path = *microseg_cache
            .entry((row.osm_id, row.segment_idx))
            .or_insert_with(|| {
                compute_microseg_path(
                    rasters,
                    barriers,
                    obstacles,
                    &mut cand_scratch,
                    cp_lat,
                    cp_lon,
                    recv_lat,
                    recv_lon,
                    d_to_recv,
                    rcv_alt,
                )
            });

        // Receiver-side geometric term — branches by `veh_kind`:
        //  - Aircraft (0): CNOSSOS-EU §2.5.5 line-source formula
        //    `+ 10·log10(θ / d_perp)` where θ is the angle subtended by
        //    the full microsegment as seen from the receiver. Signed
        //    `fraction` makes the formula work for receivers past either
        //    endpoint (one of d1, d2 turns negative; atan obliges).
        //    Stored `band_energy_lin` is per-metre LW' × density.
        //  - GSE (1): per-event SEL@25m → point-source divergence from
        //    the 25 m anchor. Stored `band_energy_lin` already integrates
        //    the kinematic moving-point pass over `length_within_segment_m`.
        let geo_recv_db = if row.veh_kind == 0 {
            let d_perp = pts.d_perp_m.max(pixel_floor_m);
            let l = row.length_m as f64;
            let rx_along = pts.fraction * l;
            let theta = ((l - rx_along) / d_perp).atan() + (rx_along / d_perp).atan();
            10.0 * (theta.max(1e-12) / d_perp).log10()
        } else {
            10.0 * (GROUND_OPS_REF_OFFSET_M / d_to_recv).log10()
        };
        let d_minus_ref_km = (d_to_recv - GROUND_OPS_REF_OFFSET_M).max(0.0) / 1000.0;
        let mut prop_full = [0.0f64; NUM_BANDS];
        let mut prop_no_terrain = [0.0f64; NUM_BANDS];
        let mut prop_no_screening = [0.0f64; NUM_BANDS];
        let mut prop_no_vegetation = [0.0f64; NUM_BANDS];
        let mut prop_no_atmospheric = [0.0f64; NUM_BANDS];
        let mut prop_no_ground = [0.0f64; NUM_BANDS];
        for i in 0..NUM_BANDS {
            // ISO 9613-2 §7.3.1 / CNOSSOS-EU §2.5.6: ground and
            // obstacle attenuations DON'T add linearly. The standard
            // pattern is `A_gob = max(A_gr, A_bar)` where
            // `A_bar = A_terrain + A_screening` (when either is
            // non-zero; otherwise `A_gob = A_gr` directly).
            // Mirrors the road kernel at iso9613.rs:217-228.
            //
            // Vegetation is separate and added directly (not part
            // of A_bar). Atmospheric loss is added at the source
            // distance (here, from the 25 m line-source anchor
            // outward).
            let atm_atten_db = ALPHA_ATM[i] * d_minus_ref_km;
            let a_gr = aircraft_ground_atten_db(i, path.ground_g);
            let a_terr = path.terrain_atten_db[i];
            let a_scr = path.screening_atten_db[i];
            let a_veg = path.vegetation_atten_db[i];
            let a_bar_full = a_terr + a_scr;
            let a_bar_no_t = a_scr;
            let a_bar_no_s = a_terr;
            let max_gob = |a_bar: f64| -> f64 {
                if a_bar > 0.0 {
                    a_gr.max(a_bar)
                } else {
                    a_gr
                }
            };
            let gob_full = max_gob(a_bar_full);
            let gob_no_t = max_gob(a_bar_no_t);
            let gob_no_s = max_gob(a_bar_no_s);
            // `path_base` carries geometry-level terms (CNOSSOS line-source
            // angle/distance for aircraft, point-source divergence for GSE,
            // plus A_refl) so every variant inherits them — otherwise the
            // `no_atmospheric` ablation would silently drop them and
            // mis-attribute their magnitude to atmospheric impact in the
            // popup propagation breakdown.
            let path_base = geo_recv_db + refl_db;
            let base = path_base - atm_atten_db;
            prop_full[i] = db_to_lin(base - gob_full - a_veg);
            prop_no_terrain[i] = db_to_lin(base - gob_no_t - a_veg);
            prop_no_screening[i] = db_to_lin(base - gob_no_s - a_veg);
            prop_no_vegetation[i] = db_to_lin(base - gob_full);
            prop_no_atmospheric[i] = db_to_lin(path_base - gob_full - a_veg);
            // `no_ground` semantics per road kernel: keep A_bar (= terrain
            // + screening), drop A_gr from the max (i.e. use A_bar alone
            // as the obstacle term). For receivers behind a hill /
            // building this is essentially a no-op (gob was already
            // a_bar); over flat soft ground it removes the ground
            // absorption.
            prop_no_ground[i] = db_to_lin(base - a_bar_full - a_veg);
        }

        // A-weighted linear energy at the receiver. Per-band Z energy
        // → per-band received Z energy via prop multiply → A-weight at
        // receiver → sum. Variants reuse the same row energy with
        // their respective prop_X array.
        let mut aw_band_sum = 0.0f64;
        let mut aw_band_sum_25m = 0.0f64;
        let mut aw_no_terrain = 0.0f64;
        let mut aw_no_screening = 0.0f64;
        let mut aw_no_vegetation = 0.0f64;
        let mut aw_no_atmospheric = 0.0f64;
        let mut aw_no_ground = 0.0f64;
        for i in 0..NUM_BANDS {
            let z = row.band_energy_lin[i] as f64;
            let aw_lin = A_WEIGHT_LIN[i];
            aw_band_sum_25m += z * aw_lin;
            aw_band_sum += z * prop_full[i] * aw_lin;
            aw_no_terrain += z * prop_no_terrain[i] * aw_lin;
            aw_no_screening += z * prop_no_screening[i] * aw_lin;
            aw_no_vegetation += z * prop_no_vegetation[i] * aw_lin;
            aw_no_atmospheric += z * prop_no_atmospheric[i] * aw_lin;
            aw_no_ground += z * prop_no_ground[i] * aw_lin;
        }
        // GA hybrid weight: aircraft rows scale by `w[class]`, GSE rows by
        // 1.0. Fold into the
        // RECEIVED energies (all variants) so every Lden-normalized
        // accumulator below — airport, per-microseg, per-ops-kind,
        // class_energy, and the trace `received_bands` — inherits the
        // `1/ga_n_days` scaling for a one-off GA movement. `aw_band_sum_25m` (the
        // per-event emission Lw display) is deliberately left UNWEIGHTED:
        // it is per-event source physics, like the airborne SEL/Lmax.
        let row_weight = if row.veh_kind == 0 {
            class_weights.get(row.class_idx)
        } else {
            1.0
        };
        aw_band_sum *= row_weight;
        aw_no_terrain *= row_weight;
        aw_no_screening *= row_weight;
        aw_no_vegetation *= row_weight;
        aw_no_atmospheric *= row_weight;
        aw_no_ground *= row_weight;
        if !aw_band_sum.is_finite() || aw_band_sum <= 0.0 {
            continue;
        }
        let period = row.period.min(2) as usize;
        let acc = if let Some(acc) = by_airport.get_mut(row.airport_key) {
            acc
        } else {
            let display_name = synth_airport_display_name(row.airport_key);
            by_airport
                .entry(row.airport_key.to_string())
                .or_insert_with(|| AirportAcc {
                    name: format!("Aircraft - {display_name} ground ops"),
                    period_energy: [0.0; 3],
                    period_energy_no_terrain: [0.0; 3],
                    period_energy_no_screening: [0.0; 3],
                    period_energy_no_vegetation: [0.0; 3],
                    period_energy_no_atmospheric: [0.0; 3],
                    period_energy_no_ground: [0.0; 3],
                    class_energy: [0.0; NUM_CLASSES],
                    runway_period_energy: [0.0; 3],
                    taxi_period_energy: [0.0; 3],
                    apron_period_energy: [0.0; 3],
                    sum_energy: 0.0,
                    sum_energy_x_dist: 0.0,
                    sum_energy_25m: 0.0,
                })
        };
        // Writer-contract guards. Stage 2C never emits
        // - `veh_kind` ∉ {0=aircraft, 1=GSE}
        // - `is_departure` ∉ {0, 1}
        // - aircraft `class_idx` ≥ NUM_CLASSES
        // - GSE `class_idx` ≥ NUM_GSE_CLASSES (pre-filtered in writer)
        // - `ops_kind` ∉ {RUNWAY_ROLL, TAXI, APRON_MOVEMENT}
        //
        // Any of these reaching the kernel means a stale binary or
        // arrow schema drift — debug-assert so dev/CI panics; release
        // silently degrades (skips affected bucketing) to keep the
        // popup responsive.
        debug_assert!(
            row.veh_kind <= 1,
            "veh_kind {} unexpected (expected 0 or 1); stale binary or arrow?",
            row.veh_kind
        );
        debug_assert!(
            row.is_departure <= 1,
            "is_departure {} not in {{0,1}}",
            row.is_departure
        );
        if row.veh_kind == 0 {
            debug_assert!(
                (row.class_idx as usize) < NUM_CLASSES,
                "aircraft class_idx {} out of range (expected < {NUM_CLASSES})",
                row.class_idx
            );
        }
        acc.period_energy[period] += aw_band_sum;
        acc.period_energy_no_terrain[period] += aw_no_terrain;
        acc.period_energy_no_screening[period] += aw_no_screening;
        acc.period_energy_no_vegetation[period] += aw_no_vegetation;
        acc.period_energy_no_atmospheric[period] += aw_no_atmospheric;
        acc.period_energy_no_ground[period] += aw_no_ground;

        // Per-microsegment fold for SegmentTrace emission. v5: per-
        // microsegment UNION counts come directly from the row's
        // `microseg_unique_*` scalars (row-replicated). We capture
        // them on first insert; subsequent rows of the same
        // microsegment all carry the same value so we don't need to
        // dedup ourselves.
        let microseg_entry = by_microseg
            .entry((row.osm_id, row.segment_idx))
            .or_insert_with(|| MicrosegAcc {
                airport_key: row.airport_key.to_string(),
                ops_kind: row.ops_kind,
                start_lat: row.start_lat as f64,
                start_lon: row.start_lon as f64,
                end_lat: row.end_lat as f64,
                end_lon: row.end_lon as f64,
                length_m: row.length_m as f64,
                period_energy_full: [0.0; 3],
                period_energy_no_terrain: [0.0; 3],
                period_energy_no_screening: [0.0; 3],
                period_energy_no_vegetation: [0.0; 3],
                period_energy_no_atmospheric: [0.0; 3],
                period_energy_no_ground: [0.0; 3],
                band_energy_lin_per_period: [[0.0; NUM_BANDS]; 3],
                received_bands_lin_per_period: [[0.0; NUM_BANDS]; 3],
                unique_count: row.microseg_unique_count,
                unique_arr_count: row.microseg_unique_arr_count,
                unique_dep_count: row.microseg_unique_dep_count,
                unique_gse_count_per_class: *row.microseg_unique_gse_count_per_class,
                unique_ga_count: row.microseg_unique_ga_count,
                unique_ga_arr_count: row.microseg_unique_ga_arr_count,
                unique_ga_dep_count: row.microseg_unique_ga_dep_count,
                class_energy: [0.0; NUM_CLASSES],
            });
        microseg_entry.period_energy_full[period] += aw_band_sum;
        microseg_entry.period_energy_no_terrain[period] += aw_no_terrain;
        microseg_entry.period_energy_no_screening[period] += aw_no_screening;
        microseg_entry.period_energy_no_vegetation[period] += aw_no_vegetation;
        microseg_entry.period_energy_no_atmospheric[period] += aw_no_atmospheric;
        microseg_entry.period_energy_no_ground[period] += aw_no_ground;
        for i in 0..NUM_BANDS {
            let z = row.band_energy_lin[i] as f64;
            // Source Lw display stays per-event (unweighted); the received
            // band energy carries the GA hybrid `row_weight` so the trace's
            // per-band received Lp matches the weighted scalar Lden above.
            microseg_entry.band_energy_lin_per_period[period][i] += z;
            microseg_entry.received_bands_lin_per_period[period][i] +=
                z * prop_full[i] * A_WEIGHT_LIN[i] * row_weight;
        }
        if row.veh_kind == 0 && (row.class_idx as usize) < NUM_CLASSES {
            microseg_entry.class_energy[row.class_idx as usize] += aw_band_sum;
        }
        acc.sum_energy += aw_band_sum;
        acc.sum_energy_x_dist += aw_band_sum * d_to_recv;
        acc.sum_energy_25m += aw_band_sum_25m;
        // Per-ops-kind energy buckets stay — they integrate band
        // energy across rows of the same kind. v5 dropped the per-
        // ops-kind flight_id HashSets here; airport-level unique
        // counts come from `airport_summary` lookup at build time.
        match row.ops_kind {
            GROUND_OPS_KIND_RUNWAY_ROLL => acc.runway_period_energy[period] += aw_band_sum,
            GROUND_OPS_KIND_TAXI => acc.taxi_period_energy[period] += aw_band_sum,
            GROUND_OPS_KIND_APRON_MOVEMENT => acc.apron_period_energy[period] += aw_band_sum,
            _ => debug_assert!(
                false,
                "ops_kind {} not in {{RUNWAY_ROLL, TAXI, APRON_MOVEMENT}}",
                row.ops_kind
            ),
        }
        if row.veh_kind == 0 {
            if (row.class_idx as usize) < acc.class_energy.len() {
                acc.class_energy[row.class_idx as usize] += aw_band_sum;
            }
        } else {
            // GSE class out-of-range = stale arrow vs current binary
            // (or schema drift). Stage 2C writer pre-filters this,
            // so reaching here means a binary/cache mismatch.
            debug_assert!(
                (row.class_idx as usize) < NUM_GSE_CLASSES,
                "GSE class_idx {} out of range (expected < {NUM_GSE_CLASSES}); \
                 stale binary or arrow file? See AGENTS.md.",
                row.class_idx
            );
        }
    }

    // Ascending `(osm_id, segment_idx)` / `airport_key` from here on, not
    // HashMap order. Both feed order-sensitive f64 work downstream: the
    // contributor sequence is summed by `periods::sum_periods` into the
    // popup's aircraft total, and the microsegment sequence decides the
    // 150-row `GROUND_TRACE_CAP` cut and the MultiLineString byte order.
    // See `crate::compute::key_sorted` for why sorting rather than a fixed hasher.
    let microsegs_by_id = crate::compute::into_key_sorted(by_microseg);
    let mut out: Vec<Contributor> = Vec::with_capacity(by_airport.len());
    for (airport_key, acc) in crate::compute::into_key_sorted(by_airport) {
        // Stored band energy is raw Σ over n_days (v6); period_leq
        // divides by `n_days × period_seconds` to recover Leq.
        let ld = aircraft::period_leq(acc.period_energy[0], n_days_f, aircraft::PERIOD_SECONDS[0]);
        let le = aircraft::period_leq(acc.period_energy[1], n_days_f, aircraft::PERIOD_SECONDS[1]);
        let ln = aircraft::period_leq(acc.period_energy[2], n_days_f, aircraft::PERIOD_SECONDS[2]);
        let periods = periods::periods(ld, le, ln);
        if !periods.lden_db.is_finite() {
            continue;
        }
        let summary_entry = airport_summary.and_then(|m| m.get(airport_key.as_str()).copied());
        let metadata =
            build_ground_ops_metadata(&acc, &periods, n_days_f, ga_n_days_f, summary_entry);
        out.push(Contributor {
            source_type: LayerKind::Aircraft,
            osm_id: None,
            name: acc.name.clone(),
            subtype: format!("airport_traffic:{airport_key}"),
            // Ground ops surround the receiver from all sides — "0 m
            // overhead" matches the FE label convention for airborne
            // contributors. The energy-weighted centroid distance is
            // available in `metadata.distance_m` for the detail row.
            distance_m: 0.0,
            periods: periods.clone(),
            periods_free: periods,
            emission_db: metadata.emission_db,
            baseline: PropagationBaseline::default(),
            terrain: Default::default(),
            screening: Default::default(),
            vegetation: Default::default(),
            // Mirror the airport-aggregate ΔL_A onto the Contributor
            // fields the FE reads for road/rail. For ground ops the
            // FE actually reads from `metadata.ground_ops.*_impact_db`
            // (richer struct), but keeping the Contributor-level
            // fields populated avoids divergent display paths and
            // matches the road/rail convention.
            terrain_impact_db: metadata.terrain_impact_db,
            screening_impact_db: metadata.screening_impact_db,
            vegetation_impact_db: metadata.vegetation_impact_db,
            atmospheric_impact_db: metadata.atmospheric_impact_db,
            ground_impact_db: metadata.ground_impact_db,
            received_bands: [0.0; NUM_BANDS],
            geometry: {
                // Emit one LineString per UNIQUE microsegment (not per
                // arrow row). Same `(osm_id, segment_idx)` repeats ~44×
                // across periods × veh_kinds × classes; without this
                // dedup the LKPR ground-ops contributor ships ~5 MB of
                // exact duplicates. `airport_key` uniqueness per
                // microseg is guaranteed by Stage 2C's R4Cache key
                // resolution (airport_traffic_writer.rs:511).
                let pairs: Vec<LatLonSegment> = microsegs_by_id
                    .iter()
                    .map(|(_, m)| m)
                    .filter(|m| m.airport_key.as_str() == airport_key.as_str())
                    .map(|m| {
                        (
                            (m.start_lat as f32, m.start_lon as f32),
                            (m.end_lat as f32, m.end_lon as f32),
                        )
                    })
                    .collect();
                Some(
                    serde_json::from_str(&multiline_geojson(&pairs))
                        .unwrap_or(serde_json::Value::Null),
                )
            },
            metadata: Some(SourceMetadata::Aircraft(Box::new(AircraftMetadata {
                variant: "ground_ops".to_string(),
                airport_name: Some(acc.name.clone()),
                airport_key: Some(airport_key.clone()),
                airborne: None,
                ground_ops: Some(metadata),
            }))),
        });
    }

    // Emit per-microsegment SegmentTrace rows for the Noise Segments
    // tab. One row per `(osm_id, segment_idx)` with the rolled-up
    // Lden contribution at this receiver and the microsegment polyline
    // for map highlight. Top-K cap is applied downstream by
    // `source-reader/lib.rs:apply_segment_top_k_with_cap` (subtype = 1
    // → ground bucket, 150 rows max).
    if let Some(t) = traces {
        emit_segment_traces(
            t,
            microsegs_by_id,
            &microseg_cache,
            n_days_f,
            ga_n_days_f,
            recv_lat,
            recv_lon,
            refl_db,
            osm_ref_lookup,
        );
    }
    out
}

/// One ground-ops microsegment as `((start_lat, start_lon), (end_lat, end_lon))` f32 pairs.
type LatLonSegment = ((f32, f32), (f32, f32));

fn multiline_geojson(segments: &[LatLonSegment]) -> String {
    if segments.is_empty() {
        return "{\"type\":\"MultiLineString\",\"coordinates\":[]}".to_string();
    }
    let mut s = String::with_capacity(64 * segments.len());
    s.push_str("{\"type\":\"MultiLineString\",\"coordinates\":[");
    for (i, ((a_lat, a_lon), (b_lat, b_lon))) in segments.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        // `write!` into the buffer avoids a transient String alloc
        // per segment that `format!` + `push_str` would do.
        let _ = write!(s, "[[{a_lon:.6},{a_lat:.6}],[{b_lon:.6},{b_lat:.6}]]");
    }
    s.push_str("]}");
    s
}

mod metadata;
mod traces;
use metadata::build_ground_ops_metadata;
use traces::emit_segment_traces;

#[cfg(test)]
mod tests;
