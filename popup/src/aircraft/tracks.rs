//! The listed flights' lines on the map: each flight's track (`tiles::aircraft_tracks`, the whole
//! flight as the map draws it) clipped to the square of [`WINDOW_M`] around the click, segment by
//! segment, and simplified as the contributors' lines are until it holds at most [`POINTS_MAX`]
//! points, so the final update stays within its size.

use super::flights::LoudFlight;
use std::path::Path;
use tiles::aircraft_tracks::{TrackPoint, TracksFile, tracks_path};

/// How far from the click a track is drawn (m, each way): beyond the aircraft reach, so a listed
/// flight's line runs off the area it was heard from.
const WINDOW_M: f64 = 20_000.0;
/// The most points a flight's line keeps (about 2 KB of the update).
const POINTS_MAX: usize = 100;
const METRES_PER_DEGREE: f64 = 111_195.0;

/// A point in metres east and north of `centre` (lat, lon in deg), across the antimeridian too.
fn local(centre: [f64; 2], point: &TrackPoint) -> [f64; 2] {
    let east = (point[1] - centre[1] + 540.0).rem_euclid(360.0) - 180.0;
    [
        east * METRES_PER_DEGREE * centre[0].to_radians().cos(),
        (point[0] - centre[0]) * METRES_PER_DEGREE,
    ]
}

/// The point (lat, lon) at `metres` east and north of `centre`.
fn degrees(centre: [f64; 2], [east, north]: [f64; 2]) -> TrackPoint {
    let lon = centre[1] + east / (METRES_PER_DEGREE * centre[0].to_radians().cos());
    [
        centre[0] + north / METRES_PER_DEGREE,
        (lon + 540.0).rem_euclid(360.0) - 180.0,
    ]
}

/// The part of the segment from `a` to `b` inside the window (Liang-Barsky), if any.
fn clipped(a: [f64; 2], b: [f64; 2]) -> Option<[[f64; 2]; 2]> {
    let (mut enter, mut leave) = (0.0f64, 1.0f64);
    let delta = [b[0] - a[0], b[1] - a[1]];
    for axis in 0..2 {
        for (p, q) in [
            (-delta[axis], a[axis] + WINDOW_M),
            (delta[axis], WINDOW_M - a[axis]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else if p < 0.0 {
                enter = enter.max(q / p);
            } else {
                leave = leave.min(q / p);
            }
        }
    }
    (enter <= leave).then(|| {
        // An end inside stays itself, so consecutive segments still meet.
        let at = |t: f64| match t {
            0.0 => a,
            1.0 => b,
            _ => [a[0] + t * delta[0], a[1] + t * delta[1]],
        };
        [at(enter), at(leave)]
    })
}

/// The runs of `parts` inside the window (metres from `centre`), each segment clipped at its
/// edge, simplified together to at most [`POINTS_MAX`] points.
fn near(parts: Vec<Vec<TrackPoint>>, centre: [f64; 2]) -> Vec<Vec<TrackPoint>> {
    let mut runs: Vec<Vec<[f64; 2]>> = Vec::new();
    for part in parts {
        let mut run: Vec<[f64; 2]> = Vec::new();
        for pair in part.windows(2) {
            match clipped(local(centre, &pair[0]), local(centre, &pair[1])) {
                Some([from, to]) => {
                    if run.last() != Some(&from) {
                        if run.len() >= 2 {
                            runs.push(std::mem::take(&mut run));
                        }
                        run = vec![from];
                    }
                    run.push(to);
                }
                None if run.len() >= 2 => runs.push(std::mem::take(&mut run)),
                None => run.clear(),
            }
        }
        if run.len() >= 2 {
            runs.push(run);
        }
    }
    crate::lines::simplified(&[&runs], POINTS_MAX)
        .into_iter()
        .flatten()
        .map(|run| {
            run.into_iter()
                .map(|point| degrees(centre, point))
                .collect()
        })
        .collect()
}

/// Gives each listed flight its line near the click at `centre` (lat, lon in deg), each tracks
/// file opened once; the files opened and bytes read.
pub fn attach(
    year_root: &Path,
    flights: &mut [LoudFlight],
    centre: [f64; 2],
) -> Result<(usize, u64), String> {
    let mut opened: Vec<(std::path::PathBuf, TracksFile)> = Vec::new();
    let mut bytes = 0;
    for flight in flights {
        let path = tracks_path(year_root, flight.icao);
        let index = match opened.iter().position(|(open, _)| *open == path) {
            Some(index) => index,
            None => {
                let file = TracksFile::open(&path)?;
                bytes += file.opened_bytes();
                opened.push((path, file));
                opened.len() - 1
            }
        };
        let (parts, read) = opened[index].1.flight(flight.icao, flight.start_unix)?;
        bytes += read;
        flight.track = near(parts, centre);
    }
    Ok((opened.len(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line crossing the window keeps what lies inside, cut at the window's edges; a segment
    /// passing over the click with both ends far outside still shows; a line far away shows
    /// nothing; a long line is simplified to the cap with its ends; a line across the
    /// antimeridian beside the click is near it.
    #[test]
    fn a_track_is_cut_to_the_window_and_simplified() {
        let centre = [50.0, 14.0];
        let degree = METRES_PER_DEGREE * 50f64.to_radians().cos();
        let east = |metres: f64| [50.0, 14.0 + metres / degree];
        let runs = near(
            vec![vec![east(-48_000.0), east(-6_000.0), east(48_000.0)]],
            centre,
        );
        assert_eq!(runs.len(), 1);
        let ends = [
            local(centre, &runs[0][0]),
            local(centre, runs[0].last().unwrap()),
        ];
        assert!((ends[0][0] + WINDOW_M).abs() < 1e-6 && (ends[1][0] - WINDOW_M).abs() < 1e-6);
        assert_eq!(
            near(vec![vec![east(-90_000.0), east(90_000.0)]], centre).len(),
            1
        );
        assert!(near(vec![vec![[52.0, 14.0], [52.1, 14.0]]], centre).is_empty());
        let wiggle: Vec<TrackPoint> = (0..1_000)
            .map(|k| {
                let offset = if k % 2 == 0 { 0.0 } else { 300.0 };
                [50.0 + k as f64 * 1e-5, 14.0 + offset / degree]
            })
            .collect();
        let kept: usize = near(vec![wiggle], centre).iter().map(Vec::len).sum();
        assert!((2..=POINTS_MAX).contains(&kept), "{kept}");
        let dateline = [10.0, 179.99];
        let across = near(vec![vec![[10.0, -179.98], [10.01, -179.97]]], dateline);
        assert_eq!(across.len(), 1);
    }
}
