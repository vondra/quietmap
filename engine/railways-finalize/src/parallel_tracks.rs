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

/// Best sibling candidate of one way: lateral metres, stable grid-geometry tiebreak, row.
type Candidate = (f64, (i32, i32, i32, i32), usize);

fn usage_family(usage: u8) -> u8 {
    // A main-tagged track and its untagged twin (usage 0 and 3) are one corridor.
    if usage == 3 {
        0
    } else {
        usage
    }
}

/// Local metres: Web Mercator scaled by the square centre's latitude (under 1 % error
/// across a z9 square). The scale comes from the square, never from a row, so reversing the
/// input cannot flip near-gate sibling pairs.
fn project_tracks(rows: &[Expanded], square: grid::Square) -> Vec<Option<Track>> {
    let scale = grid::square_center_lat_deg(square).to_radians().cos();
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
            let middle = [
                (track.start[0] + track.end[0]) / 2.0,
                (track.start[1] + track.end[1]) / 2.0,
            ];
            // One representative per way; ties break on grid geometry, never on input position,
            // so the sections (and everything downstream) ignore input row order.
            let mut nearest: HashMap<i64, Candidate> = HashMap::new();
            for x in cell(middle[0]) - 1..=cell(middle[0]) + 1 {
                for y in cell(middle[1]) - 1..=cell(middle[1]) + 1 {
                    for &other in buckets.get(&(x, y)).map(Vec::as_slice).unwrap_or_default() {
                        let other_track = tracks[other].as_ref().unwrap();
                        let Some(lateral_m) =
                            sibling_lateral_m(&rows[index], track, &rows[other], other_track)
                        else {
                            continue;
                        };
                        let geom = rows[other].child.geom;
                        let key = (geom.start_gx, geom.start_gy, geom.end_gx, geom.end_gy);
                        let best = nearest
                            .entry(rows[other].osm_id)
                            .or_insert((lateral_m, key, other));
                        if (lateral_m, key) < (best.0, best.1) {
                            *best = (lateral_m, key, other);
                        }
                    }
                }
            }
            let mut siblings: Vec<_> = nearest.into_values().map(|(_, _, other)| other).collect();
            siblings.sort_by_key(|&other| rows[other].osm_id);
            members.extend(siblings);
        }
        sections.push(members);
    }
    sections
}

/// Highest-ranked claim of one scope (domestic or foreign) over a cross-section.
/// A residual yields to ranked evidence elsewhere on the line: a no-service stamp where the
/// walk routed trains is walk artifact, not silence.
fn scope_winner(
    own_iso: [u8; 2],
    members: &[usize],
    flows: &[RowTraffic],
    line_ranked: bool,
    category: fn(&RowTraffic) -> CategoryFlow,
) -> u16 {
    let mut winner = 0u16;
    for &member in members {
        let flow = category(&flows[member]);
        if flow.status == STATUS_UNKNOWN
            || !source_applies_to_row(flow.source_id, own_iso)
            || (line_ranked && is_residual(flow.source_id))
        {
            continue;
        }
        if winner != 0 && blocks_foreign_national(winner, flow.source_id) {
            continue;
        }
        if should_overwrite(winner, flow.source_id) {
            winner = flow.source_id;
        }
    }
    winner
}

/// Whole-line value of one scope's winner: a measured source counts the trains of each track
/// (a routed trip sits on the track it used, a platform stop counts its own direction), so the
/// line carries their sum; a proxy or residual repeats the line's value on every track it stamps.
fn scope_value(
    members: &[usize],
    flows: &[RowTraffic],
    winner: u16,
    category: fn(&RowTraffic) -> CategoryFlow,
) -> CategoryFlow {
    let carriers: Vec<CategoryFlow> = members
        .iter()
        .map(|&member| category(&flows[member]))
        .filter(|flow| flow.status != STATUS_UNKNOWN && flow.source_id == winner)
        .collect();
    let divisor = if stamps_whole_line(winner) {
        carriers.len() as f64
    } else {
        1.0
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
        source_id: winner,
        matching: carriers.iter().fold(0, |mask, flow| mask | flow.matching),
    }
}

/// The line's class prior, once (the largest of its tracks' classes).
fn line_prior(
    members: &[usize],
    priors: &[RowTraffic],
    category: fn(&RowTraffic) -> CategoryFlow,
) -> CategoryFlow {
    members
        .iter()
        .map(|&member| category(&priors[member]))
        .fold(CategoryFlow::default(), |best, flow| {
            let total = |f: &CategoryFlow| f.periods.iter().sum::<f64>();
            if best.status == STATUS_UNKNOWN || total(&flow) > total(&best) {
                flow
            } else {
                best
            }
        })
}

fn daily_total(flow: &CategoryFlow) -> f64 {
    flow.periods.iter().sum()
}

fn carries_passengers(mode: u8) -> bool {
    mode != 2
}

fn carries_freight(mode: u8) -> bool {
    mode != 1
}

/// The line value of one category over a cross-section, divided among its tracks.
///
/// A timetable aims at full domestic coverage, so domestic evidence is trusted where the walk
/// covered every track (a ranked claim or a residual no-service stamp); where it covered only
/// some tracks the walked sum is a lower bound and the class prior stands for the line.
/// Neighbour timetables only ever see cross-border services: they bound a line the domestic
/// timetable missed and lose to domestic evidence, but ranked neighbour trains still mark the
/// line active, yielding a domestic no-service stamp beside them.
/// The class prior belongs only to the tracks whose OSM traffic mode carries the category:
/// a passenger-only track takes no freight prior (and symmetrically), so the line prior
/// divides among the capable tracks. Measured evidence still wins over the tag and keeps
/// the whole cross-section as its divisor.
fn track_share(
    own_iso: [u8; 2],
    members: &[usize],
    domestic: &[RowTraffic],
    foreign: &[RowTraffic],
    priors: &[RowTraffic],
    category: fn(&RowTraffic) -> CategoryFlow,
    modes: &[u8],
    own: usize,
    allows: fn(u8) -> bool,
) -> CategoryFlow {
    let tracks = members.len() as f64;
    // Ranked evidence from either timetable marks the line active; a no-service stamp beside
    // it is a walk gap on that piece (Revnice segment 2), not silence.
    let line_ranked = members.iter().any(|&member| {
        [
            domestic[member].passenger,
            domestic[member].freight,
            foreign[member].passenger,
            foreign[member].freight,
        ]
        .iter()
        .any(|flow| flow.status != STATUS_UNKNOWN && !is_residual(flow.source_id))
    });
    let capable: Vec<usize> = members
        .iter()
        .copied()
        .filter(|&member| allows(modes[member]))
        .collect();
    let prior = line_prior(&capable, priors, category);
    let winner = scope_winner(own_iso, members, domestic, line_ranked, category);
    let (line, divisor, from_prior) = if winner != 0 && !is_residual(winner) {
        let value = scope_value(members, domestic, winner, category);
        if stamps_whole_line(winner) {
            // A proxy estimates the line itself, not the tracks it stamps.
            (value, tracks, false)
        } else {
            let fully_covered = members
                .iter()
                .all(|&member| category(&domestic[member]).status != STATUS_UNKNOWN);
            if fully_covered || daily_total(&value) > daily_total(&prior) {
                (value, tracks, false)
            } else {
                (prior, capable.len() as f64, true)
            }
        }
    } else if is_residual(winner) {
        (
            scope_value(members, domestic, winner, category),
            tracks,
            false,
        )
    } else {
        let abroad = scope_winner(own_iso, members, foreign, line_ranked, category);
        let value = if abroad == 0 {
            CategoryFlow::default()
        } else {
            scope_value(members, foreign, abroad, category)
        };
        if daily_total(&value) > daily_total(&prior) {
            (value, tracks, false)
        } else {
            (prior, capable.len() as f64, true)
        }
    };
    if from_prior && !allows(modes[own]) {
        return CategoryFlow {
            periods: [0.0; 3],
            status: STATUS_ESTIMATED,
            source_id: 0,
            matching: 0,
        };
    }
    CategoryFlow {
        periods: line.periods.map(|value| value / divisor),
        ..line
    }
}

/// Replace each non-service row's evidence by its share of the line; service tracks keep theirs.
/// The square sets the projection scale, so the result ignores input row order.
pub(crate) fn allocate_over_parallel_tracks(rows: &mut [Expanded], square: grid::Square) {
    let tracks = project_tracks(rows, square);
    let sections = cross_sections(rows, &tracks);
    let domestic: Vec<RowTraffic> = rows.iter().map(|row| row.child.traffic).collect();
    let foreign: Vec<RowTraffic> = rows.iter().map(|row| row.child.foreign).collect();
    let priors: Vec<RowTraffic> = rows.iter().map(|row| row.prior).collect();
    let modes: Vec<u8> = rows.iter().map(|row| row.traffic_mode).collect();
    for (index, row) in rows.iter_mut().enumerate() {
        if row.service != 0 {
            continue;
        }
        let members = &sections[index];
        row.child.traffic = RowTraffic {
            passenger: track_share(
                row.country_iso,
                members,
                &domestic,
                &foreign,
                &priors,
                |t| t.passenger,
                &modes,
                index,
                carries_passengers,
            ),
            freight: track_share(
                row.country_iso,
                members,
                &domestic,
                &foreign,
                &priors,
                |t| t.freight,
                &modes,
                index,
                carries_freight,
            ),
        };
    }
}

#[cfg(test)]
#[path = "parallel_tracks_tests.rs"]
mod tests;
