//! `aircraft-tracks` files: every flight of the year as the map draws it, so a listed flight's line
//! runs whole where the boxes keep only each cell's loudest pieces. A flight is its segments' ends
//! with those within [`TOLERANCE_M`] of the line through the ends kept left out (Douglas-Peucker),
//! one part unless it crosses the antimeridian. The flights are split by the low byte of their
//! address into 256 files (`<year root>/aircraft-tracks/<xx>.aircraft-tracks`), each sorted by
//! address and start, so a click reads a file's fence (the first key of every page of 256 parts),
//! one page of its directory a flight, and only that flight's points. The map is flat: no heights.
//!
//! ```text
//! magic "qmtrk3\n\0", u32 parts
//! per page of 256 parts, 8 B: u32 address, u32 start of its first part
//! per part, sorted by address, start and first point, 16 B: u32 address, u32 start (the flight's
//!   key as its boxes carry it), u32 first point, u32 points
//! per point, 8 B: i32 latitude and longitude (1e-6 deg)
//! ```

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"qmtrk3\n\0";
const HEADER_BYTES: u64 = 12;
const FENCE_BYTES: u64 = 8;
/// Parts a page of the directory holds: what a click reads of it for one flight (4 KiB).
const PAGE_PARTS: u64 = 256;
const PART_BYTES: u64 = 16;
const POINT_BYTES: u64 = 8;
/// How far a left-out end may lie from the line drawn (m): a pixel at zoom 13, and a helicopter's
/// circle of a few hundred metres keeps its shape.
pub const TOLERANCE_M: f64 = 20.0;

/// A point of a track: latitude and longitude (deg).
pub type TrackPoint = [f64; 2];

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
    let pages = (parts.len() as u64).div_ceil(PAGE_PARTS) as usize;
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES as usize
            + FENCE_BYTES as usize * pages
            + PART_BYTES as usize * parts.len()
            + POINT_BYTES as usize * points,
    );
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    for part in parts.iter().step_by(PAGE_PARTS as usize) {
        bytes.extend_from_slice(&part.address.to_le_bytes());
        bytes.extend_from_slice(&part.start.to_le_bytes());
    }
    let mut first = 0u32;
    for part in &parts {
        for value in [part.address, part.start, first, part.points.len() as u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        first += part.points.len() as u32;
    }
    for part in &parts {
        for &[lat, lon] in &part.points {
            bytes.extend_from_slice(&((lat * 1e6).round() as i32).to_le_bytes());
            bytes.extend_from_slice(&((lon * 1e6).round() as i32).to_le_bytes());
        }
    }
    bytes
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// The points of consecutive point records.
fn points_of(bytes: &[u8]) -> Vec<TrackPoint> {
    bytes
        .chunks_exact(POINT_BYTES as usize)
        .map(|point| {
            [
                f64::from(i32::from_le_bytes(point[0..4].try_into().unwrap())) / 1e6,
                f64::from(i32::from_le_bytes(point[4..8].try_into().unwrap())) / 1e6,
            ]
        })
        .collect()
}

/// Where a file's directory and points start and how many points there are, from its length and
/// parts: none for a file shorter than its header, fence and parts' records, or ending inside a
/// point.
fn layout(length: u64, parts: u64) -> Option<(u64, u64, u64)> {
    let directory_at = parts
        .div_ceil(PAGE_PARTS)
        .checked_mul(FENCE_BYTES)?
        .checked_add(HEADER_BYTES)?;
    let points_at = parts.checked_mul(PART_BYTES)?.checked_add(directory_at)?;
    let point_bytes = length.checked_sub(points_at)?;
    (point_bytes % POINT_BYTES == 0).then_some((directory_at, points_at, point_bytes / POINT_BYTES))
}

/// Every part of a file's bytes, in the file's order; the fence must name each page's first key.
pub fn decode(bytes: &[u8]) -> Result<Vec<Part>, String> {
    if bytes.len() < HEADER_BYTES as usize || &bytes[..8] != MAGIC {
        return Err("aircraft-tracks: bad magic".into());
    }
    let parts = u64::from(u32_at(bytes, 8));
    let (directory_at, points_at, points) =
        layout(bytes.len() as u64, parts).ok_or("aircraft-tracks: length does not match")?;
    let mut decoded = Vec::with_capacity(parts as usize);
    let mut next = 0;
    for index in 0..parts as usize {
        let at = directory_at as usize + PART_BYTES as usize * index;
        let [address, start, first, count] =
            std::array::from_fn(|field| u32_at(bytes, at + 4 * field));
        if (index as u64).is_multiple_of(PAGE_PARTS) {
            let fence =
                HEADER_BYTES as usize + FENCE_BYTES as usize * (index / PAGE_PARTS as usize);
            if [u32_at(bytes, fence), u32_at(bytes, fence + 4)] != [address, start] {
                return Err("aircraft-tracks: the fence misses a page's first key".into());
            }
        }
        let (first, count) = (u64::from(first), u64::from(count));
        if first != next || first + count > points {
            return Err("aircraft-tracks: a part outside the points".into());
        }
        next = first + count;
        let from = (points_at + POINT_BYTES * first) as usize;
        decoded.push(Part {
            address,
            start,
            points: points_of(&bytes[from..from + (POINT_BYTES * count) as usize]),
        });
    }
    if next != points {
        return Err("aircraft-tracks: points no part holds".into());
    }
    Ok(decoded)
}

/// A tracks file opened for lookups: its fence read whole (one read, a few KB), then for a flight
/// a page of its directory and its points, one read each.
pub struct TracksFile {
    file: File,
    path: PathBuf,
    fence: Vec<u8>,
    parts: u64,
    directory_at: u64,
    points_at: u64,
    points: u64,
}

impl TracksFile {
    /// Opens the file at `path` and reads its fence, checking its extents.
    pub fn open(path: &Path) -> Result<Self, String> {
        let failed = |what: &dyn std::fmt::Display| format!("{}: {what}", path.display());
        let file = File::open(path).map_err(|error| failed(&error))?;
        let length = file.metadata().map_err(|error| failed(&error))?.len();
        let mut header = [0u8; HEADER_BYTES as usize];
        file.read_exact_at(&mut header, 0)
            .map_err(|error| failed(&error))?;
        if &header[..8] != MAGIC {
            return Err(failed(&"bad magic"));
        }
        let parts = u64::from(u32_at(&header, 8));
        let (directory_at, points_at, points) =
            layout(length, parts).ok_or_else(|| failed(&"length does not match"))?;
        let mut fence = vec![0u8; (directory_at - HEADER_BYTES) as usize];
        file.read_exact_at(&mut fence, HEADER_BYTES)
            .map_err(|error| failed(&error))?;
        Ok(TracksFile {
            file,
            path: path.to_path_buf(),
            fence,
            parts,
            directory_at,
            points_at,
            points,
        })
    }

    /// The bytes read to open it.
    pub fn opened_bytes(&self) -> u64 {
        HEADER_BYTES + self.fence.len() as u64
    }

    fn read_at(&self, length: u64, at: u64) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; length as usize];
        self.file
            .read_exact_at(&mut bytes, at)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        Ok(bytes)
    }

    /// The parts of the flight `(address, start)` in their order along it, and the bytes read;
    /// none when the file holds no such flight. Its first part lies in the last page whose first
    /// key is before it (a flight's parts may run on into the next pages).
    pub fn flight(&self, address: u32, start: u32) -> Result<(Vec<Vec<TrackPoint>>, u64), String> {
        let key = (address, start);
        let (fence, _) = self.fence.as_chunks::<{ FENCE_BYTES as usize }>();
        let mut page = fence
            .partition_point(|first| (u32_at(first, 0), u32_at(first, 4)) < key)
            .saturating_sub(1) as u64;
        let (mut found, mut read) = (Vec::new(), 0);
        while page * PAGE_PARTS < self.parts {
            let first = page * PAGE_PARTS;
            let count = PAGE_PARTS.min(self.parts - first);
            let directory =
                self.read_at(PART_BYTES * count, self.directory_at + PART_BYTES * first)?;
            read += directory.len() as u64;
            for entry in directory.chunks_exact(PART_BYTES as usize) {
                let [entry_address, entry_start, from, points] =
                    std::array::from_fn(|field| u32_at(entry, 4 * field));
                match (entry_address, entry_start).cmp(&key) {
                    std::cmp::Ordering::Less => continue,
                    std::cmp::Ordering::Greater => return Ok((found, read)),
                    std::cmp::Ordering::Equal => {}
                }
                let (from, points) = (u64::from(from), u64::from(points));
                if from + points > self.points {
                    return Err(format!(
                        "{}: a part outside the points",
                        self.path.display()
                    ));
                }
                let bytes =
                    self.read_at(POINT_BYTES * points, self.points_at + POINT_BYTES * from)?;
                read += bytes.len() as u64;
                found.push(points_of(&bytes));
            }
            page += 1;
        }
        Ok((found, read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flight's parts come back in their order and to the stored precision; another flight's
    /// and a flight not in the file are apart; a file shorter than its parts, or with a part
    /// claiming points it does not hold, is refused without reading past its end.
    #[test]
    fn a_flight_reads_back_its_parts() {
        let part = |address: u32, start: u32, lat: f64| Part {
            address,
            start,
            points: vec![[lat, 14.5], [lat + 0.01, 14.51]],
        };
        let bytes = encode(vec![
            part(0x4b1805, 7, 50.0),
            part(0x4ca1aa, 1_780_000_000, 50.1),
            part(0x4ca1aa, 1_780_000_000, 50.2),
            part(0x4ca1aa, 1_780_009_999, 49.0),
        ]);
        let path = std::env::temp_dir().join(format!("qm-tracks-{}.test", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let tracks = TracksFile::open(&path).unwrap();
        let (parts, read) = tracks.flight(0x4ca1aa, 1_780_000_000).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], vec![[50.1, 14.5], [50.11, 14.51]]);
        assert_eq!(parts[1][0], [50.2, 14.5]);
        assert!(read > 0 && tracks.opened_bytes() > 0);
        assert_eq!(tracks.flight(0x4b1805, 7).unwrap().0.len(), 1);
        assert!(tracks.flight(0x4ca1aa, 8).unwrap().0.is_empty());
        assert!(tracks.flight(0xffffff, 0).unwrap().0.is_empty());
        assert_eq!(decode(&bytes).unwrap().len(), 4);
        // One part declared and none stored; then a part claiming every point there could be.
        let mut short = bytes[..12].to_vec();
        short[8] = 1;
        assert!(decode(&short).is_err());
        std::fs::write(&path, &short).unwrap();
        assert!(TracksFile::open(&path).is_err());
        // The first part's point count, after the header and the one page's fence entry.
        let mut greedy = bytes.clone();
        greedy[20 + 12..20 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&greedy).is_err());
        std::fs::write(&path, &greedy).unwrap();
        assert!(
            TracksFile::open(&path)
                .unwrap()
                .flight(0x4b1805, 7)
                .is_err()
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            tracks_path(Path::new("/y"), 0x4ca1aa),
            Path::new("/y/aircraft-tracks/aa.aircraft-tracks")
        );
    }

    /// A flight whose parts run across pages is read whole from the page before its key's; a
    /// flight first in a page and the last flight of the file are found; a key between two
    /// flights finds nothing, and a fence naming a wrong first key is refused.
    #[test]
    fn a_flight_across_pages_reads_whole() {
        let part = |address: u32, start: u32| Part {
            address,
            start,
            points: vec![[f64::from(start) / 1e3, 14.0]],
        };
        let mut parts: Vec<Part> = (0..600).map(|k| part(10 + 2 * k, k)).collect();
        // Five parts of one flight around the first page boundary (parts 254..259 once sorted).
        parts.splice(254..254, (0..5).map(|_| part(517, 0)));
        let bytes = encode(parts);
        let path =
            std::env::temp_dir().join(format!("qm-tracks-pages-{}.test", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let tracks = TracksFile::open(&path).unwrap();
        assert_eq!(tracks.opened_bytes(), 12 + 3 * 8);
        let (across, read) = tracks.flight(517, 0).unwrap();
        assert_eq!(across.len(), 5);
        assert!(read < 3 * 256 * 16 + 5 * 8, "{read}");
        assert_eq!(tracks.flight(10 + 2 * 256, 256).unwrap().0.len(), 1);
        assert_eq!(tracks.flight(10 + 2 * 599, 599).unwrap().0.len(), 1);
        assert!(tracks.flight(11, 0).unwrap().0.is_empty());
        assert!(tracks.flight(u32::MAX, 0).unwrap().0.is_empty());
        assert_eq!(decode(&bytes).unwrap().len(), 605);
        let mut wrong_fence = bytes;
        wrong_fence[12 + 8] ^= 1;
        assert!(decode(&wrong_fence).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
