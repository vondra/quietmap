//! Unit tests for the airport_traffic (aircraft ground-ops) kernel — flat-ground
//! fixtures asserting energy folding, per-period counts, and GA-window math.
use super::*;

struct FlatGround;
impl RasterSampler for FlatGround {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn building_height(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        1.0
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

/// Zero-count scalars + zero-count GSE array — the common test
/// fixture for rows whose unique counts don't matter (we only
/// check that energy folds correctly).
const ZERO_GSE: [u32; NUM_GSE_CLASSES] = [0, 0, 0];

/// Uniform (non-hybrid) weight LUT — these tests predate the GA
/// hybrid, so every class weights 1.0 and the GA-split count columns
/// stay zero (degenerates to the legacy single-window math).
fn uniform_weights() -> crate::emission::aircraft::ClassWeights {
    crate::emission::aircraft::ClassWeights::uniform()
}

fn run_flat(
    receiver: &Receiver,
    rows: &[AirportTrafficRowView<'_>],
    n_days: u16,
) -> Vec<Contributor> {
    run(
        receiver,
        rows,
        n_days,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    )
}

fn make_row<'a>(bands: &'a [f32; 8]) -> AirportTrafficRowView<'a> {
    AirportTrafficRowView {
        airport_key: "LKPR",
        osm_id: 42,
        segment_idx: 0,
        geometry_kind: 0,
        start_lat: 50.105,
        start_lon: 14.250,
        end_lat: 50.105,
        end_lon: 14.252,
        length_m: 250.0,
        ops_kind: 1,
        is_departure: 1,
        veh_kind: 0,
        class_idx: 2,
        period: 0,
        band_energy_lin: bands,
        unique_movement_count: 0,
        unique_arr_count: 0,
        unique_dep_count: 0,
        unique_gse_count_per_class: &ZERO_GSE,
        microseg_unique_count: 0,
        microseg_unique_arr_count: 0,
        microseg_unique_dep_count: 0,
        microseg_unique_gse_count_per_class: &ZERO_GSE,
        microseg_unique_ga_count: 0,
        microseg_unique_ga_arr_count: 0,
        microseg_unique_ga_dep_count: 0,
    }
}

#[test]
fn metadata_populated_with_arr_dep_split_and_profile_mix() {
    // v5: arr/dep/observed counts come from the airport_summary
    // lookup (UNIONed across all R4s upstream). 2-day window,
    // 2 arrivals + 2 departures, B738 only.
    // Expectation:
    //   arrivals_per_day = 2/2 = 1.0
    //   departures_per_day = 2/2 = 1.0
    //   observed_movements_per_day = ops_count_per_kind[runway] / n_days = 4/2 = 2.0
    //   profile_mix: one entry (B738, share=1.0).
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let arr_row = AirportTrafficRowView {
        airport_key: "LKPR",
        osm_id: 1,
        segment_idx: 0,
        geometry_kind: 0,
        start_lat: 50.105,
        start_lon: 14.250,
        end_lat: 50.105,
        end_lon: 14.252,
        length_m: 250.0,
        ops_kind: GROUND_OPS_KIND_RUNWAY_ROLL,
        is_departure: 0,
        veh_kind: 0,
        class_idx: 2,
        period: 0,
        band_energy_lin: &bands,
        unique_movement_count: 2,
        unique_arr_count: 2,
        unique_dep_count: 0,
        unique_gse_count_per_class: &ZERO_GSE,
        microseg_unique_count: 4,
        microseg_unique_arr_count: 2,
        microseg_unique_dep_count: 2,
        microseg_unique_gse_count_per_class: &ZERO_GSE,
        microseg_unique_ga_count: 0,
        microseg_unique_ga_arr_count: 0,
        microseg_unique_ga_dep_count: 0,
    };
    let dep_row = AirportTrafficRowView {
        is_departure: 1,
        unique_arr_count: 0,
        unique_dep_count: 2,
        ..arr_row
    };
    let mut summary: AirportSummaryLookup = HashMap::new();
    summary.insert(
        "LKPR".to_string(),
        AirportSummaryEntry {
            arr_count: 2,
            dep_count: 2,
            gse_count_per_class: [0, 0, 0],
            ops_count_per_kind: [4, 0, 0],
            ..Default::default()
        },
    );
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run(
        &receiver,
        &[arr_row, dep_row],
        2,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        Some(&summary),
        None,
    );
    assert_eq!(out.len(), 1);
    let Some(SourceMetadata::Aircraft(md)) = &out[0].metadata else {
        panic!(
            "expected Some(SourceMetadata::Aircraft), got {:?}",
            out[0].metadata
        );
    };
    assert_eq!(md.variant, "ground_ops");
    let detail = md.ground_ops.as_ref().expect("ground_ops populated");
    assert!((detail.arrivals_per_day - 1.0).abs() < 1e-9);
    assert!((detail.departures_per_day - 1.0).abs() < 1e-9);
    assert!((detail.observed_movements_per_day - 2.0).abs() < 1e-9);
    assert_eq!(detail.gse_per_day, [0.0, 0.0, 0.0]);
    assert_eq!(detail.profile_mix.len(), 1);
    assert_eq!(detail.profile_mix[0].class, 2);
    assert_eq!(detail.profile_mix[0].rep_typecode, "B738");
    assert!((detail.profile_mix[0].share - 1.0).abs() < 1e-9);
    assert!((detail.runway_roll.observed_movements_per_day - 2.0).abs() < 1e-9);
    assert_eq!(detail.taxi.observed_movements_per_day, 0.0);
    assert_eq!(detail.apron_movement.observed_movements_per_day, 0.0);
    // taxi/apron had zero energy → silence() periods (not
    // -∞-filled which would break serde_json).
    assert!(!detail.taxi.periods.lden_db.is_finite());
    assert!(!detail.apron_movement.periods.lden_db.is_finite());
}

/// v5 internal contract: when the `airport_summary` lookup is
/// `None` here, the compute kernel returns zero arr/dep — it
/// must NOT silently fall back to per-row sum (which would over-
/// count rotations by 4-8× per Codex C4). The source-reader
/// layer above is the gatekeeper that turns "airport_traffic.arrow
/// rows present but sidecar missing" into a loud Err; this test
/// only pins the compute kernel's contract when the caller
/// explicitly passes None (e.g. heatmap validate_vs_popup
/// validator which has no sidecar dep).
#[test]
fn metadata_missing_summary_zeros_arr_dep_counts() {
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let mut row = make_row(&bands);
    row.unique_arr_count = 5;
    row.unique_dep_count = 5;
    row.microseg_unique_arr_count = 5;
    row.microseg_unique_dep_count = 5;
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row], 2);
    assert_eq!(out.len(), 1);
    let Some(SourceMetadata::Aircraft(md)) = &out[0].metadata else {
        panic!("metadata missing");
    };
    let detail = md.ground_ops.as_ref().unwrap();
    assert_eq!(detail.arrivals_per_day, 0.0, "no summary → zero arr count");
    assert_eq!(detail.departures_per_day, 0.0);
    assert_eq!(detail.observed_movements_per_day, 0.0);
}

#[test]
fn metadata_gse_per_day_populated() {
    // v5: gse_per_day comes from airport_summary lookup.
    let bands: [f32; 8] = [1e6; 8];
    let row = AirportTrafficRowView {
        airport_key: "LKPR",
        osm_id: 1,
        segment_idx: 0,
        geometry_kind: 0,
        start_lat: 50.105,
        start_lon: 14.250,
        end_lat: 50.105,
        end_lon: 14.252,
        length_m: 250.0,
        ops_kind: GROUND_OPS_KIND_RUNWAY_ROLL,
        is_departure: 0,
        veh_kind: 1,
        class_idx: 0,
        period: 0,
        band_energy_lin: &bands,
        unique_movement_count: 3,
        unique_arr_count: 0,
        unique_dep_count: 0,
        unique_gse_count_per_class: &[3, 0, 0],
        microseg_unique_count: 3,
        microseg_unique_arr_count: 0,
        microseg_unique_dep_count: 0,
        microseg_unique_gse_count_per_class: &[3, 0, 0],
        microseg_unique_ga_count: 0,
        microseg_unique_ga_arr_count: 0,
        microseg_unique_ga_dep_count: 0,
    };
    let mut summary: AirportSummaryLookup = HashMap::new();
    summary.insert(
        "LKPR".to_string(),
        AirportSummaryEntry {
            arr_count: 0,
            dep_count: 0,
            gse_count_per_class: [3, 0, 0],
            ops_count_per_kind: [0, 0, 0],
            ..Default::default()
        },
    );
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run(
        &receiver,
        &[row],
        2,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        Some(&summary),
        None,
    );
    assert_eq!(out.len(), 1);
    let Some(SourceMetadata::Aircraft(md)) = &out[0].metadata else {
        panic!("metadata missing");
    };
    let detail = md.ground_ops.as_ref().unwrap();
    assert!((detail.gse_per_day[0] - 1.5).abs() < 1e-9);
    assert_eq!(detail.gse_per_day[1], 0.0);
    assert_eq!(detail.gse_per_day[2], 0.0);
    // No aircraft → arrivals/departures/profile_mix zero.
    assert_eq!(detail.arrivals_per_day, 0.0);
    assert_eq!(detail.departures_per_day, 0.0);
    assert!(detail.profile_mix.is_empty());
}

#[test]
fn empty_rows_returns_no_contributors() {
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[], 1);
    assert!(out.is_empty());
}

#[test]
fn single_row_produces_one_contributor() {
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_row(&bands);
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row], 1);
    assert_eq!(out.len(), 1);
    assert!(out[0].periods.lden_db.is_finite());
    assert!(out[0].name.contains("LKPR"));
}

#[test]
fn zero_energy_skipped() {
    let bands: [f32; 8] = [0.0; 8];
    let row = make_row(&bands);
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row], 1);
    assert!(out.is_empty());
}

/// Hybrid LUT: GA classes → 365, airline → 12 (consumer divides by 12).
fn hybrid_weights() -> crate::emission::aircraft::ClassWeights {
    use crate::emission::aircraft::{is_ga_sampled_class, ClassWeights, NUM_CLASSES};
    let vec: String = (0..NUM_CLASSES)
        .map(|c| {
            if is_ga_sampled_class(c as u8) {
                "365"
            } else {
                "12"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    ClassWeights::parse(Some(&vec), 12).unwrap()
}

fn ground_lden(out: &[Contributor]) -> f64 {
    out[0].periods.lden_db
}

/// GSE rows (`veh_kind == 1`) MUST weight 1.0 even under the hybrid
/// LUT — their `class_idx` indexes the GSE class space and GSE is an
/// airline-pass artifact (`ga-365d-hybrid-plan.md` §2 / layer-ground.md
/// §4). Same row → identical received Lden under hybrid vs uniform.
#[test]
fn gse_row_weight_pinned_to_one_under_hybrid() {
    let bands: [f32; 8] = [1e6; 8];
    let mut row = make_row(&bands);
    row.veh_kind = 1;
    row.class_idx = 0; // GSE LIGHT
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let uni = run(
        &receiver,
        &[row],
        12,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let hyb = run(
        &receiver,
        &[row],
        12,
        &hybrid_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    assert_eq!(uni.len(), 1);
    assert_eq!(hyb.len(), 1);
    assert!(
        (ground_lden(&uni) - ground_lden(&hyb)).abs() < 1e-9,
        "GSE Lden must be identical under hybrid vs uniform (weight 1.0)"
    );
}

/// A GA-class aircraft ground row (`veh_kind == 0`, HELICOPTER) is
/// weighted 12/365 under the hybrid LUT → its received Lden drops
/// ~10·log10(365/12) ≈ 14.83 dB vs uniform. The Kytín ground-ops
/// correction.
#[test]
fn ga_aircraft_ground_row_weighted_down() {
    let bands: [f32; 8] = [1e6; 8];
    let mut row = make_row(&bands);
    row.veh_kind = 0;
    // HELICOPTER class (GA-sampled) — resolve by name so the test
    // survives a class-index re-sort.
    row.class_idx =
        crate::emission::aircraft::noise_class_of(crate::emission::aircraft::profile_idx("R44"));
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let uni = run(
        &receiver,
        &[row],
        12,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let hyb = run(
        &receiver,
        &[row],
        12,
        &hybrid_weights(),
        &FlatGround,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let drop = ground_lden(&uni) - ground_lden(&hyb);
    let expected = 10.0 * (365.0f64 / 12.0).log10();
    assert!(
        (drop - expected).abs() < 0.05,
        "GA ground Lden drop {drop:.2} dB != {expected:.2} dB"
    );
}

/// Guard against `A_WEIGHT_LIN` drifting out of sync with
/// `crate::constants::A_WEIGHTING` (the const table is a manual
/// transcription of `10^(A[i]/10)` — a future revision of
/// `A_WEIGHTING` must regenerate this table or this test fires).
#[test]
fn a_weight_lin_matches_constants() {
    use crate::constants::A_WEIGHTING;
    for (i, a_db) in A_WEIGHTING.iter().enumerate() {
        let expected = 10f64.powf(*a_db / 10.0);
        let got = A_WEIGHT_LIN[i];
        let rel_err = (got - expected).abs() / expected;
        assert!(
            rel_err < 1e-12,
            "A_WEIGHT_LIN[{i}] = {got} but 10^(A[{i}]={a_db}/10) = {expected} \
                 (rel_err = {rel_err:e})"
        );
    }
}

#[test]
fn far_receiver_pruned() {
    let bands: [f32; 8] = [1e10; 8];
    let row = make_row(&bands);
    // 50° away — well beyond any per-ops_kind reach (max 5 km runway).
    let receiver = Receiver {
        lat: 0.0,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row], 1);
    assert!(out.is_empty());
}

#[test]
fn rows_aggregate_by_airport_key() {
    let bands1: [f32; 8] = [1e6; 8];
    let bands2: [f32; 8] = [2e6; 8];
    let row1 = make_row(&bands1);
    let mut row2 = make_row(&bands2);
    row2.start_lon = 14.255;
    row2.end_lon = 14.257;
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.260,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row1, row2], 1);
    assert_eq!(out.len(), 1, "two rows same airport → one contributor");
}

#[test]
fn distinct_airports_two_contributors() {
    let bands: [f32; 8] = [1e6; 8];
    let row1 = make_row(&bands);
    let mut row2 = make_row(&bands);
    row2.airport_key = "LKKB";
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.260,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row1, row2], 1);
    assert_eq!(out.len(), 2);
}

/// Guards against re-introducing the per-row geometry push bug:
/// `airport_traffic.arrow` carries ~44 rows per microsegment
/// (period × veh_kind × class fan-out). The contributor geometry
/// must dedup by `(osm_id, segment_idx)` so the wire payload stays
/// proportional to unique taxiways, not row count.
#[test]
fn geometry_dedups_replicated_microseg_rows() {
    let bands: [f32; 8] = [1e6; 8];
    let row_p0 = make_row(&bands);
    let mut row_p1 = make_row(&bands);
    row_p1.period = 1;
    let mut row_p2 = make_row(&bands);
    row_p2.period = 2;
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.260,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let out = run_flat(&receiver, &[row_p0, row_p1, row_p2], 1);
    assert_eq!(out.len(), 1);
    let geom = out[0].geometry.as_ref().expect("geometry present");
    let coords = geom
        .get("coordinates")
        .and_then(|v| v.as_array())
        .expect("coords array");
    assert_eq!(coords.len(), 1, "3 rows of same microseg → 1 LineString");
}

/// When the OSM `ref` lookup carries a value for the microsegment's
/// osm_id, the SegmentTrace name swaps the generic ops_kind label
/// for "RWY {ref}" / "TWY {ref}". Synth osm_ids (top bit set) are
/// absent from the lookup by construction in source-reader.
#[test]
fn osm_ref_lookup_renames_runway_segment_trace() {
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_row(&bands); // osm_id=42, ops_kind=1 (RUNWAY_ROLL)
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let mut traces = crate::types::TraceCollector::new();
    let mut lookup: HashMap<u64, String> = HashMap::new();
    lookup.insert(42, "06/24".to_string());
    run(
        &receiver,
        &[row],
        1,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &lookup,
        None,
        Some(&mut traces),
    );
    let trace = traces
        .segments
        .iter()
        .find(|t| matches!(t.kind, LayerKind::Aircraft))
        .expect("aircraft SegmentTrace emitted");
    assert_eq!(trace.name, "LKPR RWY 06/24");
}

/// Taxiway arm of the match: ref="A" should render as "LKPR TWY A"
/// rather than the generic "LKPR taxi". Guards against accidental
/// arm reorder between RUNWAY_ROLL / TAXI / APRON_MOVEMENT.
#[test]
fn osm_ref_lookup_renames_taxi_segment_trace() {
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let mut row = make_row(&bands);
    row.ops_kind = GROUND_OPS_KIND_TAXI;
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let mut traces = crate::types::TraceCollector::new();
    let mut lookup: HashMap<u64, String> = HashMap::new();
    lookup.insert(42, "A".to_string());
    run(
        &receiver,
        &[row],
        1,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &lookup,
        None,
        Some(&mut traces),
    );
    let trace = traces
        .segments
        .iter()
        .find(|t| matches!(t.kind, LayerKind::Aircraft))
        .expect("aircraft SegmentTrace emitted");
    assert_eq!(trace.name, "LKPR TWY A");
}

/// Without a lookup entry for this osm_id, the SegmentTrace name
/// stays at the generic "{airport_key} runway-roll" form — keeping
/// the pre-#67 popup label for unmapped ways and synth airfields.
#[test]
fn osm_ref_lookup_missing_keeps_generic_label() {
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_row(&bands);
    let receiver = Receiver {
        lat: 50.105,
        lon: 14.255,
        elevation_m: 0.0,
        height_m: 4.0,
    };
    let mut traces = crate::types::TraceCollector::new();
    let empty: HashMap<u64, String> = HashMap::new();
    run(
        &receiver,
        &[row],
        1,
        &uniform_weights(),
        &FlatGround,
        &[],
        None,
        &empty,
        None,
        Some(&mut traces),
    );
    let trace = traces
        .segments
        .iter()
        .find(|t| matches!(t.kind, LayerKind::Aircraft))
        .expect("aircraft SegmentTrace emitted");
    assert_eq!(trace.name, "LKPR runway-roll");
}

/// Path-effect test geometry: source at (50.0, 14.0); receiver
/// ~800 m east at (50.0, 14.01122) — at lat 50°, 0.01122° lon
/// ≈ 802 m via the equirectangular convention used by
/// `propagation::geo::flat_dist`. Obstacle midpoint at
/// (50.0, 14.00561) is the centerline of source–receiver.
const PE_SRC_LAT: f64 = 50.0;
const PE_SRC_LON: f64 = 14.0;
const PE_RCV_LAT: f64 = 50.0;
const PE_RCV_LON: f64 = 14.01122;
const PE_MID_LAT: f64 = 50.0;
const PE_MID_LON: f64 = 14.00561;

/// Pyramid hill between source and receiver. Linear falloff inside
/// `hill_radius_m`, zero outside. `g` is the ground-factor returned
/// by `ground_g`; tests typically use `g = 1.0` (soft ground) so
/// `A_gr` is non-zero — that's how the max-rule kicker becomes
/// observable. No buildings, no vegetation.
struct MockHilly {
    hill_lat: f64,
    hill_lon: f64,
    hill_radius_m: f64,
    hill_height_m: f64,
    g: f64,
}
impl RasterSampler for MockHilly {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        let d = crate::propagation::geo::flat_dist(lat, lon, self.hill_lat, self.hill_lon);
        if d < self.hill_radius_m {
            self.hill_height_m * (1.0 - d / self.hill_radius_m)
        } else {
            0.0
        }
    }
    fn building_height(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        self.g
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

/// Tall obstacle on the centerline between source and receiver,
/// modeled via `building_height` (the path-profile sampler feeds
/// this into `screening_attenuation_with_meta`). Elevation is flat
/// so the diffraction is purely structural, not topographic.
struct MockBuildingScreen {
    obs_lat: f64,
    obs_lon: f64,
    obs_radius_m: f64,
    obs_height_m: f64,
    g: f64,
}
impl RasterSampler for MockBuildingScreen {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn building_height(&self, lat: f64, lon: f64) -> f64 {
        let d = crate::propagation::geo::flat_dist(lat, lon, self.obs_lat, self.obs_lon);
        if d < self.obs_radius_m {
            self.obs_height_m
        } else {
            0.0
        }
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        self.g
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

/// Build one AirportTrafficRowView placed at the path-effect
/// source position so all three tests share identical geometry.
/// `make_row(&bands)` is unsuitable — it pins the source near LKPR
/// for other tests' aggregation checks.
fn make_path_effect_row<'a>(bands: &'a [f32; 8]) -> AirportTrafficRowView<'a> {
    AirportTrafficRowView {
        airport_key: "LKPR",
        osm_id: 42,
        segment_idx: 0,
        geometry_kind: 0,
        start_lat: PE_SRC_LAT as f32,
        start_lon: PE_SRC_LON as f32,
        end_lat: PE_SRC_LAT as f32,
        end_lon: (PE_SRC_LON + 0.00001) as f32,
        length_m: 1.0,
        ops_kind: 1,
        is_departure: 1,
        veh_kind: 0,
        class_idx: 2,
        period: 0,
        band_energy_lin: bands,
        unique_movement_count: 0,
        unique_arr_count: 0,
        unique_dep_count: 0,
        unique_gse_count_per_class: &ZERO_GSE,
        microseg_unique_count: 0,
        microseg_unique_arr_count: 0,
        microseg_unique_dep_count: 0,
        microseg_unique_gse_count_per_class: &ZERO_GSE,
        microseg_unique_ga_count: 0,
        microseg_unique_ga_arr_count: 0,
        microseg_unique_ga_dep_count: 0,
    }
}

fn pe_receiver() -> Receiver {
    Receiver {
        lat: PE_RCV_LAT,
        lon: PE_RCV_LON,
        elevation_m: 0.0,
        height_m: 4.0,
    }
}

/// Extract the ground_ops metadata from a single-contributor run
/// result. Panics with a useful message if the result shape isn't
/// what the path-effect tests expect.
fn pe_ground_ops(out: &[Contributor]) -> &AircraftGroundOpsDetail {
    assert_eq!(out.len(), 1, "expected one airport contributor");
    match &out[0].metadata {
        Some(SourceMetadata::Aircraft(a)) => {
            a.ground_ops.as_ref().expect("ground_ops detail populated")
        }
        other => panic!("expected SourceMetadata::Aircraft, got {other:?}"),
    }
}

/// Hill between source and receiver drives `A_terr > 0`. With
/// soft ground (`g=1`) the per-band gob is `max(A_gr, A_terr)`.
/// Symptom of the pre-fix linear-sum bug: `Lden_full < Lden_full_iso`
/// because the (now-correctly-applied) max rule keeps fewer dB.
/// Test pins the post-fix behavior: `Lden_full` matches the
/// max-rule prediction and beats both extremes (no_terrain too
/// loud, no_ground too quiet — though no_ground only differs from
/// full when `A_bar > A_gr`).
#[test]
fn terrain_path_effect_engages_max_rule() {
    // 30 m pyramid hill at the midpoint; source/receiver heights
    // 4 m → flat-ground LOS sits at 4 m, hill obstructs.
    let rasters = MockHilly {
        hill_lat: PE_MID_LAT,
        hill_lon: PE_MID_LON,
        hill_radius_m: 200.0,
        hill_height_m: 30.0,
        g: 1.0,
    };
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_path_effect_row(&bands);
    let out = run(
        &pe_receiver(),
        &[row],
        1,
        &uniform_weights(),
        &rasters,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let md = pe_ground_ops(&out);
    // With a hill in the way, terrain_impact_db must be NEGATIVE
    // (lower full Lden vs no_terrain → terrain attenuates).
    assert!(
        md.terrain_impact_db < -0.5,
        "expected terrain_impact_db ≤ -0.5 (hill diffracts); got {}",
        md.terrain_impact_db
    );
    // ground_impact_db is small when A_bar already dominates A_gr.
    // |ground| < |terrain| confirms max-rule kept terrain, not sum.
    assert!(
        md.ground_impact_db.abs() < md.terrain_impact_db.abs(),
        "ground_impact ({}) should be smaller in magnitude than \
             terrain_impact ({}) — max-rule must keep A_bar",
        md.ground_impact_db,
        md.terrain_impact_db
    );
}

/// Tall building between source and receiver drives `A_scr > 0`.
/// Same max-rule assertion as the terrain test, but exercising the
/// screening leg of `A_bar = A_terr + A_scr`.
#[test]
fn screening_path_effect_engages_max_rule() {
    let rasters = MockBuildingScreen {
        obs_lat: PE_MID_LAT,
        obs_lon: PE_MID_LON,
        // Wide enough (~150 m) that at least one path-profile
        // sample lands inside — the path sampler steps in the
        // 20-80 m range and we want screening_attenuation to see
        // the obstacle even at the coarsest step.
        obs_radius_m: 150.0,
        obs_height_m: 30.0,
        g: 1.0,
    };
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_path_effect_row(&bands);
    let out = run(
        &pe_receiver(),
        &[row],
        1,
        &uniform_weights(),
        &rasters,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let md = pe_ground_ops(&out);
    // Building diffraction must drive screening_impact_db negative.
    assert!(
        md.screening_impact_db < -0.5,
        "expected screening_impact_db ≤ -0.5 (building diffracts); got {}",
        md.screening_impact_db
    );
    // Same max-rule guard as the terrain test, but with A_bar
    // = A_scr (no terrain). Under the max rule, A_bar dominates
    // → ground_impact ≈ 0. Under the additive bug, ground would
    // drop full Lden by ~1.7 dB (Codex-simulated). 0.5 dB
    // separates the two regimes cleanly.
    assert!(
        md.ground_impact_db.abs() < 0.5,
        "ground_impact_db {} suggests A_gr was summed onto A_scr \
             instead of max'd — same regression as max_rule_not_sum_rule.",
        md.ground_impact_db
    );
}

/// Regression guard for the linear-sum bug: pre-fix code was
/// `gob = A_gr + A_terr + A_scr` (additive). Post-fix it is
/// `gob = max(A_gr, A_terr + A_scr)`. With `g = 1` (soft ground)
/// and a hill, the full Lden under the additive bug would be
/// strictly lower than under the max rule by at least the soft-
/// ground band-mean `GROUND_CF · g` (~3–6 dB). This test pins the
/// max-rule Lden so any future re-introduction of the additive
/// stack would shift it down measurably.
#[test]
fn max_rule_not_sum_rule() {
    let rasters = MockHilly {
        hill_lat: PE_MID_LAT,
        hill_lon: PE_MID_LON,
        hill_radius_m: 200.0,
        hill_height_m: 30.0,
        g: 1.0,
    };
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_path_effect_row(&bands);
    let out = run(
        &pe_receiver(),
        &[row],
        1,
        &uniform_weights(),
        &rasters,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let md = pe_ground_ops(&out);
    // Under the max rule, ground_impact ≈ 0 (per-band identity:
    // both full and no_ground use a_bar when a_bar > a_gr).
    // Codex-simulated values: max-rule ground = +0.009 dB,
    // additive-bug ground = -1.87 dB. A 0.5 dB threshold
    // separates the two regimes cleanly. A LOOSER threshold (e.g.
    // 2 dB) would have let the additive bug pass — the prior
    // version of this test did and was reported by /gg.
    assert!(
        md.ground_impact_db.abs() < 0.5,
        "ground_impact_db {} indicates additive A_gr instead of \
             max(A_gr, A_bar). Hill is in the way → A_bar > A_gr → \
             gob should be ~A_bar regardless of A_gr.",
        md.ground_impact_db
    );
}

/// Complement to `max_rule_not_sum_rule`: covers the other branch
/// of the max-rule conditional — flat ground with soft `g=1` and
/// NO obstacle, so `A_bar = 0` and the rule falls through to
/// `gob = A_gr` directly (`if a_bar > 0.0 { … } else { a_gr }`).
/// Without this test, a future regression that drops the `else`
/// arm or returns `0.0` when `A_bar == 0` would silently zero
/// soft-ground attenuation on every flat-terrain receiver — the
/// common case near every airport runway.
#[test]
fn ground_only_when_no_obstacle() {
    // Flat raster: no hill, no building. Soft ground (g=1) so
    // A_gr is non-zero; A_bar = 0 + 0 = 0 → max rule short-circuits
    // to A_gr.
    let rasters = MockHilly {
        hill_lat: PE_MID_LAT,
        hill_lon: PE_MID_LON,
        hill_radius_m: 0.0, // disables hill — no elevation anywhere
        hill_height_m: 0.0,
        g: 1.0,
    };
    let bands: [f32; 8] = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
    let row = make_path_effect_row(&bands);
    let out = run(
        &pe_receiver(),
        &[row],
        1,
        &uniform_weights(),
        &rasters,
        &[],
        None,
        &HashMap::new(),
        None,
        None,
    );
    let md = pe_ground_ops(&out);
    // No obstacle → both terrain and screening impacts are zero.
    assert!(
        md.terrain_impact_db.abs() < 0.1,
        "expected near-zero terrain_impact (no hill); got {}",
        md.terrain_impact_db
    );
    assert!(
        md.screening_impact_db.abs() < 0.1,
        "expected near-zero screening_impact (no buildings); got {}",
        md.screening_impact_db
    );
    // Ground attenuation is the only path effect active. Removing
    // it (no_ground variant) must raise Lden vs full, so
    // ground_impact_db must be NEGATIVE and substantial — soft
    // ground over ~800 m on the dominant mid-frequency bands
    // gives a few dB. If a future change accidentally zeroed out
    // the `else { a_gr }` arm, ground_impact would collapse to
    // 0 and this test fires.
    assert!(
        md.ground_impact_db < -1.0,
        "expected ground_impact ≤ -1.0 dB on soft ground with no \
             obstacle (A_bar=0 → gob=A_gr); got {}",
        md.ground_impact_db
    );
}
