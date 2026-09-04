//! Per-row trace builders for the airborne + cruise aircraft
//! sub-types (`SegmentTrace` `aircraft_subtype` 2 = airborne
//! sub-segment, 3 = cruise grid cell). Ground (subtype 1) traces are
//! emitted by `compute::aircraft_v6::airport_traffic`.

use crate::emission::aircraft::{typecode_to_string, PERIOD_SECONDS};
use crate::types::{
    CruiseBucketBreakdown, CruiseCellTopFlight, EmissionTrace, LayerKind, PropagationVariants,
    SegmentTrace,
};

use super::variants_to_lden;

fn aircraft_period_variants(period_energies: [f64; 3], n_days: f64) -> [PropagationVariants; 3] {
    aircraft_period_variants_with_effects(
        period_energies,
        period_energies,
        period_energies,
        period_energies,
        n_days,
    )
}

fn aircraft_period_variants_with_effects(
    period_energies: [f64; 3],
    free_period_energies: [f64; 3],
    no_terrain_period_energies: [f64; 3],
    no_screening_period_energies: [f64; 3],
    n_days: f64,
) -> [PropagationVariants; 3] {
    std::array::from_fn(|i| {
        let normalize = |energy: f64| {
            if n_days > 0.0 {
                energy / (n_days * PERIOD_SECONDS[i])
            } else {
                0.0
            }
        };
        PropagationVariants {
            full_energy: normalize(period_energies[i]),
            free_field_energy: normalize(free_period_energies[i]),
            no_terrain_energy: normalize(no_terrain_period_energies[i]),
            no_screening_energy: normalize(no_screening_period_energies[i]),
            ..Default::default()
        }
    })
}

/// Inputs for an airborne sub-segment trace.
pub struct BuildAircraftAirborneSubSegmentTrace<'a> {
    pub callsign: &'a str,
    pub aircraft_type: &'a [u8; 4],
    pub class_name: &'static str,
    pub flight_id: u64,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub cpa_distance_m: f64,
    pub altitude_m_at_cpa: f64,
    pub d_slant_m: f64,
    /// From Stage 2A sub-segment `flags & 0b001`.
    pub is_departure: bool,
    /// Linear-domain event energy per period `[day, evening, night]`
    /// (active period holds `10^(SEL/10)`, others zero).
    pub period_energies: [f64; 3],
    /// Same energy before receiver-side terrain/building screening.
    pub free_period_energies: [f64; 3],
    /// Same energy with terrain screening removed.
    pub no_terrain_period_energies: [f64; 3],
    /// Same energy with building screening removed.
    pub no_screening_period_energies: [f64; 3],
    pub n_days: f64,
    /// Doc 29 Eq. 4-8b decomposition from the kernel evaluation. CFFK
    /// fast path (slant > 7.62 km) populates `lambda_db = 0.0` and
    /// `delta_i_db = 0.0` per Doc 29 §A.2.7 and sets `cffk_fast_path = true`.
    pub doc29: crate::types::Doc29Breakdown,
}

pub fn build_aircraft_airborne_subsegment_trace(
    inputs: BuildAircraftAirborneSubSegmentTrace<'_>,
) -> SegmentTrace {
    let typecode_str = typecode_to_string(inputs.aircraft_type);
    let variants = aircraft_period_variants_with_effects(
        inputs.period_energies,
        inputs.free_period_energies,
        inputs.no_terrain_period_energies,
        inputs.no_screening_period_energies,
        inputs.n_days,
    );
    let (icao_hex, start_unix) = crate::flight_id::icao_hex_and_start_unix(inputs.flight_id);
    // Title identity preference: airline callsign → ICAO hex →
    // typecode-only. Callsign-less broadcasts (general aviation,
    // military, transponder hex-only modes) used to land in the list
    // as bare "B738" / "C172" rows with no way to tell them apart;
    // promote the icao_hex into the title so two GA flights of the
    // same type stay visibly distinct. Synthetic fids carry no hex
    // and fall through to typecode-only as before.
    let title = if !inputs.callsign.is_empty() {
        format!("{} ({typecode_str})", inputs.callsign)
    } else if !icao_hex.is_empty() {
        // `icao24_to_hex_lower` formats `{:06x}` so the chars are pure
        // ASCII hex — `to_ascii_uppercase` skips the Unicode tables
        // `to_uppercase` would otherwise walk.
        format!("{} ({typecode_str})", icao_hex.to_ascii_uppercase())
    } else {
        typecode_str.clone()
    };
    SegmentTrace {
        kind: LayerKind::Aircraft,
        osm_id: None,
        segment_idx: 0,
        name: title,
        subtype: "airborne".to_string(),
        is_dominant_of_group: false,
        start_lat: inputs.start_lat,
        start_lon: inputs.start_lon,
        end_lat: inputs.end_lat,
        end_lon: inputs.end_lon,
        cp_lat: 0.5 * (inputs.start_lat + inputs.end_lat),
        cp_lon: 0.5 * (inputs.start_lon + inputs.end_lon),
        length_m: 0.0,
        dist_m: inputs.cpa_distance_m,
        d_slant_m: inputs.d_slant_m,
        bridge: false,
        tunnel: false,
        emission: EmissionTrace::AircraftAirborne {
            class: inputs.class_name,
            callsign: inputs.callsign.to_string(),
            aircraft_type: typecode_str,
            cpa_distance_m: inputs.cpa_distance_m,
            altitude_m_at_cpa: inputs.altitude_m_at_cpa,
            is_departure: inputs.is_departure,
            icao_hex,
            start_unix,
        },
        propagation: crate::types::PropagationBreakdown::Doc29(inputs.doc29),
        received_lden: variants_to_lden(&variants),
        aircraft_subtype: 2,
        polyline: None,
        cell_polygon: None,
        cruise_buckets: None,
        cruise_top_flights: None,
        length_m_per_kind: None,
    }
}

/// Inputs for a cruise grid-cell aggregate trace.
pub struct BuildAircraftCruiseCellTrace {
    /// Explicit centroid (degrees) of the aggregated cell.
    pub lon: f64,
    pub lat: f64,
    pub n_unique_flights: u32,
    pub rep_alt_m: f32,
    pub d_slant_m: f64,
    /// Linear-domain event-energy sum per period `[day, evening, night]`
    /// across every cruise row contributing to this grid cell.
    pub period_energies: [f64; 3],
    pub n_days: f64,
    pub cruise_buckets: Vec<CruiseBucketBreakdown>,
    pub cruise_top_flights: Vec<CruiseCellTopFlight>,
    /// Doc 29 breakdown for the representative sub-segment (loudest
    /// contributor to this cell's slant). CFFK fast path at cruise
    /// altitudes — `lambda_db` and `delta_i_db` are typically 0.0 since
    /// FL250+ implies slant ≥ 7.62 km.
    pub doc29: crate::types::Doc29Breakdown,
}

pub fn build_aircraft_cruise_cell_trace(inputs: BuildAircraftCruiseCellTrace) -> SegmentTrace {
    // Display + emission label is the z9 square name of the centroid
    // (e.g. `z9/276/173`).
    let square = grid::square_name(grid::square_of(inputs.lat, inputs.lon));
    let display_name = format!("Cruise over {square}");
    let cell_polygon = cruise_cell_polygon(inputs.lat, inputs.lon);
    let variants = aircraft_period_variants(inputs.period_energies, inputs.n_days);

    SegmentTrace {
        kind: LayerKind::Aircraft,
        osm_id: None,
        segment_idx: 0,
        name: display_name,
        subtype: "cruise".to_string(),
        is_dominant_of_group: false,
        start_lat: inputs.lat,
        start_lon: inputs.lon,
        end_lat: inputs.lat,
        end_lon: inputs.lon,
        cp_lat: inputs.lat,
        cp_lon: inputs.lon,
        length_m: 0.0,
        dist_m: 0.0,
        d_slant_m: inputs.d_slant_m,
        bridge: false,
        tunnel: false,
        emission: EmissionTrace::AircraftCruise {
            square,
            n_unique_flights: inputs.n_unique_flights,
            rep_alt_m: inputs.rep_alt_m,
        },
        propagation: crate::types::PropagationBreakdown::Doc29(inputs.doc29),
        received_lden: variants_to_lden(&variants),
        aircraft_subtype: 3,
        polyline: None,
        cell_polygon: Some(cell_polygon),
        cruise_buckets: Some(inputs.cruise_buckets),
        cruise_top_flights: Some(inputs.cruise_top_flights),
        length_m_per_kind: None,
    }
}

/// Display polygon for a cruise grid-cell aggregate: small axis-aligned
/// box around the centroid with 2 km half-side. Last vertex equals the
/// first to close the GeoJSON ring. Display-only; the kernel never reads it.
fn cruise_cell_polygon(lat: f64, lon: f64) -> Vec<(f64, f64)> {
    const HALF_SIDE_M: f64 = 2000.0;
    let d_lat = HALF_SIDE_M / crate::constants::M_PER_DEG_LAT;
    let d_lon = HALF_SIDE_M / crate::constants::m_per_deg_lon(lat.to_radians());
    vec![
        (lat - d_lat, lon - d_lon),
        (lat - d_lat, lon + d_lon),
        (lat + d_lat, lon + d_lon),
        (lat + d_lat, lon - d_lon),
        (lat - d_lat, lon - d_lon),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emission::aircraft::period_leq;

    fn build_airborne(period_energies: [f64; 3], n_days: f64) -> SegmentTrace {
        build_aircraft_airborne_subsegment_trace(BuildAircraftAirborneSubSegmentTrace {
            callsign: "TEST",
            aircraft_type: b"A320",
            class_name: "Jet medium",
            // Synth fid so `icao_hex_and_start_unix` returns (empty, None),
            // exercising the same trace-emission branch real synth IDs take.
            flight_id: crate::flight_id::pack_synth(1),
            start_lat: 50.0,
            start_lon: 14.0,
            end_lat: 50.001,
            end_lon: 14.001,
            cpa_distance_m: 500.0,
            altitude_m_at_cpa: 800.0,
            d_slant_m: 943.4,
            is_departure: false,
            period_energies,
            free_period_energies: period_energies,
            no_terrain_period_energies: period_energies,
            no_screening_period_energies: period_energies,
            n_days,
            doc29: crate::types::Doc29Breakdown {
                sel_npd_db: 0.0,
                delta_v_db: 0.0,
                delta_i_db: 0.0,
                lambda_db: 0.0,
                delta_f_db: 0.0,
                d_p_m: 500.0,
                lateral_m: 0.0,
                beta_deg: 90.0,
                seg_len_m: 0.0,
                d_bar_m: 500.0,
                installation: "wing",
                cffk_fast_path: false,
                screening_kind: "none",
                screening_db: 0.0,
            },
        })
    }

    /// Popup invariant: segments add up to the source aggregate. The
    /// END Lden mix is open-coded against EU 2002/49/EC — delegating to
    /// `variants_to_lden` would be circular (production path uses it).
    #[test]
    fn airborne_segments_energy_sum_to_source_aggregate() {
        let n_days = 7.0;
        let segs = [
            build_airborne([1.0e9, 0.0, 0.0], n_days),
            build_airborne([0.0, 5.0e8, 2.0e8], n_days),
        ];

        let segment_energy_sum: f64 = segs
            .iter()
            .map(|s| 10f64.powf(s.received_lden.full / 10.0))
            .sum();

        let total = [1.0e9, 5.0e8, 2.0e8];
        let ld = period_leq(total[0], n_days, PERIOD_SECONDS[0]);
        let le = period_leq(total[1], n_days, PERIOD_SECONDS[1]);
        let ln = period_leq(total[2], n_days, PERIOD_SECONDS[2]);
        let source_lden = 10.0
            * ((12.0 * 10f64.powf(ld / 10.0)
                + 4.0 * 10f64.powf((le + 5.0) / 10.0)
                + 8.0 * 10f64.powf((ln + 10.0) / 10.0))
                / 24.0)
                .log10();
        let source_energy = 10f64.powf(source_lden / 10.0);

        assert!(
            (segment_energy_sum - source_energy).abs() / source_energy < 1e-9,
            "segment Σ {:.6e} vs source {:.6e} (Lden segs={:.4} dB, source={:.4} dB)",
            segment_energy_sum,
            source_energy,
            10.0 * segment_energy_sum.log10(),
            source_lden
        );
    }

    #[test]
    fn single_day_event_matches_period_leq() {
        let s = build_airborne([1.0e9, 0.0, 0.0], 7.0);
        let ld = period_leq(1.0e9, 7.0, PERIOD_SECONDS[0]);
        let expected = 10.0 * (12.0 / 24.0 * 10f64.powf(ld / 10.0)).log10();
        assert!(
            (s.received_lden.full - expected).abs() < 1e-9,
            "received_lden.full={} expected={}",
            s.received_lden.full,
            expected
        );
    }

    /// `n_days = 0` must not produce NaN — popup would render junk.
    /// `−∞ dB` floor is the expected outcome (matches road / rail empty
    /// inputs via `lden_from_periods`).
    #[test]
    fn n_days_zero_returns_neg_inf_not_nan() {
        let s = build_airborne([1.0e9, 0.0, 0.0], 0.0);
        assert!(
            !s.received_lden.full.is_nan(),
            "received_lden.full={}",
            s.received_lden.full
        );
    }
}
