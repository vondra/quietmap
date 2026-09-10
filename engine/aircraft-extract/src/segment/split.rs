//! Split airborne chords whose stored geometry exceeds the length cap into equal pieces.

use crate::flight::{segment_flags, FlightSegment, Phase};
use crate::geo::interp_along_path;
use crate::support::{
    airborne_stored_endpoints, airborne_stored_length_m, airborne_stored_length_within_cap,
};
use noise_compute::emission::aircraft::AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M;

/// Append `segment` to `pieces`: unchanged unless it is an airborne chord
/// whose *stored* geometry (z30-quantized, Mercator-clamped) is longer than
/// `AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M`; that chord becomes the fewest equal
/// pieces whose stored geometry each fits the cap. Position, altitude and the
/// two endpoint terrain samples interpolate linearly along the stored chord
/// (the popup never had interior samples, so the pieces carry today's
/// information); period, date, speed and identity are those of the chord.
/// Every piece carries `SPLIT_PIECE`, the first `CHORD_START`, the last
/// `CHORD_END`, and consecutive pieces share their endpoint value exactly, so
/// the popup chains them back into one event and one polyline.
pub fn split_airborne_segment(segment: FlightSegment, pieces: &mut Vec<FlightSegment>) {
    let stored = match airborne_stored_endpoints(&segment) {
        Some(stored) if segment.phase == Phase::Airborne => stored,
        // Invalid coordinates are refused by the writer, not here.
        _ => return pieces.push(segment),
    };
    let Some(length) = airborne_stored_length_m(&segment) else {
        return pieces.push(segment);
    };
    if length <= AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M {
        return pieces.push(segment);
    }
    let (start, end) = stored;
    let first = pieces.len();
    let mut count = (length / AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M).ceil() as u32;
    loop {
        let point = |k: u32| {
            let frac = k as f32 / count as f32;
            let position = match k {
                0 => (start[0], start[1]),
                k if k == count => (end[0], end[1]),
                _ => interp_along_path(start[0], start[1], end[0], end[1], frac),
            };
            (
                position,
                segment.start_alt_m + (segment.end_alt_m - segment.start_alt_m) * frac,
                segment.start_elev_m + (segment.end_elev_m - segment.start_elev_m) * frac,
            )
        };
        for k in 0..count {
            let ((start_lat, start_lon), start_alt_m, start_elev_m) = point(k);
            let ((end_lat, end_lon), end_alt_m, end_elev_m) = point(k + 1);
            let mut flags = segment.flags | segment_flags::SPLIT_PIECE;
            if k == 0 {
                flags |= segment_flags::CHORD_START;
            }
            if k + 1 == count {
                flags |= segment_flags::CHORD_END;
            }
            pieces.push(FlightSegment {
                start_lat,
                start_lon,
                start_alt_m,
                end_lat,
                end_lon,
                end_alt_m,
                start_elev_m,
                end_elev_m,
                flags,
                length_m: length / count as f32,
                ..segment.clone()
            });
        }
        // Re-encoding an interpolated point moves it by up to one grid
        // quantum; a piece that lands over the cap needs one more piece.
        if pieces[first..]
            .iter()
            .all(airborne_stored_length_within_cap)
        {
            return;
        }
        pieces.truncate(first);
        count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::flat_dist;
    use crate::support::airborne_decoded_endpoint;

    fn chord(phase: Phase, start: (f32, f32), end: (f32, f32)) -> FlightSegment {
        FlightSegment {
            flight_id: 42,
            callsign: "SPLIT42".into(),
            aircraft_type: *b"A320",
            profile_idx: 3,
            source_id: 2,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 2,
            date_id: 200,
            phase,
            flags: 1,
            start_lat: start.0,
            start_lon: start.1,
            start_alt_m: 1000.0,
            end_lat: end.0,
            end_lon: end.1,
            end_alt_m: 2000.0,
            speed_kt: 250.0,
            length_m: flat_dist(start.0, start.1, end.0, end.1),
            agl_avg_m: 900.0,
            start_elev_m: 300.0,
            end_elev_m: 400.0,
        }
    }

    fn split(segment: FlightSegment) -> Vec<FlightSegment> {
        let mut pieces = Vec::new();
        split_airborne_segment(segment, &mut pieces);
        pieces
    }

    /// An 18 km chord becomes ceil(18/4) = 5 pieces of equal length along
    /// the path, exact at both stored ends, linear in altitude and terrain,
    /// chained through shared endpoints, with every other field copied.
    #[test]
    fn long_chord_splits_into_equal_pieces_within_the_cap() {
        let start = (50.0_f32, 14.0_f32);
        let end = (
            50.0_f32,
            14.0 + 18_000.0 / (111_320.0 * 50.0_f32.to_radians().cos()),
        );
        let chord = chord(Phase::Airborne, start, end);
        assert!((chord.length_m - 18_000.0).abs() < 1.0);
        let pieces = split(chord.clone());
        assert_eq!(pieces.len(), 5);
        let decoded = |p: (f32, f32)| airborne_decoded_endpoint(p.0, p.1).unwrap();
        assert_eq!([pieces[0].start_lat, pieces[0].start_lon], decoded(start));
        assert_eq!([pieces[4].end_lat, pieces[4].end_lon], decoded(end));
        let mut drawn = 0.0;
        for (k, piece) in pieces.iter().enumerate() {
            assert!(airborne_stored_length_within_cap(piece));
            assert_eq!(piece.length_m, chord.length_m / 5.0);
            let geometry = flat_dist(
                piece.start_lat,
                piece.start_lon,
                piece.end_lat,
                piece.end_lon,
            );
            assert!((geometry - 3_600.0).abs() < 0.5, "piece {k}: {geometry} m");
            drawn += geometry;
            if k > 0 {
                let previous = &pieces[k - 1];
                assert_eq!(
                    (previous.end_lat, previous.end_lon),
                    (piece.start_lat, piece.start_lon)
                );
                assert_eq!(previous.end_alt_m, piece.start_alt_m);
                assert_eq!(previous.end_elev_m, piece.start_elev_m);
            }
            assert!((piece.end_alt_m - piece.start_alt_m - 200.0).abs() < 0.01);
            assert!((piece.end_elev_m - piece.start_elev_m - 20.0).abs() < 0.01);
            let expected_flags = segment_flags::IS_DEPARTURE
                | segment_flags::SPLIT_PIECE
                | if k == 0 {
                    segment_flags::CHORD_START
                } else {
                    0
                }
                | if k == 4 { segment_flags::CHORD_END } else { 0 };
            assert_eq!(
                (
                    piece.flight_id,
                    piece.callsign.as_str(),
                    piece.period,
                    piece.date_id,
                    piece.flags
                ),
                (42, "SPLIT42", 2, 200, expected_flags)
            );
            assert_eq!((piece.speed_kt, piece.agl_avg_m), (250.0, 900.0));
        }
        assert!((drawn - chord.length_m).abs() < 1.0);
    }

    #[test]
    fn chords_within_the_cap_and_other_phases_pass_through_unchanged() {
        let short = chord(Phase::Airborne, (50.0, 14.0), (50.0, 14.05));
        assert!(short.length_m < AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M);
        let ground = chord(Phase::Ground, (50.0, 14.0), (50.0, 14.5));
        for segment in [short, ground] {
            let pieces = split(segment.clone());
            assert_eq!(pieces.len(), 1);
            assert_eq!(pieces[0].flags, segment.flags);
            assert_eq!(pieces[0].length_m, segment.length_m);
            assert_eq!(
                (
                    pieces[0].start_lat,
                    pieces[0].start_lon,
                    pieces[0].end_lat,
                    pieces[0].end_lon
                ),
                (
                    segment.start_lat,
                    segment.start_lon,
                    segment.end_lat,
                    segment.end_lon
                )
            );
        }
    }

    /// A seam-crossing chord splits along the short arc, every piece wrapped.
    #[test]
    fn seam_crossing_chord_splits_along_the_short_arc() {
        let pieces = split(chord(Phase::Airborne, (0.0, 179.95), (0.0, -179.95)));
        assert_eq!(pieces.len(), 3);
        assert!(pieces
            .iter()
            .all(|p| p.start_lon.abs() <= 180.0 && p.end_lon.abs() <= 180.0));
        assert!(pieces[1].start_lon > 179.9 && pieces[1].end_lon < -179.9);
    }

    /// The cap bounds what is stored: a 3.9 km chord at 89.9° N is clamped
    /// to the Mercator limit on disk, where its 20° of longitude span
    /// 192 km (111 320 m × cos 85.051° × 20), so it is split until every
    /// stored piece fits the cap.
    #[test]
    fn polar_chord_is_split_by_its_stored_clamped_geometry() {
        let chord = chord(Phase::Airborne, (89.9, 0.0), (89.9, 20.0));
        assert!((chord.length_m - 3_886.0).abs() < 5.0, "{}", chord.length_m);
        let stored = airborne_stored_length_m(&chord).unwrap();
        assert!((stored - 192_062.0).abs() < 50.0, "{stored}");
        let pieces = split(chord);
        assert_eq!(
            pieces.len(),
            (stored / AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M).ceil() as usize
        );
        assert!(pieces.iter().all(airborne_stored_length_within_cap));
        assert!(pieces.iter().all(|p| (p.start_lat - 85.0511).abs() < 0.001));
        assert_eq!(pieces[0].start_lon, 0.0);
        assert!((pieces.last().unwrap().end_lon - 20.0).abs() < 1e-4);
    }
}
