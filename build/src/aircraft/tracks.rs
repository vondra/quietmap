//! `qm-build aircraft-tracks`: every flight of the year's Stage 1 days as the map draws it
//! (`tiles::aircraft_tracks`). A day's aircraft segments, a flight's following one another in its
//! file, chain into one line, straight across the receivers' gaps as the aircraft flew there, and
//! broken only where it crosses the antimeridian (a segment from one side to the other is left
//! out); the line keeps the ends Douglas-Peucker needs within the tolerance of the segments drawn. The days run in parallel, each writing its flights split by address into
//! a scratch file per split; then each split's days are merged into its file.

use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, UInt8Type, UInt64Type};
use arrow_array::{Array, RecordBatch};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use tiles::aircraft_tracks::{Part, TOLERANCE_M, TrackPoint, decode, encode};

/// Stage 1's vehicle kind of an aircraft (the rest are airport vehicles).
const KIND_AIRCRAFT: u8 = 0;
const COLUMNS: [&str; 6] = [
    "flight_id",
    "veh_kind",
    "start_lat",
    "start_lon",
    "end_lat",
    "end_lon",
];
const SPLITS: usize = 256;

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a dyn Array, String> {
    batch
        .column_by_name(name)
        .map(|column| column.as_ref())
        .ok_or_else(|| format!("segments: no column {name}"))
}

/// Metres between two points (an equirectangular projection: the parts' steps are short).
fn metres(a: &TrackPoint, b: &TrackPoint) -> [f64; 2] {
    const METRES_PER_DEGREE: f64 = 111_195.0;
    let east = (b[1] - a[1]) * METRES_PER_DEGREE * ((a[0] + b[0]) / 2.0).to_radians().cos();
    [east, (b[0] - a[0]) * METRES_PER_DEGREE]
}

/// The distance (m) from `point` to the segment from `a` to `b`.
fn offset_m(point: &TrackPoint, a: &TrackPoint, b: &TrackPoint) -> f64 {
    let [dx, dy] = metres(a, b);
    let [px, py] = metres(a, point);
    let length_squared = dx * dx + dy * dy;
    let along = if length_squared > 0.0 {
        ((px * dx + py * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - along * dx).hypot(py - along * dy)
}

/// Whether a step from `a` to `b` crosses the antimeridian (its longitudes more than half the
/// world apart).
fn crosses(a: &TrackPoint, b: &TrackPoint) -> bool {
    (a[1] - b[1]).abs() > 180.0
}

/// The points Douglas-Peucker keeps within [`TOLERANCE_M`], the first and last always.
pub fn simplify(points: &[TrackPoint]) -> Vec<TrackPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut spans = vec![(0, points.len() - 1)];
    while let Some((first, last)) = spans.pop() {
        let farthest = (first + 1..last)
            .map(|index| {
                (
                    offset_m(&points[index], &points[first], &points[last]),
                    index,
                )
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((offset, index)) = farthest
            && offset > TOLERANCE_M
        {
            keep[index] = true;
            spans.push((first, index));
            spans.push((index, last));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, kept)| kept.then_some(*point))
        .collect()
}

/// A flight's key as its boxes carry it: address and start.
fn key(flight_id: u64) -> (u32, u32) {
    ((flight_id >> 40) as u32 & 0x00ff_ffff, flight_id as u32)
}

/// The parts of one day file's aircraft flights, by split.
fn day_parts(path: &Path) -> Result<Vec<Vec<Part>>, String> {
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let open = || {
        std::fs::File::open(path)
            .map(std::io::BufReader::new)
            .map_err(|e| failed(&e))
    };
    let schema = arrow_ipc::reader::FileReader::try_new(open()?, None)
        .map_err(|e| failed(&e))?
        .schema();
    let projection = COLUMNS
        .iter()
        .map(|name| schema.index_of(name).map_err(|e| failed(&e)))
        .collect::<Result<Vec<_>, _>>()?;
    let reader = arrow_ipc::reader::FileReader::try_new(open()?, Some(projection))
        .map_err(|e| failed(&e))?;
    let mut splits: Vec<Vec<Part>> = (0..SPLITS).map(|_| Vec::new()).collect();
    let mut current: Option<(u64, Vec<TrackPoint>)> = None;
    let mut close = |part: Option<(u64, Vec<TrackPoint>)>| {
        if let Some((flight_id, points)) = part.filter(|(_, points)| points.len() >= 2) {
            let (address, start) = key(flight_id);
            splits[(address & 0xff) as usize].push(Part {
                address,
                start,
                points: simplify(&points),
            });
        }
    };
    for batch in reader {
        let batch = batch.map_err(|e| failed(&e))?;
        let flight_id = column(&batch, "flight_id")?.as_primitive::<UInt64Type>();
        let kind = column(&batch, "veh_kind")?.as_primitive::<UInt8Type>();
        let floats = ["start_lat", "start_lon", "end_lat", "end_lon"]
            .map(|name| column(&batch, name).map(|c| c.as_primitive::<Float32Type>()));
        let [start_lat, start_lon, end_lat, end_lon] = floats;
        let (start_lat, start_lon, end_lat, end_lon) = (start_lat?, start_lon?, end_lat?, end_lon?);
        for row in 0..batch.num_rows() {
            if kind.value(row) != KIND_AIRCRAFT {
                continue;
            }
            let id = flight_id.value(row);
            let start = [start_lat.value(row), start_lon.value(row)].map(f64::from);
            let end = [end_lat.value(row), end_lon.value(row)].map(f64::from);
            if current
                .as_ref()
                .is_some_and(|(current_id, _)| *current_id != id)
            {
                close(current.take());
            }
            let points = &mut current.get_or_insert_with(|| (id, Vec::new())).1;
            // A segment from one side of the antimeridian to the other breaks the line; a gap
            // between segments is drawn straight unless it crosses it too.
            if crosses(&start, &end) || points.last().is_some_and(|last| crosses(last, &start)) {
                close(current.take());
                if crosses(&start, &end) {
                    current = Some((id, vec![end]));
                    continue;
                }
                current = Some((id, Vec::new()));
            }
            let points = &mut current.as_mut().expect("a flight").1;
            if points.last() != Some(&start) {
                points.push(start);
            }
            points.push(end);
        }
    }
    close(current.take());
    Ok(splits)
}

fn split_name(split: usize) -> String {
    format!("{split:02x}.aircraft-tracks")
}

/// Writes `<out>/aircraft-tracks/<xx>.aircraft-tracks` from every day file in `segments`; the
/// scratch files of a day already done are kept, so a rerun resumes.
pub fn build(segments: &Path, out: &Path) -> Result<(), String> {
    let mut days: Vec<PathBuf> = std::fs::read_dir(segments)
        .map_err(|error| format!("{}: {error}", segments.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "arrow")
        })
        .collect();
    days.sort();
    let scratch = out.join("aircraft-tracks.days");
    let done = out.join("aircraft-tracks");
    std::fs::create_dir_all(&done).map_err(|error| error.to_string())?;
    days.par_iter().try_for_each(|day| -> Result<(), String> {
        let name = day.file_stem().unwrap().to_string_lossy().to_string();
        let directory = scratch.join(&name);
        if directory.join("complete").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        for (split, parts) in day_parts(day)?.into_iter().enumerate() {
            std::fs::write(directory.join(split_name(split)), encode(parts))
                .map_err(|error| error.to_string())?;
        }
        std::fs::write(directory.join("complete"), b"").map_err(|error| error.to_string())?;
        eprintln!("aircraft tracks: {name}");
        Ok(())
    })?;
    (0..SPLITS)
        .into_par_iter()
        .try_for_each(|split| -> Result<(), String> {
            let mut parts = Vec::new();
            for day in &days {
                let name = day.file_stem().unwrap().to_string_lossy().to_string();
                let path = scratch.join(name).join(split_name(split));
                let bytes =
                    std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                parts.extend(
                    decode(&bytes).map_err(|error| format!("{}: {error}", path.display()))?,
                );
            }
            let path = done.join(split_name(split));
            let partial = path.with_extension("partial");
            std::fs::write(&partial, encode(parts)).map_err(|error| error.to_string())?;
            std::fs::rename(&partial, &path).map_err(|error| error.to_string())
        })?;
    std::fs::remove_dir_all(&scratch).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight run keeps its ends; a circle of 300 m radius keeps enough points to stay within
    /// the tolerance of its arcs; a point off the line by more than the tolerance stays, and so
    /// does a turn back along the same line (its end lies beyond the segment, not off its line).
    #[test]
    fn douglas_peucker_keeps_the_shape_within_the_tolerance() {
        let straight: Vec<TrackPoint> = (0..50).map(|k| [50.0 + k as f64 * 1e-4, 14.0]).collect();
        assert_eq!(simplify(&straight), vec![straight[0], straight[49]]);
        let circle: Vec<TrackPoint> = (0..=360)
            .map(|degree| {
                let angle = (degree as f64).to_radians();
                [
                    50.0 + 300.0 * angle.sin() / 111_195.0,
                    14.0 + 300.0 * angle.cos() / (111_195.0 * 50f64.to_radians().cos()),
                ]
            })
            .collect();
        let kept = simplify(&circle);
        assert!(kept.len() > 8 && kept.len() < 60, "{}", kept.len());
        for point in &circle {
            let nearest = kept
                .windows(2)
                .map(|pair| offset_m(point, &pair[0], &pair[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(nearest <= TOLERANCE_M + 1e-6);
        }
        let bent = [
            [50.0, 14.0],
            [50.0005, 14.0 + 20.0 / 71_474.0],
            [50.001, 14.0],
        ];
        assert_eq!(simplify(&bent).len(), 3);
        let back = [[50.0, 14.0], [50.0, 14.02], [50.0, 14.01]];
        assert_eq!(simplify(&back).len(), 3);
    }

    /// Steps more than half the world apart cross the antimeridian.
    #[test]
    fn a_step_across_the_antimeridian_is_told() {
        assert!(crosses(&[10.0, 179.98], &[10.0, -179.99]));
        assert!(!crosses(&[10.0, 179.98], &[10.0, 179.99]));
    }
}
