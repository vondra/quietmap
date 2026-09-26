//! Level-crossing horns: FRA + Transport Canada inventories → horn approach
//! rows appended to finalized `railways.arrow` (rail_type 6, soundings in the
//! passenger slots, freight estimated zero).
//!
//! Per sounding crossing, one approach segment per travel direction ends at
//! the crossing, oriented along the nearest finalized track within 60 m and
//! `min(1/4 mi, v·20 s)` long (49 CFR 222.21: sound 15–20 s, whistle post at
//! most 1/4 mi out; CROR 14(l) likewise). Each approach carries half the
//! crossing's soundings; overlapping same-direction approaches merge onto
//! shared pieces with the max soundings (one train sounds once through).
//!
//! Silence: full-day quiet zones (FRA whistleban 1) and Chicago-excused
//! crossings (3) sound nothing; partial zones (2) are silent 22–07. FRA
//! day (6a–6p) / night (6p–6a) thru counts map to END periods uniform within
//! blocks; TC daily totals split flat 12/4/8. TC carries no cessation data —
//! every TC horn is labelled cessation-unknown via its dataset name.
//!
//! Approaches file under the crossing's square whole, even when the 402 m
//! zone overhangs a square border: serving prunes batches by bbox envelope,
//! so the overhanging row is found from either side, exactly once.
//!
//! Runs after `finalize_year` (world build order) and is idempotent: horn
//! rows carry negative synthetic osm_ids, and crossings already present are
//! skipped. Horn rows must never enter the finalizer (their periods are
//! stamped, not split from daily counts).

use arrow::array::{
    Array, ArrayRef, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, StringArray, UInt16Array, UInt32Array, UInt8Array,
};
use arrow::compute::{concat_batches, take};
use grid::Square;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::encode::CONTRACT_KEY;

/// Horn approach marker: `railways.arrow` rail_type for horn rows.
pub const HORN_RAIL_TYPE: u8 = 6;
/// `us-fra-crossings` dataset id (passenger provenance of FRA horns).
pub const SOURCE_ID_FRA: u16 = 111;
/// `ca-tc-crossings` dataset id (passenger provenance of TC horns).
pub const SOURCE_ID_TC: u16 = 112;
/// Crossing-to-track match gate [m]: FRA/TC coordinates sit on the crossing.
const MATCH_GATE_M: f64 = 60.0;
/// Horn zone cap [m]: 1/4 mile (49 CFR 222.21 whistle-post limit).
const HORN_ZONE_M: f64 = 402.336;
/// Sounding lead time [s]: the regulation's 15–20 s window, long end.
const SOUNDING_LEAD_S: f64 = 20.0;
/// TC inventory offset into the negative synthetic osm_id space (FRA rows take
/// −1, −2, …; TC rows take −1_000_000_001, …).
const TC_OSM_OFFSET: i64 = 1_000_000_000;
/// Same-direction gate for approach merging (cosine of travel headings).
const MERGE_DIR_DOT: f64 = 0.9;
/// Lateral gate for approach merging [m].
const MERGE_LATERAL_M: f64 = 5.0;

/// One sounding crossing from either inventory.
struct Crossing {
    /// Display id (FRA Crossing ID / TC Number).
    id: String,
    lat: f64,
    lon: f64,
    /// Soundings per END period (both directions; halved per approach).
    soundings: [f64; 3],
    /// Sounding-train speed [km/h] (None = inventory missing → horn default).
    speed_kmh: Option<f64>,
    source_id: u16,
    /// Negative synthetic osm_id, stable per retained input bytes.
    synth_osm: i64,
}

/// One directed horn approach before overlap merging.
struct Approach {
    crossing: usize,
    /// Index within the crossing (0/1: the two travel directions).
    approach_idx: usize,
    /// Matched finalized row (tags + baked identity copy from here).
    matched_row: u32,
    /// Start (whistle post) and crossing end, lon/lat degrees.
    start_lon: f64,
    start_lat: f64,
    end_lon: f64,
    end_lat: f64,
    /// Unit travel vector in local metres.
    dir_x: f64,
    dir_y: f64,
    length_m: f64,
    /// Soundings per END period on this approach (half the crossing's).
    soundings: [f64; 3],
}

#[derive(Debug, Default)]
pub struct HornStats {
    pub fra_rows: usize,
    pub tc_rows: usize,
    pub sounding: usize,
    pub silent_zone: usize,
    pub squares_touched: usize,
    pub horns_appended: usize,
    pub crossings_unmatched: usize,
    pub crossings_skipped_present: usize,
}

/// Parse both inventories and append horn rows to every finalized square.
/// Fails loudly on a non-finalized `railways.arrow` (finalize would rewrite
/// the file and drop the horns); squares without the file are skipped.
pub fn append_horns_year(
    prepared_year: &Path,
    fra_csv: &Path,
    tc_csv: &Path,
) -> Result<HornStats, String> {
    let mut stats = HornStats::default();
    let mut crossings = parse_fra(fra_csv, &mut stats)?;
    crossings.extend(parse_tc(tc_csv, &mut stats)?);
    let mut by_square: HashMap<Square, Vec<usize>> = HashMap::new();
    for (idx, crossing) in crossings.iter().enumerate() {
        by_square
            .entry(grid::square_of(crossing.lat, crossing.lon))
            .or_default()
            .push(idx);
    }
    let mut squares: Vec<(Square, Vec<usize>)> = by_square.into_iter().collect();
    squares.sort_by_key(|(square, _)| (square.x, square.y));
    for (square, idxs) in &squares {
        let dir = prepared_year
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string());
        let path = dir.join("railways.arrow");
        if !path.is_file() {
            stats.crossings_unmatched += idxs.len();
            continue;
        }
        let appended = append_horns_to_square(&dir, &crossings, idxs, &mut stats)?;
        if appended > 0 {
            stats.squares_touched += 1;
            stats.horns_appended += appended;
        }
    }
    Ok(stats)
}

// ── Inventory parsing ──

fn csv_records(path: &Path) -> Result<(Vec<String>, Vec<csv::StringRecord>), String> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|err| format!("opens {}: {err}", path.display()))?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(|err| format!("reads {} headers: {err}", path.display()))?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let mut records = Vec::new();
    for record in reader.records() {
        records.push(record.map_err(|err| format!("reads {}: {err}", path.display()))?);
    }
    Ok((headers, records))
}

fn column(headers: &[String], name: &str, path: &Path) -> Result<usize, String> {
    headers
        .iter()
        .position(|h| h == name)
        .ok_or_else(|| format!("{} lacks a {name} column", path.display()))
}

fn num(record: &csv::StringRecord, idx: usize) -> Option<f64> {
    let text = record.get(idx)?.trim();
    if text.is_empty() || text == "-" {
        return None;
    }
    text.parse::<f64>().ok()
}

/// FRA day (6a–6p) / night (6p–6a) thru counts to END periods, uniform within
/// blocks: day takes 11 day-hours + 1 night-hour, evening 4 night-hours,
/// night 1 day-hour + 7 night-hours.
fn fra_end_periods(day: f64, night: f64) -> [f64; 3] {
    [
        day * 11.0 / 12.0 + night / 12.0,
        night * 4.0 / 12.0,
        day / 12.0 + night * 7.0 / 12.0,
    ]
}

/// Partial quiet zone: soundings only 07–22. The 6a–7a day-hour and the
/// 22p–6a night-hours go silent; evening keeps 19–22 (3 night-hours).
fn fra_partial_periods(day: f64, night: f64) -> [f64; 3] {
    [day * 11.0 / 12.0 + night / 12.0, night * 3.0 / 12.0, 0.0]
}

fn parse_fra(path: &Path, stats: &mut HornStats) -> Result<Vec<Crossing>, String> {
    let (headers, records) = csv_records(path)?;
    let c_id = column(&headers, "Crossing ID", path)?;
    let c_type = column(&headers, "Crossing Type Code", path)?;
    let c_purpose = column(&headers, "Crossing Purpose Code", path)?;
    let c_position = column(&headers, "Crossing Position Code", path)?;
    let c_ban = column(&headers, "Whistleban Code", path)?;
    let c_lat = column(&headers, "Latitude", path)?;
    let c_lon = column(&headers, "Longitude", path)?;
    let c_day = column(&headers, "Total Daylight Thru Trains", path)?;
    let c_night = column(&headers, "Total Nighttime Thru Trains", path)?;
    let c_timetable = column(&headers, "Maximum Timetable Speed", path)?;
    let c_typical = column(&headers, "Typical Maximum Speed Over Crossing", path)?;
    let c_closed = column(&headers, "Crossing Closed", path)?;
    let mut out = Vec::new();
    for (row, record) in records.iter().enumerate() {
        stats.fra_rows += 1;
        if record.get(c_type) != Some("3")
            || record.get(c_purpose) != Some("1")
            || record.get(c_position) != Some("1")
            || record.get(c_closed) != Some("No")
        {
            continue;
        }
        let (Some(lat), Some(lon)) = (num(record, c_lat), num(record, c_lon)) else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }
        let day = num(record, c_day).unwrap_or(0.0).max(0.0);
        let night = num(record, c_night).unwrap_or(0.0).max(0.0);
        if day + night <= 0.0 {
            continue;
        }
        // Whistleban: 1 = 24 hr and 3 = Chicago excused are silent all day;
        // 2 = partial sounds 07–22; 0/empty sounds around the clock.
        let soundings = match record.get(c_ban) {
            Some("1") | Some("3") => {
                stats.silent_zone += 1;
                continue;
            }
            Some("2") => fra_partial_periods(day, night),
            _ => fra_end_periods(day, night),
        };
        let mph = num(record, c_typical)
            .filter(|v| *v > 0.0)
            .or_else(|| num(record, c_timetable).filter(|v| *v > 0.0));
        out.push(Crossing {
            id: record.get(c_id).unwrap_or("").to_string(),
            lat,
            lon,
            soundings,
            speed_kmh: mph.map(|v| v * 1.609344),
            source_id: SOURCE_ID_FRA,
            synth_osm: -(row as i64 + 1),
        });
    }
    stats.sounding += out.len();
    Ok(out)
}

fn parse_tc(path: &Path, stats: &mut HornStats) -> Result<Vec<Crossing>, String> {
    let (headers, records) = csv_records(path)?;
    let c_id = column(&headers, "TC Number", path)?;
    let c_access = column(&headers, "Access", path)?;
    let c_lat = column(&headers, "Latitude", path)?;
    let c_lon = column(&headers, "Longitude", path)?;
    let c_trains = column(&headers, "Trains Daily", path)?;
    let c_speed = column(&headers, "Train Max Speed (mph)", path)?;
    let mut out = Vec::new();
    for (row, record) in records.iter().enumerate() {
        stats.tc_rows += 1;
        if record.get(c_access) != Some("Public") {
            continue;
        }
        let (Some(lat), Some(lon)) = (num(record, c_lat), num(record, c_lon)) else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }
        // Daily total, no day/night split: flat 12/4/8 across END periods.
        let daily = num(record, c_trains).unwrap_or(0.0);
        if daily < 1.0 {
            continue;
        }
        let mph = num(record, c_speed).filter(|v| *v > 0.0);
        out.push(Crossing {
            id: record.get(c_id).unwrap_or("").to_string(),
            lat,
            lon,
            soundings: [daily * 12.0 / 24.0, daily * 4.0 / 24.0, daily * 8.0 / 24.0],
            speed_kmh: mph.map(|v| v * 1.609344),
            source_id: SOURCE_ID_TC,
            synth_osm: -(TC_OSM_OFFSET + row as i64 + 1),
        });
    }
    stats.sounding += out.len();
    Ok(out)
}

// ── Per-square append ──

struct BaseRow {
    start_lon: f64,
    start_lat: f64,
    end_lon: f64,
    end_lat: f64,
}

fn read_finalized(dir: &Path) -> Result<(arrow::datatypes::SchemaRef, RecordBatch), String> {
    let path = dir.join("railways.arrow");
    let bytes = std::fs::read(&path).map_err(|err| format!("reads {}: {err}", path.display()))?;
    let cursor = std::io::Cursor::new(bytes);
    let reader = arrow::ipc::reader::FileReader::try_new(cursor, None)
        .map_err(|err| format!("opens {}: {err}", path.display()))?;
    let schema = reader.schema();
    if schema.metadata().get(CONTRACT_KEY).map(String::as_str) != Some("1") {
        return Err(format!(
            "{} is not finalized (finalize it before appending horns)",
            path.display()
        ));
    }
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("reads {}: {err}", path.display()))?;
    let base = concat_batches(&schema, &batches).map_err(|err| err.to_string())?;
    Ok((schema, base))
}

fn base_rows(base: &RecordBatch) -> Result<(Vec<BaseRow>, Vec<u8>), String> {
    let start_gx = crate::write::col_i32(base, "start_gx")?;
    let start_gy = crate::write::col_i32(base, "start_gy")?;
    let end_gx = crate::write::col_i32(base, "end_gx")?;
    let end_gy = crate::write::col_i32(base, "end_gy")?;
    let rail_type = crate::write::col_u8(base, "rail_type")?;
    let mut rows = Vec::with_capacity(base.num_rows());
    let mut types = Vec::with_capacity(base.num_rows());
    for row in 0..base.num_rows() {
        let (s_lon, s_lat) =
            crate::encode::grid_cell_lonlat(start_gx.value(row), start_gy.value(row));
        let (e_lon, e_lat) = crate::encode::grid_cell_lonlat(end_gx.value(row), end_gy.value(row));
        rows.push(BaseRow {
            start_lon: s_lon,
            start_lat: s_lat,
            end_lon: e_lon,
            end_lat: e_lat,
        });
        types.push(rail_type.value(row));
    }
    Ok((rows, types))
}

/// Nearest non-horn row within the match gate: (row, crossing projection).
fn match_crossing(crossing: &Crossing, rows: &[BaseRow], types: &[u8]) -> Option<(u32, f64, f64)> {
    let mut best: Option<(f64, u32, f64, f64)> = None;
    for (row, base) in rows.iter().enumerate() {
        if types[row] == HORN_RAIL_TYPE {
            continue;
        }
        let (dist, cp_lat, cp_lon, _) = grid::geo::point_to_segment(
            crossing.lat,
            crossing.lon,
            base.start_lat,
            base.start_lon,
            base.end_lat,
            base.end_lon,
        );
        if dist <= MATCH_GATE_M && best.is_none_or(|(d, _, _, _)| dist < d) {
            best = Some((dist, row as u32, cp_lat, cp_lon));
        }
    }
    best.map(|(_, row, cp_lat, cp_lon)| (row, cp_lat, cp_lon))
}

/// Both directed approaches of one crossing along its matched row.
fn build_approaches(
    crossing_idx: usize,
    crossing: &Crossing,
    matched_row: u32,
    cp_lat: f64,
    cp_lon: f64,
    rows: &[BaseRow],
) -> [Approach; 2] {
    let base = &rows[matched_row as usize];
    // Local-metre direction of the matched row at the crossing.
    let m_lon = grid::geo::m_per_deg_lon(cp_lat.to_radians());
    let dx = grid::geo::wrapped_longitude_delta(base.start_lon, base.end_lon) * m_lon;
    let dy = (base.end_lat - base.start_lat) * grid::geo::M_PER_DEG_LAT;
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (ux, uy) = (dx / len, dy / len);
    // Approach length: the train sounds SOUNDING_LEAD_S before arrival, but
    // the whistle post stands at most 1/4 mi out.
    let v_ms = crossing.speed_kmh.unwrap_or(60.0) / 3.6;
    let zone = (v_ms * SOUNDING_LEAD_S).min(HORN_ZONE_M);
    let half = [
        crossing.soundings[0] / 2.0,
        crossing.soundings[1] / 2.0,
        crossing.soundings[2] / 2.0,
    ];
    [1.0, -1.0].map(|sign| {
        let (sx, sy) = (-sign * ux * zone, -sign * uy * zone);
        Approach {
            crossing: crossing_idx,
            approach_idx: if sign > 0.0 { 0 } else { 1 },
            matched_row,
            start_lon: cp_lon + sx / m_lon,
            start_lat: cp_lat + sy / grid::geo::M_PER_DEG_LAT,
            end_lon: cp_lon,
            end_lat: cp_lat,
            dir_x: sign * ux,
            dir_y: sign * uy,
            length_m: zone,
            soundings: half,
        }
    })
}

/// Split overlapping same-direction approaches at shared boundaries; each
/// shared window is emitted once, with the per-period max soundings (one
/// train sounds once through). The covering approach with the most soundings
/// owns the window; ties break by input order (crossing osm, approach).
fn merge_overlaps(approaches: &[Approach]) -> Vec<MergedPiece> {
    let totals: Vec<f64> = approaches.iter().map(|a| a.soundings.iter().sum()).collect();
    let mut pieces = Vec::new();
    for (idx, approach) in approaches.iter().enumerate() {
        // Split fractions along this approach from overlapping neighbours.
        let mut splits = vec![0.0, 1.0];
        let mut neighbours: Vec<(f64, f64, [f64; 3], usize)> = Vec::new();
        for (other_idx, other) in approaches.iter().enumerate() {
            if other_idx == idx {
                continue;
            }
            if approach.dir_x * other.dir_x + approach.dir_y * other.dir_y < MERGE_DIR_DOT {
                continue;
            }
            // Project the other approach's ends onto this axis (local metres
            // at this approach's start; approaches are ≤ 402 m).
            let m_lon = grid::geo::m_per_deg_lon(approach.start_lat.to_radians());
            let rel = |lon: f64, lat: f64| {
                (
                    grid::geo::wrapped_longitude_delta(approach.start_lon, lon) * m_lon,
                    (lat - approach.start_lat) * grid::geo::M_PER_DEG_LAT,
                )
            };
            let (ox0, oy0) = rel(other.start_lon, other.start_lat);
            let (ox1, oy1) = rel(other.end_lon, other.end_lat);
            let lateral = |x: f64, y: f64| (x * approach.dir_y - y * approach.dir_x).abs();
            if lateral(ox0, oy0) > MERGE_LATERAL_M || lateral(ox1, oy1) > MERGE_LATERAL_M {
                continue;
            }
            let along =
                |x: f64, y: f64| (x * approach.dir_x + y * approach.dir_y) / approach.length_m;
            let (mut a, mut b) = (along(ox0, oy0), along(ox1, oy1));
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            if b <= 0.0 || a >= 1.0 {
                continue;
            }
            splits.push(a.clamp(0.0, 1.0));
            splits.push(b.clamp(0.0, 1.0));
            neighbours.push((a, b, other.soundings, other_idx));
        }
        splits.sort_by(f64::total_cmp);
        splits.dedup();
        let mut piece_idx = 0i16;
        for window in splits.windows(2) {
            let (a, b) = (window[0], window[1]);
            if b - a < 1e-6 {
                continue;
            }
            let mid = (a + b) / 2.0;
            let mut soundings = approach.soundings;
            let mut owner = idx;
            for (na, nb, ns, other) in &neighbours {
                if *na <= mid && mid <= *nb {
                    for (period, max) in soundings.iter_mut().enumerate() {
                        *max = max.max(ns[period]);
                    }
                    if totals[*other] > totals[owner]
                        || (totals[*other] == totals[owner] && *other < owner)
                    {
                        owner = *other;
                    }
                }
            }
            if owner != idx {
                continue; // the louder (or earlier) approach emits this window
            }
            pieces.push(MergedPiece {
                approach_idx: idx,
                piece_idx,
                frac0: a,
                frac1: b,
                soundings,
            });
            piece_idx += 1;
        }
    }
    pieces
}

#[derive(Debug)]
struct MergedPiece {
    approach_idx: usize,
    piece_idx: i16,
    frac0: f64,
    frac1: f64,
    soundings: [f64; 3],
}

fn append_horns_to_square(
    dir: &Path,
    crossings: &[Crossing],
    idxs: &[usize],
    stats: &mut HornStats,
) -> Result<usize, String> {
    let (schema, base) = read_finalized(dir)?;
    let (rows, types) = base_rows(&base)?;
    // Idempotency: crossings whose horn rows already exist are skipped.
    let osm_id = crate::write::col_i64(&base, "osm_id")?;
    let mut present: HashSet<i64> = HashSet::new();
    for (row, rail_type) in types.iter().enumerate() {
        if *rail_type == HORN_RAIL_TYPE {
            present.insert(osm_id.value(row));
        }
    }
    let mut approaches = Vec::new();
    for idx in idxs {
        let crossing = &crossings[*idx];
        if present.contains(&crossing.synth_osm) {
            stats.crossings_skipped_present += 1;
            continue;
        }
        let Some((matched_row, cp_lat, cp_lon)) = match_crossing(crossing, &rows, &types) else {
            stats.crossings_unmatched += 1;
            continue;
        };
        approaches.extend(build_approaches(
            *idx,
            crossing,
            matched_row,
            cp_lat,
            cp_lon,
            &rows,
        ));
    }
    if approaches.is_empty() {
        return Ok(0);
    }
    // Input-order independence: sort by (crossing osm, approach).
    approaches.sort_by_key(|a| (crossings[a.crossing].synth_osm, a.approach_idx));
    let pieces = merge_overlaps(&approaches);
    let horn_batch = build_horn_batch(&schema, &base, crossings, &approaches, &pieces)?;
    let merged = concat_batches(&schema, &[base, horn_batch]).map_err(|err| err.to_string())?;
    let (start_gx, start_gy, end_gx, end_gy) = (
        crate::write::col_i32(&merged, "start_gx")?,
        crate::write::col_i32(&merged, "start_gy")?,
        crate::write::col_i32(&merged, "end_gx")?,
        crate::write::col_i32(&merged, "end_gy")?,
    );
    let bboxes: Vec<arrow_batching::RowBbox> = (0..merged.num_rows())
        .map(|row| {
            let (s_lon, s_lat) =
                crate::encode::grid_cell_lonlat(start_gx.value(row), start_gy.value(row));
            let (e_lon, e_lat) =
                crate::encode::grid_cell_lonlat(end_gx.value(row), end_gy.value(row));
            [
                s_lat.min(e_lat),
                s_lon.min(e_lon),
                s_lat.max(e_lat),
                s_lon.max(e_lon),
            ]
        })
        .collect();
    let mut fields = Vec::new();
    let mut columns: Vec<ArrayRef> = Vec::new();
    for field in schema.fields() {
        fields.push(field.as_ref().clone());
        columns.push(merged.column_by_name(field.name()).unwrap().clone());
    }
    let mut metadata = schema.metadata().clone();
    metadata.remove(arrow_batching::QM_BLOCKS_KEY);
    let block_schema = arrow::datatypes::Schema::new_with_metadata(fields, metadata);
    let (out_schema, batches) = arrow_batching::blocked_by_z14_cell(block_schema, columns, &bboxes)
        .map_err(|err| err.to_string())?;
    let mut out = Vec::new();
    let mut writer = arrow::ipc::writer::FileWriter::try_new(&mut out, &out_schema)
        .map_err(|err| err.to_string())?;
    for batch in &batches {
        crate::rail_traffic::RailTrafficColumns::read(batch)?;
        writer.write(batch).map_err(|err| err.to_string())?;
    }
    writer.finish().map_err(|err| err.to_string())?;
    drop(writer);
    crate::write::write_atomically(dir, &out)?;
    Ok(pieces.len())
}

/// Finalized horn rows for one square: geometry + traffic fresh, tags and
/// baked identity taken from each piece's matched track row.
#[allow(clippy::too_many_lines)]
fn build_horn_batch(
    schema: &arrow::datatypes::SchemaRef,
    base: &RecordBatch,
    crossings: &[Crossing],
    approaches: &[Approach],
    pieces: &[MergedPiece],
) -> Result<RecordBatch, String> {
    let matched: Vec<u32> = pieces
        .iter()
        .map(|piece| approaches[piece.approach_idx].matched_row)
        .collect();
    let matched_idx = UInt32Array::from(matched);
    let take_col = |name: &str| -> Result<ArrayRef, String> {
        let column = base
            .column_by_name(name)
            .ok_or_else(|| format!("finalized railways Arrow lacks {name}"))?;
        take(column.as_ref(), &matched_idx, None).map_err(|err| err.to_string())
    };
    // Piece geometry: interpolate along the approach, snapping every end
    // (shared boundaries snap identically: the snap is deterministic).
    let mut start_gx = Vec::with_capacity(pieces.len());
    let mut start_gy = Vec::with_capacity(pieces.len());
    let mut end_gx = Vec::with_capacity(pieces.len());
    let mut end_gy = Vec::with_capacity(pieces.len());
    let mut length_m = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let approach = &approaches[piece.approach_idx];
        let at = |frac: f64| {
            (
                approach.start_lon + (approach.end_lon - approach.start_lon) * frac,
                approach.start_lat + (approach.end_lat - approach.start_lat) * frac,
            )
        };
        let (s_lon, s_lat) = at(piece.frac0);
        let (e_lon, e_lat) = at(piece.frac1);
        let (sgx, sgy) = grid::lonlat_to_grid(s_lon, s_lat);
        let (egx, egy) = grid::lonlat_to_grid(e_lon, e_lat);
        start_gx.push(sgx);
        start_gy.push(sgy);
        end_gx.push(egx);
        end_gy.push(egy);
        length_m.push(grid::geo::flat_dist(s_lat, s_lon, e_lat, e_lon) as f32);
    }
    let mut columns: Vec<ArrayRef> = Vec::new();
    for field in schema.fields() {
        let array: ArrayRef = match field.name().as_str() {
            "osm_id" => Arc::new(Int64Array::from(
                pieces
                    .iter()
                    .map(|piece| crossings[approaches[piece.approach_idx].crossing].synth_osm)
                    .collect::<Vec<_>>(),
            )),
            "segment_idx" => Arc::new(Int16Array::from(
                pieces
                    .iter()
                    .map(|piece| {
                        let approach = &approaches[piece.approach_idx];
                        (approach.approach_idx as i16) * 1000 + piece.piece_idx
                    })
                    .collect::<Vec<_>>(),
            )),
            "start_gx" => Arc::new(Int32Array::from(start_gx.clone())),
            "start_gy" => Arc::new(Int32Array::from(start_gy.clone())),
            "end_gx" => Arc::new(Int32Array::from(end_gx.clone())),
            "end_gy" => Arc::new(Int32Array::from(end_gy.clone())),
            "length_m" => Arc::new(Float32Array::from(length_m.clone())),
            "rail_type" => Arc::new(UInt8Array::from(vec![HORN_RAIL_TYPE; pieces.len()])),
            "maxspeed" => Arc::new(UInt16Array::from(
                pieces
                    .iter()
                    .map(|piece| {
                        crossings[approaches[piece.approach_idx].crossing]
                            .speed_kmh
                            .map(|v| v.round().clamp(0.0, 65535.0) as u16)
                            .unwrap_or(0)
                    })
                    .collect::<Vec<_>>(),
            )),
            "name" => Arc::new(StringArray::from(
                pieces
                    .iter()
                    .map(|piece| {
                        format!(
                            "Level-crossing horn {}",
                            crossings[approaches[piece.approach_idx].crossing].id
                        )
                    })
                    .collect::<Vec<_>>(),
            )),
            "ref" => Arc::new(StringArray::from(vec![""; pieces.len()])),
            // Horn rows are at grade by construction: never inherit tunnel.
            "tunnel" => Arc::new(BooleanArray::from(vec![false; pieces.len()])),
            "service" => Arc::new(UInt8Array::from(vec![0u8; pieces.len()])),
            "traffic_mode" => Arc::new(UInt8Array::from(vec![0u8; pieces.len()])),
            "trains_passenger_day" => Arc::new(Float64Array::from(
                pieces.iter().map(|p| p.soundings[0]).collect::<Vec<_>>(),
            )),
            "trains_passenger_evening" => Arc::new(Float64Array::from(
                pieces.iter().map(|p| p.soundings[1]).collect::<Vec<_>>(),
            )),
            "trains_passenger_night" => Arc::new(Float64Array::from(
                pieces.iter().map(|p| p.soundings[2]).collect::<Vec<_>>(),
            )),
            "trains_freight_day" | "trains_freight_evening" | "trains_freight_night" => {
                Arc::new(Float64Array::from(vec![0.0; pieces.len()]))
            }
            "passenger_status" => Arc::new(UInt8Array::from(vec![2u8; pieces.len()])),
            // No freight term by construction: estimated zero, unattributed.
            "freight_status" => Arc::new(UInt8Array::from(vec![2u8; pieces.len()])),
            "passenger_source_id" => Arc::new(UInt16Array::from(
                pieces
                    .iter()
                    .map(|piece| crossings[approaches[piece.approach_idx].crossing].source_id)
                    .collect::<Vec<_>>(),
            )),
            "freight_source_id" => Arc::new(UInt16Array::from(vec![0u16; pieces.len()])),
            "passenger_matching" | "freight_matching" => {
                Arc::new(UInt8Array::from(vec![0u8; pieces.len()]))
            }
            // Tags + baked identity follow the matched track, as does the
            // retained OSM evidence: the approach runs along the track, and
            // the way extent describes the parent way, not the piece.
            "usage" | "electrified" | "gauge" | "bridge" | "highspeed" | "country_iso"
            | "city_id" | "continent" | "osm_tags" | "way_start_node" | "way_end_node"
            | "way_start_gx" | "way_start_gy" | "way_end_gx" | "way_end_gy" => {
                take_col(field.name())?
            }
            other => return Err(format!("horn rows cannot fill finalized column {other}")),
        };
        if array.data_type() != field.data_type() {
            return Err(format!(
                "horn column {} type {:?}, file wants {:?}",
                field.name(),
                array.data_type(),
                field.data_type()
            ));
        }
        columns.push(array);
    }
    RecordBatch::try_new(schema.clone(), columns).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kearney 816990Y (37 day / 36 night thru trains): the uniform-block
    /// split both the pilot and this mapping agree on.
    #[test]
    fn fra_day_night_maps_to_end_periods() {
        let full = fra_end_periods(37.0, 36.0);
        assert!((full[0] - 36.9167).abs() < 1e-3, "{full:?}");
        assert!((full[1] - 12.0).abs() < 1e-9, "{full:?}");
        assert!((full[2] - 24.0833).abs() < 1e-3, "{full:?}");
        assert!((full.iter().sum::<f64>() - 73.0).abs() < 1e-9);
        // Partial quiet zone (silent 22–07): night goes quiet, evening keeps
        // the 19–22 night-hours, day is unchanged.
        let partial = fra_partial_periods(37.0, 36.0);
        assert!((partial[0] - full[0]).abs() < 1e-9, "{partial:?}");
        assert!((partial[1] - 9.0).abs() < 1e-9, "{partial:?}");
        assert_eq!(partial[2], 0.0);
    }

    fn approach(start_lon: f64, end_lon: f64, lat: f64, soundings: [f64; 3]) -> Approach {
        let m_lon = grid::geo::m_per_deg_lon(lat.to_radians());
        let dx = (end_lon - start_lon) * m_lon;
        let len = dx.abs().max(1e-6);
        Approach {
            crossing: 0,
            approach_idx: 0,
            matched_row: 0,
            start_lon,
            start_lat: lat,
            end_lon,
            end_lat: lat,
            dir_x: dx / len,
            dir_y: 0.0,
            length_m: len,
            soundings,
        }
    }

    /// Two same-direction 402 m approaches 200 m apart share 202 m: the
    /// shared window is emitted once, on the louder approach at the max
    /// soundings, and the tails keep their own. Emitting it twice would
    /// double the energy on shared ground (+3.01 dB).
    #[test]
    fn overlapping_same_direction_approaches_share_max() {
        let lat: f64 = 40.69;
        let m_lon = grid::geo::m_per_deg_lon(lat.to_radians());
        // Crossings 200 m apart on an eastbound line; approaches end at each.
        let west = approach(-99.15 - 402.0 / m_lon, -99.15, lat, [20.0, 6.0, 12.0]);
        let east = approach(
            -99.15 + (200.0 - 402.0) / m_lon,
            -99.15 + 200.0 / m_lon,
            lat,
            [30.0, 10.0, 18.0],
        );
        let pieces = merge_overlaps(&[west, east]);
        // West tail + shared once (on east) + east tail.
        assert_eq!(pieces.len(), 3, "{pieces:?}");
        let west_pieces: Vec<&MergedPiece> =
            pieces.iter().filter(|p| p.approach_idx == 0).collect();
        let east_pieces: Vec<&MergedPiece> =
            pieces.iter().filter(|p| p.approach_idx == 1).collect();
        assert_eq!(west_pieces.len(), 1);
        assert_eq!(east_pieces.len(), 2);
        // The shared window sits on east (the louder approach) at the max;
        // west keeps only its tail.
        assert_eq!(west_pieces[0].soundings, [20.0, 6.0, 12.0]);
        assert!(west_pieces[0].frac1 < 1.0);
        assert_eq!(east_pieces[0].soundings, [30.0, 10.0, 18.0]);
        assert_eq!(east_pieces[0].frac0, 0.0);
        assert_eq!(east_pieces[1].soundings, [30.0, 10.0, 18.0]);
    }

    /// Horn rows forward the carried track columns (`osm_tags`, `way_*`)
    /// from the matched track row; without the forwarding arm the build
    /// fails on the finalized schema ("horn rows cannot fill finalized
    /// column way_start_node").
    #[test]
    fn horn_rows_forward_carried_track_columns() {
        use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
        let schema: SchemaRef = Arc::new(Schema::new(vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("osm_tags", DataType::Utf8, false),
            Field::new("way_start_node", DataType::Int64, true),
            Field::new("way_end_gx", DataType::Int32, true),
            Field::new("trains_passenger_day", DataType::Float64, false),
        ]));
        let base = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![123])) as ArrayRef,
                Arc::new(StringArray::from(vec!["railway=rail"])) as ArrayRef,
                Arc::new(Int64Array::from(vec![Some(7)])) as ArrayRef,
                Arc::new(Int32Array::from(vec![Some(9)])) as ArrayRef,
                Arc::new(Float64Array::from(vec![80.0])) as ArrayRef,
            ],
        )
        .unwrap();
        let crossings = vec![Crossing {
            id: "816990Y".to_string(),
            lat: 40.69,
            lon: -99.142,
            soundings: [36.0, 12.0, 24.0],
            speed_kmh: None,
            source_id: SOURCE_ID_FRA,
            synth_osm: -1,
        }];
        let approaches = vec![approach(-99.15, -99.142, 40.69, [18.0, 6.0, 12.0])];
        let pieces = vec![MergedPiece {
            approach_idx: 0,
            piece_idx: 0,
            frac0: 0.0,
            frac1: 1.0,
            soundings: [18.0, 6.0, 12.0],
        }];
        let batch = build_horn_batch(&schema, &base, &crossings, &approaches, &pieces).unwrap();
        assert_eq!(batch.num_rows(), 1);
        let tags = batch
            .column_by_name("osm_tags")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(tags.value(0), "railway=rail");
        let start = batch
            .column_by_name("way_start_node")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(start.value(0), 7);
        let end_gx = batch
            .column_by_name("way_end_gx")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(end_gx.value(0), 9);
    }

    /// Opposite-direction approaches never merge (different trains sound).
    #[test]
    fn opposite_direction_approaches_do_not_merge() {
        let lat: f64 = 40.69;
        let m_lon = grid::geo::m_per_deg_lon(lat.to_radians());
        let eastbound = approach(-99.15 - 402.0 / m_lon, -99.15, lat, [20.0, 6.0, 12.0]);
        let westbound = approach(-99.15, -99.15 - 402.0 / m_lon, lat, [20.0, 6.0, 12.0]);
        let pieces = merge_overlaps(&[eastbound, westbound]);
        assert_eq!(pieces.len(), 2);
        assert!((pieces[0].frac0 - 0.0).abs() < 1e-9);
        assert!((pieces[0].frac1 - 1.0).abs() < 1e-9);
        assert!((pieces[1].frac0 - 0.0).abs() < 1e-9);
        assert!((pieces[1].frac1 - 1.0).abs() < 1e-9);
    }
}
