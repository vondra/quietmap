//! The listed flights' lines on the map: each flight's track (`tiles::aircraft_tracks`, the whole
//! flight as the map draws it) cut to what lies within [`WINDOW_M`] of the click, at most
//! [`POINTS_MAX`] points a flight so the final update stays within its size.

use super::flights::LoudFlight;
use std::path::Path;
use tiles::aircraft_tracks::{TrackPoint, flight_parts, tracks_path};

/// How far from the click a track is drawn (m): beyond the aircraft reach, so a listed flight's line
/// runs off the area it was heard from.
const WINDOW_M: f64 = 20_000.0;
/// The most points a flight's line keeps (about 2.5 KB of the update).
const POINTS_MAX: usize = 100;

/// The horizontal distance (m) from `centre` (lat, lon in deg) to `point`.
fn distance_m(centre: [f64; 2], point: &TrackPoint) -> f64 {
    const METRES_PER_DEGREE: f64 = 111_195.0;
    let east = (point[1] - centre[1]) * METRES_PER_DEGREE * centre[0].to_radians().cos();
    east.hypot((point[0] - centre[0]) * METRES_PER_DEGREE)
}

/// The runs of `parts` within the window, each with the point before and after it so the line
/// reaches the window's edge, thinned evenly to [`POINTS_MAX`] points in all.
fn near(parts: Vec<Vec<TrackPoint>>, centre: [f64; 2]) -> Vec<Vec<TrackPoint>> {
    let mut runs: Vec<Vec<TrackPoint>> = Vec::new();
    for part in parts {
        let inside: Vec<bool> = part
            .iter()
            .map(|point| distance_m(centre, point) <= WINDOW_M)
            .collect();
        let mut run: Vec<TrackPoint> = Vec::new();
        for index in 0..part.len() {
            let wanted = inside[index]
                || (index > 0 && inside[index - 1])
                || (index + 1 < part.len() && inside[index + 1]);
            if wanted {
                run.push(part[index]);
            } else if run.len() >= 2 {
                runs.push(std::mem::take(&mut run));
            } else {
                run.clear();
            }
        }
        if run.len() >= 2 {
            runs.push(run);
        }
    }
    let points: usize = runs.iter().map(Vec::len).sum();
    let step = points.div_ceil(POINTS_MAX).max(1);
    runs.into_iter()
        .map(|run| {
            let last = run.len() - 1;
            run.into_iter()
                .enumerate()
                .filter(|(index, _)| index % step == 0 || *index == last)
                .map(|(_, point)| point)
                .collect()
        })
        .collect()
}

/// Gives each listed flight its line near the click at `centre` (lat, lon in deg).
pub fn attach(
    year_root: &Path,
    flights: &mut [LoudFlight],
    centre: [f64; 2],
) -> Result<(), String> {
    for flight in flights {
        let parts = flight_parts(
            &tracks_path(year_root, flight.icao),
            flight.icao,
            flight.start_unix,
        )?;
        flight.track = near(parts, centre);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line crossing the window keeps the points inside and one beyond each edge; a line far
    /// away keeps none; a long line is thinned to the cap with its ends.
    #[test]
    fn a_track_is_cut_to_the_window_and_thinned() {
        let centre = [50.0, 14.0];
        // Points every 6 km eastwards from 48 km west to 48 km east: seven inside.
        let degree = 111_195.0 * 50f64.to_radians().cos();
        let line: Vec<TrackPoint> = (-8..=8)
            .map(|k| [50.0, 14.0 + 6_000.0 * k as f64 / degree, 900.0])
            .collect();
        let runs = near(vec![line.clone()], centre);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 9);
        assert_eq!(runs[0][0], line[4]);
        assert!(near(vec![vec![[52.0, 14.0, 0.0], [52.1, 14.0, 0.0]]], centre).is_empty());
        let dense: Vec<TrackPoint> = (0..1_000)
            .map(|k| [50.0 + k as f64 * 1e-5, 14.0, 300.0])
            .collect();
        let thinned = near(vec![dense.clone()], centre);
        let kept: usize = thinned.iter().map(Vec::len).sum();
        assert!(kept <= POINTS_MAX + 1, "{kept}");
        assert_eq!(thinned[0].last(), dense.last());
    }
}
