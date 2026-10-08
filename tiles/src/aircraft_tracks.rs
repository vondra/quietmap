//! `aircraft-tracks` files: every flight of the year as the map draws it, so a listed flight's line
//! runs whole where the boxes keep only each cell's loudest pieces. A flight is its segments' ends
//! with those within [`TOLERANCE_M`] of the line through the ends kept left out (Douglas-Peucker),
//! one part unless it crosses the antimeridian. The flights
//! are split by the low byte of their address into 256 files (`<year root>/aircraft-tracks/<xx>.
//! aircraft-tracks`), each sorted by address and start, so a click finds a flight by a binary
//! search of positioned reads instead of reading a file.
//!
//! ```text
//! magic "qmtrk1\n\0", u32 parts
//! per part, sorted by address, start and first point, 16 B: u32 address, u32 start (the flight's
//!   key as its boxes carry it), u32 first point, u32 points
//! per point, 10 B: i32 latitude and longitude (1e-6 deg), i16 altitude above sea level (m)
//! ```

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"qmtrk1\n\0";
const HEADER_BYTES: u64 = 12;
const PART_BYTES: u64 = 16;
const POINT_BYTES: usize = 10;
/// How far a left-out end may lie from the line drawn (m): a pixel at zoom 13, and a helicopter's
/// circle of a few hundred metres keeps its shape.
pub const TOLERANCE_M: f64 = 20.0;

/// A point of a track: latitude and longitude (deg), altitude above sea level (m).
pub type TrackPoint = [f64; 3];

/// The file holding the flights of `address`.
pub fn tracks_path(year_root: &Path, address: u32) -> PathBuf {
    year_root
        .join("aircraft-tracks")
        .join(format!("{:02x}.aircraft-tracks", address & 0xff))
}

/// One part of a flight's track, keyed as its boxes key the flight.
pub struct Part {
    pub address: u32,
    pub start: u32,
    pub points: Vec<TrackPoint>,
}

/// The bytes of one file's parts (any order; sorted here).
pub fn encode(mut parts: Vec<Part>) -> Vec<u8> {
    parts.sort_by_key(|part| (part.address, part.start));
    let points: usize = parts.iter().map(|part| part.points.len()).sum();
    assert!(u32::try_from(points).is_ok(), "points of one file");
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES as usize + PART_BYTES as usize * parts.len() + POINT_BYTES * points,
    );
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    let mut first = 0u32;
    for part in &parts {
        for value in [part.address, part.start, first, part.points.len() as u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        first += part.points.len() as u32;
    }
    for part in &parts {
        for &[lat, lon, altitude] in &part.points {
            bytes.extend_from_slice(&((lat * 1e6).round() as i32).to_le_bytes());
            bytes.extend_from_slice(&((lon * 1e6).round() as i32).to_le_bytes());
            bytes.extend_from_slice(
                &(altitude.round().clamp(-32_768.0, 32_767.0) as i16).to_le_bytes(),
            );
        }
    }
    bytes
}

/// The points of consecutive point records.
fn points_of(bytes: &[u8]) -> Vec<TrackPoint> {
    bytes
        .chunks_exact(POINT_BYTES)
        .map(|point| {
            [
                f64::from(i32::from_le_bytes(point[0..4].try_into().unwrap())) / 1e6,
                f64::from(i32::from_le_bytes(point[4..8].try_into().unwrap())) / 1e6,
                f64::from(i16::from_le_bytes([point[8], point[9]])),
            ]
        })
        .collect()
}

/// Every part of a file's bytes, in the file's order.
pub fn decode(bytes: &[u8]) -> Result<Vec<Part>, String> {
    if bytes.len() < HEADER_BYTES as usize || &bytes[..8] != MAGIC {
        return Err("aircraft-tracks: bad magic".into());
    }
    let parts = u32_at(bytes, 8) as usize;
    let points_at = HEADER_BYTES as usize + PART_BYTES as usize * parts;
    let mut decoded = Vec::with_capacity(parts);
    let mut points = 0;
    for index in 0..parts {
        let at = HEADER_BYTES as usize + PART_BYTES as usize * index;
        let [address, start, first, count] =
            std::array::from_fn(|field| u32_at(bytes, at + 4 * field));
        let (first, count) = (first as usize, count as usize);
        if first != points || points_at + POINT_BYTES * (first + count) > bytes.len() {
            return Err("aircraft-tracks: a part outside the points".into());
        }
        points += count;
        let at = points_at + POINT_BYTES * first;
        decoded.push(Part {
            address,
            start,
            points: points_of(&bytes[at..at + POINT_BYTES * count]),
        });
    }
    if points_at + POINT_BYTES * points != bytes.len() {
        return Err("aircraft-tracks: length does not match".into());
    }
    Ok(decoded)
}

fn read_at(file: &File, path: &Path, at: u64, length: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; length];
    file.read_exact_at(&mut bytes, at)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(bytes)
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// The parts of the flight `(address, start)` in the file at `path`, in their order along it; none
/// when the file holds no such flight.
pub fn flight_parts(path: &Path, address: u32, start: u32) -> Result<Vec<Vec<TrackPoint>>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let header = read_at(&file, path, 0, HEADER_BYTES as usize)?;
    if &header[..8] != MAGIC {
        return Err(format!("{}: bad magic", path.display()));
    }
    let parts = u64::from(u32_at(&header, 8));
    let points_at = HEADER_BYTES + PART_BYTES * parts;
    let entry = |index: u64| -> Result<[u32; 4], String> {
        let bytes = read_at(
            &file,
            path,
            HEADER_BYTES + PART_BYTES * index,
            PART_BYTES as usize,
        )?;
        Ok(std::array::from_fn(|field| u32_at(&bytes, 4 * field)))
    };
    // The first part not before the flight's key.
    let (mut low, mut high) = (0, parts);
    while low < high {
        let middle = (low + high) / 2;
        let [entry_address, entry_start, ..] = entry(middle)?;
        if (entry_address, entry_start) < (address, start) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let mut found = Vec::new();
    for index in low..parts {
        let [entry_address, entry_start, first, count] = entry(index)?;
        if (entry_address, entry_start) != (address, start) {
            break;
        }
        let bytes = read_at(
            &file,
            path,
            points_at + POINT_BYTES as u64 * u64::from(first),
            POINT_BYTES * count as usize,
        )?;
        found.push(points_of(&bytes));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flight's parts come back in their order and to the stored precision; another flight's
    /// and a flight not in the file are apart.
    #[test]
    fn a_flight_reads_back_its_parts() {
        let part = |address: u32, start: u32, lat: f64| Part {
            address,
            start,
            points: vec![[lat, 14.5, 812.4], [lat + 0.01, 14.51, 905.6]],
        };
        let bytes = encode(vec![
            part(0x4b1805, 7, 50.0),
            part(0x4ca1aa, 1_780_000_000, 50.1),
            part(0x4ca1aa, 1_780_000_000, 50.2),
            part(0x4ca1aa, 1_780_009_999, 49.0),
        ]);
        let path = std::env::temp_dir().join(format!("qm-tracks-{}.test", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let parts = flight_parts(&path, 0x4ca1aa, 1_780_000_000).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], vec![[50.1, 14.5, 812.0], [50.11, 14.51, 906.0]]);
        assert_eq!(parts[1][0], [50.2, 14.5, 812.0]);
        assert_eq!(flight_parts(&path, 0x4b1805, 7).unwrap().len(), 1);
        assert!(flight_parts(&path, 0x4ca1aa, 8).unwrap().is_empty());
        assert!(flight_parts(&path, 0xffffff, 0).unwrap().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            tracks_path(Path::new("/y"), 0x4ca1aa),
            Path::new("/y/aircraft-tracks/aa.aircraft-tracks")
        );
    }
}
