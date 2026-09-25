//! One traffic allocation over parallel tracks: each category is counted once per line
//! cross-section, from evidence on any of its tracks or else one class prior, and split evenly.

use crate::encode::Expanded;
use crate::merge::{CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_KNOWN, STATUS_UNKNOWN};
use crate::sources::{
    blocks_foreign_national, is_residual, should_overwrite, source_applies_to_row, stamps_whole_line,
};
use std::collections::HashMap;

// Sibling gates moved from the retired CZ graph spread (pipeline rail-graph-metrics, reviews of
// 2026-07-15/16): track pairs of one corridor sit 4-10 m apart. Without a shared ref or name a
// tighter radius and heading stand in for identity (CZ: 0 of about 5,000 corridor segments
// carry either); equal tokens allow 50 m and 20 degrees; different tokens are never siblings.
const TOKENLESS_LATERAL_M: f64 = 15.0;
const TOKENLESS_HEADING_DEG: f64 = 10.0;
const SAME_TOKEN_LATERAL_M: f64 = 50.0;
const SAME_TOKEN_HEADING_DEG: f64 = 20.0;

struct Track {
    start: [f64; 2],
    end: [f64; 2],
    direction: [f64; 2],
}

fn usage_family(usage: u8) -> u8 {
    // A main-tagged track and its untagged twin (usage 0 and 3) are one corridor.
    if usage == 3 {
        0
    } else {
        usage
    }
}

/// Local metres: Web Mercator scaled by the square's latitude (under 1 % error across a z9 square).
fn project_tracks(rows: &[Expanded]) -> Vec<Option<Track>> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let (_, latitude) = crate::encode::grid_cell_lonlat(first.child.geom.start_gx, first.child.geom.start_gy);
    let scale = latitude.to_radians().cos();
    let metres = |gx: i32, gy: i32| {
        let (x, y) = grid::grid_to_meters(gx, gy);
        [x * scale, y * scale]
    };
    rows.iter()
        .map(|row| {
            let g = row.child.geom;
            let start = metres(g.start_gx, g.start_gy);
            let end = metres(g.end_gx, g.end_gy);
            let span_m = (end[0] - start[0]).hypot(end[1] - start[1]);
            (row.service == 0 && span_m > 1e-6).then(|| Track {
                start,
                end,
                direction: [(end[0] - start[0]) / span_m, (end[1] - start[1]) / span_m],
            })
        })
        .collect()
}

/// Distance to the foot of the perpendicular when it lands on the segment's body, else None:
/// a neighbour must run beside the track's midpoint, not end before it (stagger- and cut-immune).
fn abreast_distance_m(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> Option<f64> {
    let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
    let t = ((point[0] - start[0]) * dx + (point[1] - start[1]) * dy) / (dx * dx + dy * dy);
    (0.0..=1.0)
        .contains(&t)
        .then(|| (point[0] - start[0] - t * dx).hypot(point[1] - start[1] - t * dy))
}

/// Lateral distance from `a`'s midpoint to `b`'s body when `b` runs beside `a`, else None.
fn sibling_lateral_m(a: &Expanded, ta: &Track, b: &Expanded, tb: &Track) -> Option<f64> {
    if a.osm_id == b.osm_id
        || a.rail_type != b.rail_type
        || usage_family(a.usage) != usage_family(b.usage)
    {
        return None;
    }
    let (ga, gb) = (a.child.geom, b.child.geom);
    let a_ends = [(ga.start_gx, ga.start_gy), (ga.end_gx, ga.end_gy)];
    if a_ends.contains(&(gb.start_gx, gb.start_gy)) || a_ends.contains(&(gb.end_gx, gb.end_gy)) {
        return None; // a spur meeting the track is not a sibling
    }
    let same_token = !a.corridor.is_empty() && a.corridor == b.corridor;
    if !same_token && !a.corridor.is_empty() && !b.corridor.is_empty() {
        return None;
    }
    let (max_lateral_m, max_heading_deg) = if same_token {
        (SAME_TOKEN_LATERAL_M, SAME_TOKEN_HEADING_DEG)
    } else {
        (TOKENLESS_LATERAL_M, TOKENLESS_HEADING_DEG)
    };
    let cross = ta.direction[0] * tb.direction[1] - ta.direction[1] * tb.direction[0];
    let dot = ta.direction[0] * tb.direction[0] + ta.direction[1] * tb.direction[1];
    if cross.abs().atan2(dot.abs()).to_degrees() >= max_heading_deg {
        return None;
    }
    let middle = [(ta.start[0] + ta.end[0]) / 2.0, (ta.start[1] + ta.end[1]) / 2.0];
    abreast_distance_m(middle, tb.start, tb.end).filter(|lateral_m| *lateral_m < max_lateral_m)
}

/// For every track: itself first, then the laterally nearest row of each other way beside it.
fn cross_sections(rows: &[Expanded], tracks: &[Option<Track>]) -> Vec<Vec<usize>> {
    let cell = |value: f64| (value / SAME_TOKEN_LATERAL_M).floor() as i64;
    let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let Some(track) = track else { continue };
        for x in cell(track.start[0].min(track.end[0]))..=cell(track.start[0].max(track.end[0])) {
            for y in cell(track.start[1].min(track.end[1]))..=cell(track.start[1].max(track.end[1]))
            {
                buckets.entry((x, y)).or_default().push(index);
            }
        }
    }
    let mut sections = Vec::with_capacity(rows.len());
    for (index, track) in tracks.iter().enumerate() {
        let mut members = vec![index];
        if let Some(track) = track {
            let middle = [(track.start[0] + track.end[0]) / 2.0, (track.start[1] + track.end[1]) / 2.0];
            let mut nearest: HashMap<i64, (f64, usize)> = HashMap::new();
            for x in cell(middle[0]) - 1..=cell(middle[0]) + 1 {
                for y in cell(middle[1]) - 1..=cell(middle[1]) + 1 {
                    for &other in buckets.get(&(x, y)).map(Vec::as_slice).unwrap_or_default() {
                        let other_track = tracks[other].as_ref().unwrap();
                        let Some(lateral_m) =
                            sibling_lateral_m(&rows[index], track, &rows[other], other_track)
                        else {
                            continue;
                        };
                        let best = nearest.entry(rows[other].osm_id).or_insert((lateral_m, other));
                        if (lateral_m, other) < *best {
                            *best = (lateral_m, other);
                        }
                    }
                }
            }
            let mut siblings: Vec<_> = nearest.into_values().map(|(_, other)| other).collect();
            siblings.sort_unstable();
            members.extend(siblings);
        }
        sections.push(members);
    }
    sections
}

/// The line value of one category over a cross-section, divided among its tracks.
fn track_share(
    own_iso: [u8; 2],
    members: &[usize],
    evidence: &[RowTraffic],
    priors: &[RowTraffic],
    category: fn(&RowTraffic) -> CategoryFlow,
) -> CategoryFlow {
    let tracks = members.len() as f64;
    let line_is_evidenced = members.iter().any(|&member| {
        [evidence[member].passenger, evidence[member].freight]
            .iter()
            .any(|flow| flow.status != STATUS_UNKNOWN && !is_residual(flow.source_id))
    });
    let mut line_source = 0u16;
    for &member in members {
        let flow = category(&evidence[member]);
        if flow.status == STATUS_UNKNOWN
            || !source_applies_to_row(flow.source_id, own_iso)
            || (line_is_evidenced && is_residual(flow.source_id))
        {
            continue;
        }
        if line_source != 0 && blocks_foreign_national(line_source, flow.source_id) {
            continue;
        }
        if should_overwrite(line_source, flow.source_id) {
            line_source = flow.source_id;
        }
    }
    if line_source == 0 {
        // No track carries evidence: the line's class prior, once (the largest of its tracks' classes).
        let prior = members
            .iter()
            .map(|&member| category(&priors[member]))
            .fold(CategoryFlow::default(), |best, flow| {
                let total = |f: &CategoryFlow| f.periods.iter().sum::<f64>();
                if best.status == STATUS_UNKNOWN || total(&flow) > total(&best) {
                    flow
                } else {
                    best
                }
            });
        return CategoryFlow {
            periods: prior.periods.map(|value| value / tracks),
            ..prior
        };
    }
    let carriers: Vec<CategoryFlow> = members
        .iter()
        .map(|&member| category(&evidence[member]))
        .filter(|flow| flow.status != STATUS_UNKNOWN && flow.source_id == line_source)
        .collect();
    // A measured source counts the trains of each track (a routed trip sits on the track it used,
    // a platform stop counts its own direction), so the line carries their sum; a proxy or residual
    // repeats the line's value on every track it stamps.
    let divisor = if stamps_whole_line(line_source) {
        tracks * carriers.len() as f64
    } else {
        tracks
    };
    let mut periods = [0.0; 3];
    for flow in &carriers {
        for (total, value) in periods.iter_mut().zip(flow.periods) {
            *total += value / divisor;
        }
    }
    CategoryFlow {
        periods,
        status: if carriers.iter().any(|flow| flow.status == STATUS_KNOWN) {
            STATUS_KNOWN
        } else {
            STATUS_ESTIMATED
        },
        source_id: line_source,
        matching: carriers.iter().fold(0, |mask, flow| mask | flow.matching),
    }
}

/// Replace each non-service row's evidence by its share of the line; service tracks keep theirs.
pub(crate) fn allocate_over_parallel_tracks(rows: &mut [Expanded]) {
    let tracks = project_tracks(rows);
    let sections = cross_sections(rows, &tracks);
    let evidence: Vec<RowTraffic> = rows.iter().map(|row| row.child.traffic).collect();
    let priors: Vec<RowTraffic> = rows.iter().map(|row| row.prior).collect();
    for (index, row) in rows.iter_mut().enumerate() {
        if row.service != 0 {
            continue;
        }
        let members = &sections[index];
        row.child.traffic = RowTraffic {
            passenger: track_share(row.country_iso, members, &evidence, &priors, |t| t.passenger),
            freight: track_share(row.country_iso, members, &evidence, &priors, |t| t.freight),
        };
    }
}

#[cfg(test)]
#[path = "parallel_tracks_tests.rs"]
mod tests;
