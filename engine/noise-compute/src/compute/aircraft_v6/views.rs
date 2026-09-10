//! Typed column views over v6 popup arrow rows. These deliberately
//! borrow plain Rust slices (`&[f32]` etc.) so noise-compute keeps
//! zero arrow / IPC dependencies — source-reader extracts the slices
//! from `Arc<RecordBatch>` clones and hands them off here.

/// Flight identity columns of one prepared `airborne.arrow` — the values of its
/// `flight` dictionary column, decoded once per file; a row's `flight_key`
/// indexes them. Strings stay the Arrow offsets/bytes pair, so joining the
/// identity of a contributing flight allocates nothing until the accumulator
/// copies its callsign.
#[derive(Clone, Copy, Debug)]
pub struct AirborneFlightTable<'a> {
    pub callsign_offsets: &'a [i32],
    pub callsign_bytes: &'a [u8],
    /// Four `\0`-padded ICAO typecode bytes per flight.
    pub aircraft_type: &'a [u8],
    pub profile_idx: &'a [u8],
    pub source_id: &'a [u8],
    pub origin: &'a [u8],
}

impl AirborneFlightTable<'_> {
    pub fn len(&self) -> usize {
        self.profile_idx.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn callsign(&self, key: usize) -> &str {
        let bytes = &self.callsign_bytes
            [self.callsign_offsets[key] as usize..self.callsign_offsets[key + 1] as usize];
        std::str::from_utf8(bytes).expect("Arrow validated the callsign column as UTF-8")
    }

    pub fn aircraft_type(&self, key: usize) -> [u8; 4] {
        self.aircraft_type[key * 4..key * 4 + 4]
            .try_into()
            .expect("four typecode bytes per flight")
    }
}

/// One decoded record batch of `airborne.arrow`: flattened sub-segment rows
/// (every slice has `len()` entries; row `i` is fully described by index `i`)
/// and the flight table of the file the batch came from. A sub-segment is
/// stored once, in the square owning its midpoint, so a flight's rows can
/// arrive from several squares and are accumulated by `flight_id`.
///
/// `terrain_start_elev_m` and `terrain_end_elev_m` propagate Stage 1's
/// endpoint samples (linearly interpolated at split points); the airborne
/// path uses them for endpoint checks and stores no intermediate terrain.
#[derive(Clone, Copy, Debug)]
pub struct AirborneSegmentBatch<'a> {
    pub flight_id: &'a [u64],
    /// Index into `flights`.
    pub flight_key: &'a [i32],
    pub flights: AirborneFlightTable<'a>,
    pub start_gy: &'a [i32],
    pub start_gx: &'a [i32],
    pub start_alt_m: &'a [i16],
    pub end_gy: &'a [i32],
    pub end_gx: &'a [i32],
    pub end_alt_m: &'a [i16],
    pub speed_kt: &'a [f32],
    pub length_m: &'a [f32],
    pub period: &'a [u8],
    pub date_id: &'a [i16],
    pub flags: &'a [u8],
    pub terrain_start_elev_m: &'a [i16],
    pub terrain_end_elev_m: &'a [i16],
}

impl AirborneSegmentBatch<'_> {
    pub fn len(&self) -> usize {
        self.flight_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn start_lat_lon(&self, index: usize) -> [f32; 2] {
        Self::lat_lon(self.start_gx[index], self.start_gy[index])
    }

    pub fn end_lat_lon(&self, index: usize) -> [f32; 2] {
        Self::lat_lon(self.end_gx[index], self.end_gy[index])
    }

    fn lat_lon(gx: i32, gy: i32) -> [f32; 2] {
        let (x, y) = grid::grid_to_meters(gx, gy);
        let (lon, lat) = grid::poly::meters_to_lonlat(x, y);
        // Preserve the prepared popup's f32 geometry before the f64 kernel.
        [lat as f32, lon as f32]
    }
}

/// Sub-segment rows across every loaded batch.
pub fn airborne_row_count(batches: &[AirborneSegmentBatch<'_>]) -> usize {
    batches.iter().map(AirborneSegmentBatch::len).sum()
}

/// Number of GSE noise classes (LIGHT / MEDIUM / HEAVY). Mirrored
/// here so the popup view + heatmap loader pick up array sizes
/// without depending on `emission::gse` for a pure data shape.
pub const NUM_GSE_CLASSES: usize = 3;

/// One row of `airport_traffic.arrow` v7. Per-band raw Σ over n_days
/// of linear Z-weighted source energy, with **per-`veh_kind` storage
/// semantics** (consumer branches in the receiver formula):
///  - aircraft rows store density-weighted per-metre `LW'`
///    (= LW'_lin × overlap / line.length_m), applied at receiver via
///    CNOSSOS-EU §2.5.5 `+ 10·log10(θ / d_perp)` over the full
///    microsegment geometry. Refinement invariance follows by Chasles
///    (`Σ θᵢ = θ_total` across collinear sub-segments).
///  - GSE rows store per-event SEL@25m from the kinematic moving-point
///    integral, applied at receiver via point-source divergence
///    `+ 10·log10(25 / d_endpoint)`.
///
/// v7 replaces v6's "absolute SEL@25m for aircraft + FLC-delta at
/// receiver" formulation, which violated refinement invariance: the
/// same physical microsegment split into N collinear sub-segments
/// produced different received Lden (LKPR runway 4052652 showed a
/// 10× discrepancy). The per-metre `LW'`
/// formulation matches CNOSSOS-EU Directive 2015/996 Annex II §2.5.5.
///
/// Scalar `unique_*_count` counters plus row-replicated per-microsegment
/// UNION `microseg_unique_*` counts remain unchanged from v6.
/// Airport-level UNION counts live in the traffic file's footer metadata
/// (`qm_airport_summaries`, decoded into
/// [`crate::compute::aircraft_v6::airport_traffic::AirportSummaryEntry`]).
#[derive(Clone, Copy, Debug)]
pub struct AirportTrafficRowView<'a> {
    pub airport_key: &'a str,
    pub osm_id: u64,
    pub segment_idx: u16,
    pub geometry_kind: u8,
    pub start_lat: f32,
    pub start_lon: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    pub length_m: f32,
    pub ops_kind: u8,
    pub is_departure: u8,
    pub veh_kind: u8,
    pub class_idx: u8,
    pub period: u8,
    pub band_energy_lin: &'a [f32; 8],
    /// Distinct fids that crossed this row, regardless of
    /// `ops_kind / is_departure / veh_kind`.
    pub unique_movement_count: u32,
    /// Distinct fids on rows keyed as runway-roll arrivals; zero
    /// elsewhere.
    pub unique_arr_count: u32,
    /// Analogous for departures.
    pub unique_dep_count: u32,
    /// Per-GSE-class distinct fids; populated only on `veh_kind=1`
    /// rows.
    pub unique_gse_count_per_class: &'a [u32; NUM_GSE_CLASSES],
    /// UNION across ALL rows of this `(osm_id, segment_idx)`. v9: these
    /// three count NON-GA-class fids only — the GA-class union is below.
    /// Same value on every row of the microsegment — popup reads first
    /// row's value to populate per-microseg observed_movements, dividing
    /// each window by its own day count.
    pub microseg_unique_count: u32,
    pub microseg_unique_arr_count: u32,
    pub microseg_unique_dep_count: u32,
    pub microseg_unique_gse_count_per_class: &'a [u32; NUM_GSE_CLASSES],
    /// v9 GA-class full-year-window microseg UNION split. Zero on
    /// non-hybrid extracts.
    pub microseg_unique_ga_count: u32,
    pub microseg_unique_ga_arr_count: u32,
    pub microseg_unique_ga_dep_count: u32,
}

/// One entry of `top_candidates` (v14). Identity + ranking dimension
/// only; row-constant fields (period / date_id) live on the parent
/// row. `peak_lmax_25m_db` is the source-side NPD `lookup_lmax` at
/// the 25 m anchor (rev 2 ranking dimension); `altitude_m` is the
/// segment's altitude at peak so the popup can recompute CPA / slant
/// against the receiver.
#[derive(Clone, Debug)]
pub struct CruiseTopCandidateView<'a> {
    pub flight_id: u64,
    pub callsign: &'a str,
    pub aircraft_type: &'a [u8; 4],
    pub peak_lmax_25m_db: f32,
    pub altitude_m: f32,
}

/// One row of `cruise.arrow` v14. Grid-cell bucket aggregating
/// `sum_length_m` of cruise track at altitude `rep_alt_m`. v14 replaces
/// v13's per-fid lists with a bounded top-K `top_candidates` (ranked by
/// source-side peak Lmax at 25 m) + scalar `unique_count`.
///
/// `lon` / `lat` is the bucket's explicit centroid (degrees) — the grid
/// transfer replaced the old R7 hex id with plain coordinates. Popup
/// `band_stats` walks `top_candidates` into a per-fid HashMap
/// for dedup across grid cells. Tail fids outside the top-K cap drop
/// out of band counters; documented regression.
#[derive(Clone, Debug)]
pub struct CruiseRowView<'a> {
    pub lon: f64,
    pub lat: f64,
    pub class: u8,
    pub rep_profile_idx: u8,
    pub fl_bin: u8,
    pub period: u8,
    pub sum_length_m: f32,
    pub rep_len_m: f32,
    pub rep_alt_m: f32,
    pub rep_speed_kt: f32,
    pub source_id: u8,
    pub origin: u8,
    /// Number of distinct real fids that contributed to this bucket.
    /// Display-only — band counter dedup uses `top_candidates` instead.
    pub unique_count: u32,
    /// Bounded top-K identity slice ranked by `peak_lmax_25m_db` desc.
    pub top_candidates: &'a [CruiseTopCandidateView<'a>],
}
