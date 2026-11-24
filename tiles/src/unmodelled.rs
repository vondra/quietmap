//! Nearby unmodelled sites (active military firing ranges and training areas) indexed by z9 square.
//!
//! One compact file per year (`<year root>/unmodelled-sites`). Each grouped site is stored once
//! with its exact z30 outer and hole rings and posted into every z9 square its reach-expanded parts
//! intersect, so a click reads only its square's candidates.
//!
//! ```text
//! magic "qmunm1\n\0", u32 sites, u32 squares, u32 postings, u32 strings_bytes
//! square_table[squares]:  (u32 square_key, u32 postings_offset, u32 postings_len), sorted by key
//! postings[postings]:     u32 site_index
//! site_offsets[sites+1]:  u32 byte offset into site_blob (monotonic; last == site_blob_len)
//! site_blob:              per site: u64 group_key, u8 layer, u8 kind, u16 pad, u32 reach_m,
//!                         f64 area_m2, u32 name_offset, u32 name_len, u32 part_count,
//!                         per part: f32 bbox[4] (min_lon, min_lat, max_lon, max_lat), u32 ring_count,
//!                         per ring: u32 vertex_count, (i32 gx, i32 gy)[vertex_count]
//!                         (ring 0 is the outer ring; rings 1.. are inner hole rings)
//! strings_blob:           UTF-8 site names
//! ```

use crate::FormatError;
use crate::geo::{MAX_LATITUDE_DEG, Mercator, TileId, WGS84_A_M};
use crate::little_endian::u32_at;
use crate::sources::Layer;
use crate::square::{SQUARES_PER_AXIS, Square};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const UNMODELLED_SITES_FILE: &str = "unmodelled-sites";
const MAGIC: &[u8; 8] = b"qmunm1\n\0";
const HEADER_BYTES: usize = 24;
const SQUARE_ENTRY_BYTES: usize = 12;
const Z30_QUANTUM_M: f64 = 0.037_322_767_717_044_72;
const MAX_REPORTED: usize = 5;

/// Notification distance (m): 3 km for sites under 5 km², 10 km at or above 5 km².
pub const SMALL_SITE_REACH_M: u32 = 3_000;
pub const LARGE_SITE_REACH_M: u32 = 10_000;
pub const LARGE_SITE_THRESHOLD_M2: f64 = 5_000_000.0;

pub fn notification_reach_m(area_m2: f64) -> u32 {
    if area_m2 >= LARGE_SITE_THRESHOLD_M2 {
        LARGE_SITE_REACH_M
    } else {
        SMALL_SITE_REACH_M
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum UnmodelledKind {
    MilitaryRange = 1,
    MilitaryTrainingArea = 2,
    MilitaryDangerArea = 3,
}

impl UnmodelledKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::MilitaryRange => "military_range",
            Self::MilitaryTrainingArea => "military_training_area",
            Self::MilitaryDangerArea => "military_danger_area",
        }
    }

    pub fn from_code(code: u8) -> Result<Self, FormatError> {
        match code {
            1 => Ok(Self::MilitaryRange),
            2 => Ok(Self::MilitaryTrainingArea),
            3 => Ok(Self::MilitaryDangerArea),
            _ => Err(FormatError("unmodelled-sites: unknown site kind")),
        }
    }
}

/// One polygon part: ring 0 is the outer ring; rings 1.. are inner hole rings.
#[derive(Clone, Debug, PartialEq)]
pub struct SitePart {
    pub rings: Vec<Vec<(i32, i32)>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SiteRecord {
    pub group_key: u64,
    pub layer: Layer,
    pub kind: UnmodelledKind,
    pub name: String,
    pub area_m2: f64,
    pub reach_m: u32,
    pub parts: Vec<SitePart>,
}

/// One nearby unmodelled site matched at a click.
#[derive(Clone, Debug, PartialEq)]
pub struct NearbySite {
    pub group_key: u64,
    pub layer: Layer,
    pub kind: UnmodelledKind,
    pub name: String,
    pub inside: bool,
    pub distance_m: f64,
    pub area_m2: f64,
}

fn z30_to_lonlat(gx: i32, gy: i32) -> [f64; 2] {
    let x = (i64::from(gx) - (1 << 29)) as f64 * Z30_QUANTUM_M;
    let y = (i64::from(gy) - (1 << 29)) as f64 * Z30_QUANTUM_M;
    let lat = 2.0 * (y / WGS84_A_M).exp().atan() - std::f64::consts::FRAC_PI_2;
    [(x / WGS84_A_M).to_degrees(), lat.to_degrees()]
}

fn square_key(square: Square) -> u32 {
    square.y * SQUARES_PER_AXIS + square.x
}

fn square_of_lonlat(lon: f64, lat: f64) -> Square {
    Square::of(TileId::containing(Mercator::from_degrees(lat, lon)))
}

fn longitude_delta(delta: f64) -> f64 {
    (delta + 180.0).rem_euclid(360.0) - 180.0
}

/// Bounding box of an outer ring in degrees (unwrapped relative to its first vertex when needed,
/// returning `None` for empty rings or rings spanning more than 180° of longitude).
fn part_bounds(part: &SitePart) -> Option<[f64; 4]> {
    let outer = part.rings.first()?;
    if outer.len() < 3 {
        return None;
    }
    let first = z30_to_lonlat(outer[0].0, outer[0].1);
    let (mut min_lon, mut max_lon) = (first[0], first[0]);
    let (mut min_lat, mut max_lat) = (first[1], first[1]);
    for &(gx, gy) in outer.iter().skip(1) {
        let [lon, lat] = z30_to_lonlat(gx, gy);
        let unwrapped = first[0] + longitude_delta(lon - first[0]);
        min_lon = min_lon.min(unwrapped);
        max_lon = max_lon.max(unwrapped);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
    }
    if max_lon - min_lon > 180.0 {
        return None;
    }
    Some([min_lon, min_lat, max_lon, max_lat])
}

fn add_covered_squares(bounds: [f64; 4], reach_m: u32, squares: &mut BTreeSet<u32>) {
    let reach = f64::from(reach_m) + 250.0;
    let dlat = reach / 111_000.0;
    let min_lat = (bounds[1] - dlat).clamp(-MAX_LATITUDE_DEG, MAX_LATITUDE_DEG);
    let max_lat = (bounds[3] + dlat).clamp(-MAX_LATITUDE_DEG, MAX_LATITUDE_DEG);
    let cos_lat = min_lat
        .abs()
        .max(max_lat.abs())
        .min(84.5)
        .to_radians()
        .cos()
        .max(0.05);
    let dlon = reach / (111_000.0 * cos_lat);
    let min_lon = bounds[0] - dlon;
    let max_lon = bounds[2] + dlon;
    if max_lon - min_lon >= 360.0 {
        let y0 = square_of_lonlat(0.0, max_lat).y;
        let y1 = square_of_lonlat(0.0, min_lat).y;
        for y in y0..=y1 {
            for x in 0..SQUARES_PER_AXIS {
                squares.insert(square_key(Square { x, y }));
            }
        }
        return;
    }
    // Split longitude span cleanly across the antimeridian if expanded bounds cross ±180°.
    let spans: Vec<(f64, f64)> = if min_lon < -180.0 {
        vec![(min_lon + 360.0, 179.999_999), (-180.0, max_lon)]
    } else if max_lon >= 180.0 {
        vec![(min_lon, 179.999_999), (-180.0, max_lon - 360.0)]
    } else {
        vec![(min_lon, max_lon)]
    };
    for (lo, hi) in spans {
        let nw = square_of_lonlat(lo.clamp(-180.0, 179.999_999), max_lat);
        let se = square_of_lonlat(hi.clamp(-180.0, 179.999_999), min_lat);
        for x in nw.x..=se.x {
            for y in nw.y..=se.y {
                squares.insert(square_key(Square { x, y }));
            }
        }
    }
}

/// Atomically writes `unmodelled-sites` via a sibling `.partial` file.
pub fn write(path: &Path, sites: &[SiteRecord]) -> Result<(), String> {
    let partial = path.with_extension("partial");
    for candidate in [path, partial.as_path()] {
        if candidate.exists() {
            return Err(format!(
                "unmodelled-sites output already exists: {}",
                candidate.display()
            ));
        }
    }
    let mut square_postings: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut site_offsets: Vec<u32> = Vec::with_capacity(sites.len() + 1);
    let mut site_blob: Vec<u8> = Vec::new();
    let mut strings_blob: Vec<u8> = Vec::new();

    for (index, site) in sites.iter().enumerate() {
        let site_index = u32::try_from(index).map_err(|_| "too many unmodelled sites")?;
        let mut valid_parts: Vec<([f64; 4], &SitePart)> = Vec::new();
        let mut covered: BTreeSet<u32> = BTreeSet::new();
        for part in &site.parts {
            if let Some(bounds) = part_bounds(part) {
                add_covered_squares(bounds, site.reach_m, &mut covered);
                valid_parts.push((bounds, part));
            }
        }
        site_offsets.push(u32::try_from(site_blob.len()).map_err(|_| "site blob overflow")?);
        let name_offset = u32::try_from(strings_blob.len()).map_err(|_| "strings blob overflow")?;
        let name_bytes = site.name.as_bytes();
        let name_len = u32::try_from(name_bytes.len()).map_err(|_| "site name too long")?;
        strings_blob.extend_from_slice(name_bytes);

        site_blob.extend_from_slice(&site.group_key.to_le_bytes());
        site_blob.push(site.layer as u8);
        site_blob.push(site.kind as u8);
        site_blob.extend_from_slice(&0u16.to_le_bytes());
        site_blob.extend_from_slice(&site.reach_m.to_le_bytes());
        site_blob.extend_from_slice(&site.area_m2.to_le_bytes());
        site_blob.extend_from_slice(&name_offset.to_le_bytes());
        site_blob.extend_from_slice(&name_len.to_le_bytes());
        site_blob.extend_from_slice(&(valid_parts.len() as u32).to_le_bytes());
        for (bounds, part) in valid_parts {
            for value in bounds {
                site_blob.extend_from_slice(&(value as f32).to_le_bytes());
            }
            let rings: Vec<&Vec<(i32, i32)>> =
                part.rings.iter().filter(|ring| ring.len() >= 3).collect();
            site_blob.extend_from_slice(&(rings.len() as u32).to_le_bytes());
            for ring in rings {
                site_blob.extend_from_slice(&(ring.len() as u32).to_le_bytes());
                for &(gx, gy) in ring {
                    site_blob.extend_from_slice(&gx.to_le_bytes());
                    site_blob.extend_from_slice(&gy.to_le_bytes());
                }
            }
        }
        for key in covered {
            square_postings.entry(key).or_default().push(site_index);
        }
    }
    site_offsets.push(u32::try_from(site_blob.len()).map_err(|_| "site blob overflow")?);

    let mut postings: Vec<u32> = Vec::new();
    let mut square_table: Vec<(u32, u32, u32)> = Vec::with_capacity(square_postings.len());
    for (key, list) in square_postings {
        let offset = u32::try_from(postings.len()).map_err(|_| "postings overflow")?;
        let len = u32::try_from(list.len()).map_err(|_| "square postings overflow")?;
        postings.extend_from_slice(&list);
        square_table.push((key, offset, len));
    }

    let mut file = File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let mut header = Vec::with_capacity(HEADER_BYTES);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&(sites.len() as u32).to_le_bytes());
    header.extend_from_slice(&(square_table.len() as u32).to_le_bytes());
    header.extend_from_slice(&(postings.len() as u32).to_le_bytes());
    header.extend_from_slice(&(strings_blob.len() as u32).to_le_bytes());
    file.write_all(&header).map_err(|e| e.to_string())?;
    for (key, offset, len) in square_table {
        file.write_all(&key.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&offset.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&len.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    for site_index in postings {
        file.write_all(&site_index.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    for offset in site_offsets {
        file.write_all(&offset.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    file.write_all(&site_blob).map_err(|e| e.to_string())?;
    file.write_all(&strings_blob).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
}

struct DecodedSite {
    group_key: u64,
    layer: Layer,
    kind: UnmodelledKind,
    reach_m: u32,
    area_m2: f64,
    name: String,
    parts: Vec<DecodedPart>,
}

struct DecodedPart {
    bbox: [f64; 4],
    rings: Vec<Vec<[f64; 2]>>,
}

fn read_exact_at(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>, FormatError> {
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| FormatError("unmodelled-sites: seek failed"))?;
    file.read_exact(&mut buf)
        .map_err(|_| FormatError("unmodelled-sites: truncated file"))?;
    Ok(buf)
}

fn decode_layer(code: u8) -> Result<Layer, FormatError> {
    Layer::ALL
        .get(usize::from(code))
        .copied()
        .ok_or(FormatError("unmodelled-sites: unknown layer"))
}

fn decode_site(
    bytes: &[u8],
    file: &mut File,
    strings_start: u64,
    strings_bytes: usize,
) -> Result<DecodedSite, FormatError> {
    let err = FormatError("unmodelled-sites: truncated site record");
    if bytes.len() < 36 {
        return Err(err);
    }
    let group_key = u64::from_le_bytes(bytes[0..8].try_into().map_err(|_| err)?);
    let layer = decode_layer(bytes[8])?;
    let kind = UnmodelledKind::from_code(bytes[9])?;
    let reach_m = u32_at(bytes, 12);
    let area_m2 = f64::from_le_bytes(bytes[16..24].try_into().map_err(|_| err)?);
    let name_offset = u32_at(bytes, 24) as usize;
    let name_len = u32_at(bytes, 28) as usize;
    let part_count = u32_at(bytes, 32) as usize;
    if name_offset
        .checked_add(name_len)
        .is_none_or(|end| end > strings_bytes)
    {
        return Err(FormatError("unmodelled-sites: name out of range"));
    }
    let name = if name_len == 0 {
        String::new()
    } else {
        let raw = read_exact_at(file, strings_start + name_offset as u64, name_len)?;
        String::from_utf8(raw).map_err(|_| FormatError("unmodelled-sites: invalid utf-8 name"))?
    };

    let mut pos = 36usize;
    let mut parts = Vec::with_capacity(part_count);
    for _ in 0..part_count {
        if pos + 20 > bytes.len() {
            return Err(err);
        }
        let f32_at = |at: usize| -> Result<f64, FormatError> {
            Ok(f64::from(f32::from_le_bytes(
                bytes[at..at + 4].try_into().map_err(|_| err)?,
            )))
        };
        let bbox = [
            f32_at(pos)?,
            f32_at(pos + 4)?,
            f32_at(pos + 8)?,
            f32_at(pos + 12)?,
        ];
        let ring_count = u32_at(bytes, pos + 16) as usize;
        pos += 20;
        let mut rings = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            if pos + 4 > bytes.len() {
                return Err(err);
            }
            let vertex_count = u32_at(bytes, pos) as usize;
            pos += 4;
            let ring_bytes = vertex_count.checked_mul(8).ok_or(err)?;
            if pos
                .checked_add(ring_bytes)
                .is_none_or(|end| end > bytes.len())
            {
                return Err(err);
            }
            let mut ring = Vec::with_capacity(vertex_count);
            for _ in 0..vertex_count {
                let gx = i32::from_le_bytes(bytes[pos..pos + 4].try_into().map_err(|_| err)?);
                let gy = i32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().map_err(|_| err)?);
                pos += 8;
                ring.push(z30_to_lonlat(gx, gy));
            }
            rings.push(ring);
        }
        parts.push(DecodedPart { bbox, rings });
    }
    if pos != bytes.len() {
        return Err(FormatError("unmodelled-sites: trailing bytes in site"));
    }
    Ok(DecodedSite {
        group_key,
        layer,
        kind,
        reach_m,
        area_m2,
        name,
        parts,
    })
}

fn ring_contains(ring: &[[f64; 2]], point: [f64; 2]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let reference = ring[0][0];
    let px = reference + longitude_delta(point[0] - reference);
    let py = point[1];
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let yi = ring[i][1];
        let yj = ring[j][1];
        if (yi > py) != (yj > py) {
            let xi = reference + longitude_delta(ring[i][0] - reference);
            let xj = reference + longitude_delta(ring[j][0] - reference);
            if px < (xj - xi) * (py - yi) / (yj - yi) + xi {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn part_contains(part: &DecodedPart, point: [f64; 2]) -> bool {
    // Part bounds are stored as f32; keep a small latitude margin so boundary rounding never
    // rejects a click or vertex near the north or south edge.
    const BBOX_MARGIN_DEG: f64 = 1e-4;
    if point[1] < part.bbox[1] - BBOX_MARGIN_DEG || point[1] > part.bbox[3] + BBOX_MARGIN_DEG {
        return false;
    }
    let Some(outer) = part.rings.first() else {
        return false;
    };
    if !ring_contains(outer, point) {
        return false;
    }
    !part.rings[1..]
        .iter()
        .any(|hole| ring_contains(hole, point))
}

fn site_contains_point(site: &DecodedSite, point: [f64; 2]) -> bool {
    site.parts.iter().any(|part| part_contains(part, point))
}

fn ring_distance_m(ring: &[[f64; 2]], point: [f64; 2]) -> f64 {
    if ring.is_empty() {
        return f64::INFINITY;
    }
    let m_per_deg_lat = 111_132.95;
    let m_per_deg_lon = 111_319.49 * point[1].to_radians().cos().max(0.01);
    let mut best_sq = f64::INFINITY;
    let n = ring.len();
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let ax = longitude_delta(a[0] - point[0]) * m_per_deg_lon;
        let ay = (a[1] - point[1]) * m_per_deg_lat;
        let bx = longitude_delta(b[0] - point[0]) * m_per_deg_lon;
        let by = (b[1] - point[1]) * m_per_deg_lat;
        let abx = bx - ax;
        let aby = by - ay;
        let len_sq = abx * abx + aby * aby;
        let t = if len_sq > 0.0 {
            (-(ax * abx + ay * aby) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cx = ax + t * abx;
        let cy = ay + t * aby;
        best_sq = best_sq.min(cx * cx + cy * cy);
    }
    best_sq.sqrt()
}

fn site_distance_m(site: &DecodedSite, point: [f64; 2]) -> (bool, f64) {
    if site_contains_point(site, point) {
        return (true, 0.0);
    }
    let mut best = f64::INFINITY;
    for part in &site.parts {
        // Quick latitude bbox rejection before edge loop.
        let dlat = (f64::from(site.reach_m) + 500.0) / 111_000.0;
        if point[1] < part.bbox[1] - dlat || point[1] > part.bbox[3] + dlat {
            continue;
        }
        for ring in &part.rings {
            best = best.min(ring_distance_m(ring, point));
        }
    }
    (false, best)
}

/// Shared-boundary tolerance when checking whether a mapped sub-sector lies inside a larger range
/// (OSM training sectors often share outer ways or roads with the enclosing training area).
const COVER_EDGE_TOLERANCE_M: f64 = 10.0;

fn ring_within_m(ring: &[[f64; 2]], point: [f64; 2], limit_m: f64) -> bool {
    if ring.is_empty() {
        return false;
    }
    let m_per_deg_lat = 111_132.95;
    let m_per_deg_lon = 111_319.49 * point[1].to_radians().cos().max(0.01);
    let dlat = limit_m / m_per_deg_lat + 1e-6;
    let limit_sq = limit_m * limit_m;
    let n = ring.len();
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        if a[1].max(b[1]) < point[1] - dlat || a[1].min(b[1]) > point[1] + dlat {
            continue;
        }
        let ax = longitude_delta(a[0] - point[0]) * m_per_deg_lon;
        let bx = longitude_delta(b[0] - point[0]) * m_per_deg_lon;
        if ax.max(bx) < -limit_m || ax.min(bx) > limit_m {
            continue;
        }
        let ay = (a[1] - point[1]) * m_per_deg_lat;
        let by = (b[1] - point[1]) * m_per_deg_lat;
        let abx = bx - ax;
        let aby = by - ay;
        let len_sq = abx * abx + aby * aby;
        let t = if len_sq > 0.0 {
            (-(ax * abx + ay * aby) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cx = ax + t * abx;
        let cy = ay + t * aby;
        if cx * cx + cy * cy <= limit_sq {
            return true;
        }
    }
    false
}

fn site_boundary_within_m(site: &DecodedSite, point: [f64; 2], limit_m: f64) -> bool {
    let dlat = (limit_m + 50.0) / 111_000.0;
    for part in &site.parts {
        if point[1] < part.bbox[1] - dlat || point[1] > part.bbox[3] + dlat {
            continue;
        }
        for ring in &part.rings {
            if ring_within_m(ring, point, limit_m) {
                return true;
            }
        }
    }
    false
}

/// True when every outer ring of `child` lies inside `parent` (outer minus holes), allowing
/// vertices on or within [`COVER_EDGE_TOLERANCE_M`] of `parent`'s boundary while requiring at
/// least one strictly interior point so adjacent touching ranges are never suppressed.
fn covers(parent: &DecodedSite, child: &DecodedSite) -> bool {
    if child.parts.is_empty() || parent.parts.is_empty() {
        return false;
    }
    let dlat = (COVER_EDGE_TOLERANCE_M + 50.0) / 111_000.0;
    let parent_min_lat = parent
        .parts
        .iter()
        .map(|p| p.bbox[1])
        .fold(f64::INFINITY, f64::min);
    let parent_max_lat = parent
        .parts
        .iter()
        .map(|p| p.bbox[3])
        .fold(f64::NEG_INFINITY, f64::max);
    child.parts.iter().all(|part| {
        if part.bbox[1] < parent_min_lat - dlat || part.bbox[3] > parent_max_lat + dlat {
            return false;
        }
        let Some(outer) = part.rings.first() else {
            return false;
        };
        if outer.is_empty() {
            return false;
        }
        let mut any_strictly_inside = false;
        for &pt in outer {
            if site_contains_point(parent, pt) {
                any_strictly_inside = true;
            } else if !site_boundary_within_m(parent, pt, COVER_EDGE_TOLERANCE_M) {
                return false;
            }
        }
        if any_strictly_inside {
            return true;
        }
        let vertices = if outer.len() > 1 && outer.last() == outer.first() {
            &outer[..outer.len() - 1]
        } else {
            outer.as_slice()
        };
        let reference = vertices[0][0];
        let n = vertices.len() as f64;
        let mean_lon = reference
            + vertices
                .iter()
                .map(|p| longitude_delta(p[0] - reference))
                .sum::<f64>()
                / n;
        let mean_lat = vertices.iter().map(|p| p[1]).sum::<f64>() / n;
        site_contains_point(parent, [mean_lon, mean_lat])
    })
}

/// Queries `<year root>/unmodelled-sites` at `(lat, lon)`.
///
/// An absent file (legacy release) returns empty; a present corrupt or truncated file fails.
pub fn query_file(path: &Path, lat: f64, lon: f64) -> Result<Vec<NearbySite>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    query_open_file(&mut file, lat, lon).map_err(|error| format!("{}: {error}", path.display()))
}

fn query_open_file(file: &mut File, lat: f64, lon: f64) -> Result<Vec<NearbySite>, FormatError> {
    let file_len = file
        .metadata()
        .map_err(|_| FormatError("unmodelled-sites: stat failed"))?
        .len();
    if file_len < HEADER_BYTES as u64 {
        return Err(FormatError("unmodelled-sites: truncated header"));
    }
    let header = read_exact_at(file, 0, HEADER_BYTES)?;
    if &header[..8] != MAGIC {
        return Err(FormatError("unmodelled-sites: bad magic"));
    }
    let site_count = u32_at(&header, 8) as usize;
    let square_count = u32_at(&header, 12) as usize;
    let postings_count = u32_at(&header, 16) as usize;
    let strings_bytes = u32_at(&header, 20) as usize;

    let square_table_start = HEADER_BYTES as u64;
    let square_table_bytes = square_count
        .checked_mul(SQUARE_ENTRY_BYTES)
        .ok_or(FormatError("unmodelled-sites: table overflow"))?;
    let postings_start = square_table_start + square_table_bytes as u64;
    let postings_bytes = postings_count
        .checked_mul(4)
        .ok_or(FormatError("unmodelled-sites: postings overflow"))?;
    let offsets_start = postings_start + postings_bytes as u64;
    let offsets_bytes = site_count
        .checked_add(1)
        .and_then(|n| n.checked_mul(4))
        .ok_or(FormatError("unmodelled-sites: offsets overflow"))?;
    let site_blob_start = offsets_start + offsets_bytes as u64;
    if site_blob_start + strings_bytes as u64 > file_len {
        return Err(FormatError("unmodelled-sites: truncated sections"));
    }
    let last_offset_bytes = read_exact_at(file, offsets_start + (site_count * 4) as u64, 4)?;
    let site_blob_bytes = u32_at(&last_offset_bytes, 0) as u64;
    let strings_start = site_blob_start + site_blob_bytes;
    if strings_start + strings_bytes as u64 != file_len {
        return Err(FormatError("unmodelled-sites: file length mismatch"));
    }
    if square_count == 0 || site_count == 0 {
        return Ok(Vec::new());
    }

    let target_key = square_key(square_of_lonlat(lon, lat));
    let table = read_exact_at(file, square_table_start, square_table_bytes)?;
    let mut lo = 0usize;
    let mut hi = square_count;
    let mut found = None;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let key = u32_at(&table, mid * SQUARE_ENTRY_BYTES);
        match key.cmp(&target_key) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => {
                found = Some(mid);
                break;
            }
        }
    }
    let Some(entry_idx) = found else {
        return Ok(Vec::new());
    };
    let p_offset = u32_at(&table, entry_idx * SQUARE_ENTRY_BYTES + 4) as usize;
    let p_len = u32_at(&table, entry_idx * SQUARE_ENTRY_BYTES + 8) as usize;
    if p_offset
        .checked_add(p_len)
        .is_none_or(|end| end > postings_count)
    {
        return Err(FormatError(
            "unmodelled-sites: postings range out of bounds",
        ));
    }
    let posting_raw = read_exact_at(file, postings_start + (p_offset * 4) as u64, p_len * 4)?;
    let click = [(lon + 180.0).rem_euclid(360.0) - 180.0, lat];

    let mut matched: Vec<(DecodedSite, bool, f64)> = Vec::new();
    for chunk in posting_raw.chunks_exact(4) {
        let idx = u32::from_le_bytes(chunk.try_into().unwrap()) as usize;
        if idx >= site_count {
            return Err(FormatError("unmodelled-sites: site index out of range"));
        }
        let pair = read_exact_at(file, offsets_start + (idx * 4) as u64, 8)?;
        let start = u32_at(&pair, 0) as u64;
        let end = u32_at(&pair, 4) as u64;
        if end < start || end > site_blob_bytes {
            return Err(FormatError("unmodelled-sites: bad site offset"));
        }
        let raw = read_exact_at(file, site_blob_start + start, (end - start) as usize)?;
        let site = decode_site(&raw, file, strings_start, strings_bytes)?;
        let (inside, distance_m) = site_distance_m(&site, click);
        if inside || distance_m <= f64::from(site.reach_m) {
            matched.push((site, inside, distance_m));
        }
    }

    // Suppress nested child sites fully covered by a larger matched site (equal area -> lower id wins).
    let kept: Vec<usize> = (0..matched.len())
        .filter(|&i| {
            let child = &matched[i].0;
            !(0..matched.len()).any(|j| {
                if i == j {
                    return false;
                }
                let parent = &matched[j].0;
                let beats = parent.area_m2 > child.area_m2
                    || (parent.area_m2 == child.area_m2 && parent.group_key < child.group_key);
                beats && covers(parent, child)
            })
        })
        .collect();

    let mut out: Vec<NearbySite> = kept
        .into_iter()
        .map(|i| {
            let (site, inside, distance_m) = &matched[i];
            NearbySite {
                group_key: site.group_key,
                layer: site.layer,
                kind: site.kind,
                name: site.name.clone(),
                inside: *inside,
                distance_m: *distance_m,
                area_m2: site.area_m2,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.inside
            .cmp(&a.inside)
            .then_with(|| a.distance_m.total_cmp(&b.distance_m))
            .then_with(|| a.group_key.cmp(&b.group_key))
    });
    out.truncate(MAX_REPORTED);
    Ok(out)
}

#[cfg(test)]
#[path = "unmodelled_tests.rs"]
mod tests;
