//! Synthetic aircraft sources shared by the integration tests that paint them.
//!
//! Two suites need the same rows: `aircraft_scatter_is_reproducible` (a painted tile carries the
//! same bytes whatever the core count) and `windowed_aircraft_paint_matches_the_whole_cell` (a
//! `tiles=` window changes neither the bytes nor what the cell admits). They must build their
//! sources the same way or the two claims are about different fixtures.

use noise_compute::compute::aircraft_v6::views::{BBox, SubSegmentSlice};
use noise_compute::compute::aircraft_v6::{AirborneRowView, CruiseRowView};

/// Flat ground under every fixture receiver and every fixture sub-segment endpoint.
pub const TERRAIN_M: f32 = 300.0;

/// A reproducible per-source offset. The sources have to land at COMPARABLE energies with
/// differing low bits — a wide spread would be order-independent for the opposite reason (a term
/// below `max * 2^-24` is a no-op wherever it is added).
pub fn jitter(i: usize, salt: u64) -> f64 {
    ((i as u64).wrapping_mul(2_654_435_761).wrapping_add(salt) % 997) as f64 * 1.0e-5
}

/// One cruise bucket at `(lat, lon)`. `near` puts it under the far-field gate, so it takes the
/// exact per-pixel path instead of broadcasting onto the coarse lattice.
pub fn cruise_row(index: usize, lat: f64, lon: f64, near: bool) -> CruiseRowView<'static> {
    let cell = h3o::LatLng::new(lat, lon)
        .expect("fixture lat/lon")
        .to_cell(h3o::Resolution::Seven);
    CruiseRowView {
        r7_hex: u64::from(cell),
        class: 0,
        rep_profile_idx: 0,
        fl_bin: 0,
        period: (index % 3) as u8,
        sum_length_m: 40_000.0 + (index % 97) as f32,
        rep_len_m: 900.0 + (index % 31) as f32,
        rep_alt_m: if near { 4_000.0 } else { 11_000.0 },
        rep_speed_kt: 430.0 + (index % 41) as f32,
        source_id: 0,
        origin: 0,
        unique_count: 1,
        top_candidates: &[],
    }
}

/// Per-flight sub-segment columns, in `SubSegmentSlice` order:
/// `[start_lat, start_lon, start_alt_m, end_lat, end_lon, end_alt_m]`. Owned by the test because
/// the row views borrow them.
pub type FlightColumns = [Vec<f32>; 6];

/// One flight per index, one sub-segment per entry of `offset_deg` — the sub-segment starts that
/// far north-east of `(centre_lat, centre_lon)` and runs 0.01 degrees further, at `altitude_m`.
pub fn flight_columns(
    flights: usize,
    offset_deg: &[f64],
    altitude_m: impl Fn(usize, usize) -> f32,
    centre_lat: f64,
    centre_lon: f64,
) -> Vec<FlightColumns> {
    (0..flights)
        .map(|i| {
            let axis = |base: f64, extra: f64, salt: u64| -> Vec<f32> {
                offset_deg
                    .iter()
                    .map(|d| (base + d + extra + jitter(i, salt)) as f32)
                    .collect()
            };
            let alt: Vec<f32> = (0..offset_deg.len()).map(|b| altitude_m(i, b)).collect();
            [
                axis(centre_lat, 0.0, 11),
                axis(centre_lon, 0.0, 23),
                alt.clone(),
                axis(centre_lat, 0.01, 17),
                axis(centre_lon, 0.01, 29),
                alt,
            ]
        })
        .collect()
}

/// The per-sub-segment scalar columns every fixture shares. `flags & 1` = departure; the terrain
/// elevations are the tile's, so the endpoint ground-stale gate passes.
pub struct SubSegmentScalars {
    pub period: Vec<u8>,
    pub date_id: Vec<i16>,
    pub flags: Vec<u8>,
    pub speed_kt: Vec<f32>,
    pub length_m: Vec<f32>,
    pub terrain_elev_m: Vec<f32>,
}

impl SubSegmentScalars {
    pub fn new(sub_segments: usize) -> Self {
        Self {
            period: (0..sub_segments).map(|i| (i % 3) as u8).collect(),
            date_id: vec![10; sub_segments],
            flags: vec![1; sub_segments],
            speed_kt: vec![220.0; sub_segments],
            length_m: vec![1_500.0; sub_segments],
            terrain_elev_m: vec![TERRAIN_M; sub_segments],
        }
    }
}

pub fn airborne_rows<'a>(
    columns: &'a [FlightColumns],
    scalars: &'a SubSegmentScalars,
    bbox: BBox,
) -> Vec<AirborneRowView<'a>> {
    columns
        .iter()
        .enumerate()
        .map(|(i, flight)| AirborneRowView {
            flight_id: noise_compute::flight_id::pack_synth(i as u64),
            callsign: "TEST",
            aircraft_type: *b"A320",
            profile_idx: (i % 8) as u8,
            source_id: 0,
            origin: 0,
            sub_segments: SubSegmentSlice {
                start_lat: &flight[0],
                start_lon: &flight[1],
                start_alt_m: &flight[2],
                end_lat: &flight[3],
                end_lon: &flight[4],
                end_alt_m: &flight[5],
                speed_kt: &scalars.speed_kt,
                length_m: &scalars.length_m,
                period: &scalars.period,
                date_id: &scalars.date_id,
                flags: &scalars.flags,
                terrain_start_elev_m: &scalars.terrain_elev_m,
                terrain_end_elev_m: &scalars.terrain_elev_m,
            },
            bbox,
        })
        .collect()
}
