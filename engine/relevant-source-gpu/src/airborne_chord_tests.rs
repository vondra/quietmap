//! Canonical CPU scatter versus CUDA for whole-event floors and receiver-dependent chord topology.
use super::*;
use crate::airborne_chords::{ChordKey, ChordSource, ChordSources};
use noise_compute::{
    compute::aircraft_v6::{airborne, AirborneFlightTable, AirborneSegmentBatch},
    propagation::obstacle_index::ObstacleSet,
    types::RasterSampler,
};

struct Flat;
impl RasterSampler for Flat {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        1.0
    }
}

/// Start/end eastings in km at the equator, altitude, flags and flight id.
type Piece = (f64, f64, i16, u8, u64);

fn compare(pieces: &[Piece], receiver_lon: f64) -> Result<Vec<f32>> {
    let n = pieces.len();
    let point =
        |east: f64| grid::lonlat_to_grid(receiver_lon + east * 1000.0 / air::M_PER_DEG_LAT, 0.0);
    let start: Vec<_> = pieces.iter().map(|p| point(p.0)).collect();
    let end: Vec<_> = pieces.iter().map(|p| point(p.1)).collect();
    let sx: Vec<_> = start.iter().map(|p| p.0).collect();
    let sy: Vec<_> = start.iter().map(|p| p.1).collect();
    let ex: Vec<_> = end.iter().map(|p| p.0).collect();
    let ey: Vec<_> = end.iter().map(|p| p.1).collect();
    let alt: Vec<_> = pieces.iter().map(|p| p.2).collect();
    let flags: Vec<_> = pieces.iter().map(|p| p.3 | airborne::SPLIT_PIECE).collect();
    let ids: Vec<_> = pieces.iter().map(|p| p.4).collect();
    let keys = vec![0; n];
    let speed = vec![250.0; n];
    let length = vec![1000.0; n];
    let periods: Vec<_> = (0..n).map(|i| (i % 3) as u8).collect();
    let zero = vec![0; n];
    let profile = [air::profile_idx("B738")];
    let batch = AirborneSegmentBatch {
        flight_id: &ids,
        flight_key: &keys,
        flights: AirborneFlightTable {
            callsign_offsets: &[0, 1],
            callsign_bytes: b"T",
            aircraft_type: b"B738",
            profile_idx: &profile,
            source_id: &[2],
            origin: &[0],
        },
        start_gx: &sx,
        start_gy: &sy,
        end_gx: &ex,
        end_gy: &ey,
        start_alt_m: &alt,
        end_alt_m: &alt,
        speed_kt: &speed,
        length_m: &length,
        period: &periods,
        date_id: &zero,
        flags: &flags,
        terrain_start_elev_m: &zero,
        terrain_end_elev_m: &zero,
    };
    let sources = (0..n)
        .map(|i| ChordSource::prepare(&batch, i))
        .collect::<Result<Vec<_>>>()?;
    let graph: Vec<_> = (0..n)
        .map(|i| ChordKey {
            flight: ids[i],
            start: start[i],
            end: end[i],
            flags: flags[i],
        })
        .collect();
    let host = ChordSources::new(sources, &graph)?;
    let weights = air::SamplingWindow {
        baseline_days: 364,
        increment_days: 12,
        baseline_days_sha256: "fixture".into(),
        increment_days_sha256: "fixture".into(),
    }
    .provenance_weights();
    let _cuda = RelevantSourceCuda::initialize()?;
    let device = chords::DeviceChords::new(&host, &weights)?;
    let receivers = [0.0, 0.001, -0.001].map(|lat| {
        ReceiverScreening::build(
            lat,
            receiver_lon,
            4.0,
            &Flat,
            &ObstacleSet { indexes: vec![] },
        )
        .unwrap()
    });
    let uploaded = UploadedScreen::new(&receivers)?;
    let actual = device.powers(&uploaded, 364)?;
    assert_eq!(
        actual,
        device.powers(&uploaded, 364)?,
        "repeat must keep power bytes"
    );
    for (i, rx) in receivers.iter().enumerate() {
        let mut flights: Vec<_> = airborne::scatter(
            &rx.receiver,
            &[batch],
            364.0,
            &weights,
            &rx.terrain,
            Some(&rx.buildings),
            0,
            None,
        )
        .into_iter()
        .collect();
        flights.sort_unstable_by_key(|(id, _)| *id);
        for period in 0..3 {
            let expected = flights
                .iter()
                .map(|(_, flight)| flight.period_energy[period])
                .sum::<f64>()
                / (364.0 * air::PERIOD_SECONDS[period]);
            let observed = f64::from(actual[i * 3 + period]);
            let error = if expected == 0.0 && observed == 0.0 {
                0.0
            } else {
                (10.0 * (observed / expected).log10()).abs()
            };
            assert!(
                error < 0.01,
                "receiver={i} period={period}: {observed} vs {expected}, {error} dB"
            );
        }
    }
    Ok(actual)
}

#[test]
fn chord_event_floor_is_after_piece_sum() -> Result<()> {
    // 12–17 km (not 11–16): level-approach thrust interpolates above the old
    // min NPD row, so the floor boundary moved ~1 km out (mirrors the CPU
    // split_chord_takes_the_event_floor_as_one_event fixture).
    let start = airborne::CHORD_START;
    let together = compare(&[(12.0, 14.5, 50, start, 42), (14.5, 17.0, 50, 0, 42)], 0.0)?;
    let separate = compare(
        &[(12.0, 14.5, 50, start, 42), (14.5, 17.0, 50, start, 42)],
        0.0,
    )?;
    assert!(together[..3].iter().any(|v| *v > 0.0));
    assert!(separate[..3].iter().all(|v| *v == 0.0));
    Ok(())
}

#[test]
fn rejected_pieces_duplicate_endpoints_cycles_and_seams_match_cpu() -> Result<()> {
    let start = airborne::CHORD_START;
    let secondary = air::SEGMENT_FLAG_SECONDARY_ONLY;
    for pieces in [
        vec![
            (0.0, 1.0, 100, start, 42),
            (1.0, 2.0, 30000, 0, 42),
            (2.0, 3.0, 100, secondary, 42),
        ],
        vec![
            (0.0, 1.0, 30000, start, 42),
            (0.0, 1.0, 100, start, 42),
            (1.0, 2.0, 100, secondary, 42),
        ],
        vec![
            (0.0, 1.0, 100, 0, 42),
            (1.0, 0.0, 100, 0, 42),
            (2.0, 3.0, 100, start, 43),
        ],
        vec![
            (1.0, 2.0, 100, 0, 42),
            (0.0, 1.0, 100, start, 42),
            (2.0, 3.0, 100, 0, 42),
        ],
        vec![(0.0, 1.0, 100, start, 42), (1.0, 2.0, 100, 0, 43)],
        vec![(0.0, 1.0, 0, start, 42), (1.0, 2.0, 100, 0, 42)],
    ] {
        compare(&pieces, 0.0)?;
        compare(&pieces, 179.995)?;
    }
    Ok(())
}

#[test]
fn chord_reduction_spans_parts_and_keeps_period_weights() -> Result<()> {
    let pieces: Vec<_> = (0..8193)
        .map(|i| {
            (
                0.0,
                1.0,
                100,
                airborne::CHORD_START
                    | if i % 2 == 0 {
                        air::SEGMENT_FLAG_SECONDARY_ONLY
                    } else {
                        0
                    },
                42 + i,
            )
        })
        .collect();
    compare(&pieces, 0.0)?;
    assert_eq!(std::mem::size_of::<ChordSource>(), 144);
    assert_eq!(std::mem::offset_of!(ChordSource, physical), 16);
    assert_eq!(std::mem::offset_of!(ChordSource, identity), 120);
    Ok(())
}
