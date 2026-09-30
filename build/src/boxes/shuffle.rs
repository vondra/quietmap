//! The window's segments by z9 square, so that boxes are built square by square with memory for
//! one square's boxes: each day file is read once, filtered to the scope, and every segment that
//! counts on its day (primary flights on baseline days, flights only the secondary provider saw
//! on increment days) goes to that day's file of each square its ends' bounding box meets
//! (`<out>/<x>/<y>/<day>.seg`, fixed 76-byte records). A day is done when its marker
//! `<out>/days/<day>` exists; the marker names the day's roles, from which the builder weighs it.

use super::Window;
use super::read::{FLAG_SECONDARY_ONLY, FlightSegment, read_segments};
use crate::dev4::Square;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tiles::geo::Mercator;

/// Bytes of one record: the segment as the day file holds it (its reals are f32 there), with its
/// acceleration.
const RECORD_BYTES: usize = 80;
/// z12 tiles per z9 square side.
const TILES_PER_SQUARE: f64 = 8.0;

fn encode(segment: &FlightSegment, out: &mut Vec<u8>) {
    out.extend_from_slice(&segment.flight_id.to_le_bytes());
    out.extend_from_slice(&segment.callsign);
    out.extend_from_slice(&segment.designator);
    out.extend_from_slice(&[segment.source_id, segment.period, segment.flags, 0]);
    let reals = [
        segment.start[0],
        segment.start[1],
        segment.start[2],
        segment.end[0],
        segment.end[1],
        segment.end[2],
        segment.pressure_altitude_m[0],
        segment.pressure_altitude_m[1],
        segment.speed_kt,
        segment.above_ground_m,
        segment.departure_field_m,
        segment.ground_m[0],
        segment.ground_m[1],
        segment.acceleration_ms2,
    ];
    for real in reals {
        out.extend_from_slice(&(real as f32).to_le_bytes());
    }
}

fn decode(bytes: &[u8]) -> FlightSegment {
    let real = |index: usize| {
        let at = 24 + 4 * index;
        f64::from(f32::from_le_bytes(
            bytes[at..at + 4].try_into().expect("four bytes"),
        ))
    };
    FlightSegment {
        flight_id: u64::from_le_bytes(bytes[0..8].try_into().expect("eight bytes")),
        callsign: bytes[8..16].try_into().expect("eight bytes"),
        designator: bytes[16..20].try_into().expect("four bytes"),
        source_id: bytes[20],
        period: bytes[21],
        flags: bytes[22],
        start: [real(0), real(1), real(2)],
        end: [real(3), real(4), real(5)],
        pressure_altitude_m: [real(6), real(7)],
        speed_kt: real(8),
        above_ground_m: real(9),
        departure_field_m: real(10),
        ground_m: [real(11), real(12)],
        acceleration_ms2: real(13),
    }
}

/// The scope squares a segment's ends' bounding box meets.
fn squares_of(segment: &FlightSegment, scope: &HashSet<Square>) -> Vec<Square> {
    let square = |end: [f64; 3]| {
        let position = Mercator::from_degrees(end[0], end[1]);
        (
            (position.x / TILES_PER_SQUARE).floor() as i64,
            (position.y / TILES_PER_SQUARE).floor() as i64,
        )
    };
    let (a, b) = (square(segment.start), square(segment.end));
    let (x0, x1, y0, y1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
    if (x1 - x0 + 1) * (y1 - y0 + 1) > 16 {
        // A long segment (a gap in coverage): every scope square in its box.
        return scope
            .iter()
            .filter(|square| {
                (x0..=x1).contains(&i64::from(square.x)) && (y0..=y1).contains(&i64::from(square.y))
            })
            .copied()
            .collect();
    }
    let mut squares = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let square = Square {
                x: x as u32,
                y: y as u32,
            };
            if x >= 0 && y >= 0 && scope.contains(&square) {
                squares.push(square);
            }
        }
    }
    squares
}

fn day_marker(out: &Path, day: &str) -> PathBuf {
    out.join("days").join(day)
}

fn square_day_path(out: &Path, square: Square, day: &str) -> PathBuf {
    out.join(square.x.to_string())
        .join(square.y.to_string())
        .join(format!("{day}.seg"))
}

/// The roles of a day: its primary flights count (a baseline day), its flights only the
/// secondary provider saw count (an increment day).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayRoles {
    pub baseline: bool,
    pub increment: bool,
}

impl DayRoles {
    fn counts(self, segment: &FlightSegment) -> bool {
        if segment.flags & FLAG_SECONDARY_ONLY != 0 {
            self.increment
        } else {
            self.baseline
        }
    }

    /// A segment's weight on such a day: one average day of the window's `baseline` and
    /// `increment` days (the P/B + S/I estimator).
    pub fn weight(self, segment: &FlightSegment, (baseline, increment): (usize, usize)) -> f64 {
        if segment.flags & FLAG_SECONDARY_ONLY != 0 {
            if self.increment {
                1.0 / increment as f64
            } else {
                0.0
            }
        } else if self.baseline {
            1.0 / baseline as f64
        } else {
            0.0
        }
    }
}

/// Writes the segments of every day of `window` under `segments_dir` that counts into the day
/// files of the `scope` squares under `out`. Days already done are skipped, and so are days whose
/// file does not exist yet (a later run takes them); returns how many days were left.
pub fn shuffle(
    segments_dir: &Path,
    window: &Window,
    scope: &HashSet<Square>,
    out: &Path,
) -> Result<usize, String> {
    let mut days: Vec<&String> = window
        .baseline_days
        .iter()
        .chain(&window.increment_days)
        .collect();
    days.sort();
    days.dedup();
    let mut squares: Vec<Square> = scope.iter().copied().collect();
    squares.sort();
    let index_of: std::collections::HashMap<Square, usize> = squares
        .iter()
        .enumerate()
        .map(|(index, &square)| (square, index))
        .collect();
    let in_scope = |start: [f64; 2], end: [f64; 2]| {
        let segment = FlightSegment {
            start: [start[0], start[1], 0.0],
            end: [end[0], end[1], 0.0],
            ..FlightSegment::default()
        };
        !squares_of(&segment, scope).is_empty()
    };
    let mut left = 0;
    for day in days {
        if day_marker(out, day).exists() {
            continue;
        }
        let path = segments_dir.join("segments").join(format!("{day}.arrow"));
        if !path.exists() {
            left += 1;
            continue;
        }
        let roles = DayRoles {
            baseline: window.baseline_days.contains(day),
            increment: window.increment_days.contains(day),
        };
        let segments = read_segments(&path, &in_scope)?;
        // (square, segment) for every square a counting segment meets, then each square's
        // records in segment order.
        let mut routed: Vec<(usize, usize)> = segments
            .par_iter()
            .enumerate()
            .filter(|(_, segment)| roles.counts(segment))
            .flat_map_iter(|(index, segment)| {
                let index_of = &index_of;
                squares_of(segment, scope)
                    .into_iter()
                    .map(move |square| (index_of[&square], index))
            })
            .collect();
        routed.par_sort_unstable();
        let bounds: Vec<usize> = (0..=squares.len())
            .map(|square| routed.partition_point(|entry| entry.0 < square))
            .collect();
        let records: Vec<Vec<u8>> = (0..squares.len())
            .into_par_iter()
            .map(|square| {
                let mut bytes = Vec::new();
                for &(_, index) in &routed[bounds[square]..bounds[square + 1]] {
                    encode(&segments[index], &mut bytes);
                }
                bytes
            })
            .collect();
        for (square, bytes) in squares.iter().zip(&records) {
            if bytes.is_empty() {
                continue;
            }
            let path = square_day_path(out, *square, day);
            let directory = path.parent().expect("a square directory");
            std::fs::create_dir_all(directory)
                .map_err(|error| format!("{}: {error}", directory.display()))?;
            let temporary = path.with_extension("seg.partial");
            std::fs::write(&temporary, bytes)
                .map_err(|error| format!("{}: {error}", temporary.display()))?;
            std::fs::rename(&temporary, &path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
        let marker = day_marker(out, day);
        std::fs::create_dir_all(marker.parent().expect("the days directory"))
            .map_err(|error| error.to_string())?;
        let text = format!(
            "{}{}\n",
            if roles.baseline { "baseline " } else { "" },
            if roles.increment { "increment" } else { "" }
        );
        std::fs::write(&marker, text).map_err(|error| format!("{}: {error}", marker.display()))?;
        eprintln!("aircraft shuffle: {day}: {} segments", segments.len());
    }
    Ok(left)
}

/// The days done under `out` with their roles.
pub fn shuffled_days(out: &Path) -> Result<Vec<(String, DayRoles)>, String> {
    let directory = out.join("days");
    let mut days = Vec::new();
    for entry in std::fs::read_dir(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let day = entry.file_name().to_string_lossy().to_string();
        let text = std::fs::read_to_string(entry.path()).map_err(|error| error.to_string())?;
        let roles = DayRoles {
            baseline: text.contains("baseline"),
            increment: text.contains("increment"),
        };
        days.push((day, roles));
    }
    days.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(days)
}

/// The segments of one square on one day under `out` (none when it had none).
pub fn square_day(out: &Path, square: Square, day: &str) -> Result<Vec<FlightSegment>, String> {
    let path = square_day_path(out, square, day);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if bytes.len() % RECORD_BYTES != 0 {
        return Err(format!("{}: truncated", path.display()));
    }
    Ok(bytes.chunks_exact(RECORD_BYTES).map(decode).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_as_written() {
        let segment = FlightSegment {
            flight_id: 0x4b_1234 << 40 | 1_756_700_000,
            callsign: *b"CSA100  ",
            designator: *b"A320",
            source_id: 1,
            period: 2,
            flags: 5,
            start: [50.1_f32 as f64, 14.25_f32 as f64, 812.5],
            end: [50.2_f32 as f64, 14.5_f32 as f64, 900.25],
            pressure_altitude_m: [800.0, 890.0],
            speed_kt: 160.0,
            above_ground_m: 500.0,
            departure_field_m: f64::NAN,
            ground_m: [312.0, 330.5],
            acceleration_ms2: 0.625,
        };
        let mut bytes = Vec::new();
        encode(&segment, &mut bytes);
        assert_eq!(bytes.len(), RECORD_BYTES);
        let read = decode(&bytes);
        assert!(read.departure_field_m.is_nan());
        assert_eq!(
            FlightSegment {
                departure_field_m: 0.0,
                ..read
            },
            FlightSegment {
                departure_field_m: 0.0,
                ..segment
            }
        );
    }

    #[test]
    fn a_segment_goes_to_every_square_its_box_meets() {
        let scope: HashSet<Square> = [Square { x: 276, y: 173 }, Square { x: 277, y: 173 }].into();
        // Prague's square 276/173 spans 14.0625-14.765625 E; this one crosses into 277/173.
        let segment = |lon0: f64, lon1: f64| FlightSegment {
            start: [50.0, lon0, 500.0],
            end: [50.0, lon1, 500.0],
            ..FlightSegment::default()
        };
        assert_eq!(
            squares_of(&segment(14.3, 14.4), &scope),
            vec![Square { x: 276, y: 173 }]
        );
        let mut both = squares_of(&segment(14.7, 14.8), &scope);
        both.sort();
        assert_eq!(
            both,
            vec![Square { x: 276, y: 173 }, Square { x: 277, y: 173 }]
        );
        assert!(squares_of(&segment(10.0, 10.1), &scope).is_empty());
    }
}
