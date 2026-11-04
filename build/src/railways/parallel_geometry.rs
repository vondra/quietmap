//! Identify distinct physical tracks running abreast in one corridor.

use super::encode::Expanded;
use std::collections::{HashMap, HashSet};

// Sibling gates moved from the retired CZ graph spread (pipeline rail-graph-metrics, reviews of
// 2026-07-15/16): track pairs of one corridor sit 4-10 m apart. Without a shared ref or name a
// tighter radius and heading stand in for identity (CZ: 0 of about 5,000 corridor segments
// carry either); equal tokens allow 50 m and 20 degrees; different tokens are never siblings.
const TOKENLESS_LATERAL_M: f64 = 15.0;
// A neighbour counts only when the shared run is at least this long, or this fraction of
// the track's own length, whichever is greater (2026-07-16 review of the retired spread).
const MIN_OVERLAP_M: f64 = 30.0;
const MIN_OVERLAP_FRACTION: f64 = 0.3;
const TOKENLESS_HEADING_DEG: f64 = 10.0;
const SAME_TOKEN_LATERAL_M: f64 = 50.0;
const SAME_TOKEN_HEADING_DEG: f64 = 20.0;

pub(super) struct Track {
    start: [f64; 2],
    end: [f64; 2],
    direction: [f64; 2],
}

type Candidate = (f64, (i32, i32, i32, i32), usize);

fn usage_family(usage: u8) -> u8 {
    // A main-tagged track and its untagged twin (usage 0 and 3) are one corridor.
    if usage == 3 { 0 } else { usage }
}

pub(super) fn project_tracks(
    rows: &[Expanded],
    square: crate::osm::geometry::Square,
) -> Vec<Option<Track>> {
    let northing =
        (0.5 - (f64::from(square.y) + 0.5) / 512.0) * crate::osm::geometry::EARTH_CIRCUMFERENCE_M;
    let latitude = (2.0
        * (northing / crate::osm::geometry::WEB_MERCATOR_RADIUS_M)
            .exp()
            .atan()
        - std::f64::consts::FRAC_PI_2)
        .to_degrees();
    let scale = latitude.to_radians().cos();
    let metres = |gx: i32, gy: i32| {
        let (x, y) = crate::osm::geometry::grid_to_meters(gx, gy);
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

fn abreast_distance_m(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> Option<f64> {
    let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
    let t = ((point[0] - start[0]) * dx + (point[1] - start[1]) * dy) / (dx * dx + dy * dy);
    (0.0..=1.0)
        .contains(&t)
        .then(|| (point[0] - start[0] - t * dx).hypot(point[1] - start[1] - t * dy))
}

fn sibling_limit(a: &Expanded, ta: &Track, b: &Expanded, tb: &Track) -> Option<f64> {
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
    (cross.abs().atan2(dot.abs()).to_degrees() < max_heading_deg).then_some(max_lateral_m)
}

fn sibling_lateral_m(a: &Expanded, ta: &Track, b: &Expanded, tb: &Track) -> Option<f64> {
    let max_lateral_m = sibling_limit(a, ta, b, tb)?;
    let middle = [
        (ta.start[0] + ta.end[0]) / 2.0,
        (ta.start[1] + ta.end[1]) / 2.0,
    ];
    abreast_distance_m(middle, tb.start, tb.end).filter(|lateral_m| *lateral_m < max_lateral_m)
}

fn way_overlap_m(
    a: &Expanded,
    ta: &Track,
    way: &[usize],
    rows: &[Expanded],
    tracks: &[Option<Track>],
    lateral_gate: bool,
) -> f64 {
    let scalar = |p: [f64; 2]| p[0] * ta.direction[0] + p[1] * ta.direction[1];
    let (a0, a1) = (scalar(ta.start), scalar(ta.end));
    let (a_min, a_max) = (a0.min(a1), a0.max(a1));
    let mut spans: Vec<(f64, f64)> = way
        .iter()
        .filter_map(|&index| {
            let tb = tracks[index].as_ref()?;
            let limit = sibling_limit(a, ta, &rows[index], tb)?;
            if lateral_gate {
                let (dx, dy) = (tb.start[0] - ta.start[0], tb.start[1] - ta.start[1]);
                let separation = (dx * ta.direction[1] - dy * ta.direction[0]).abs();
                if separation >= limit {
                    return None;
                }
            }
            let (s, e) = (scalar(tb.start), scalar(tb.end));
            Some((s.min(e).max(a_min), s.max(e).min(a_max)))
        })
        .filter(|(start, end)| end > start)
        .collect();
    spans.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut total = 0.0;
    let mut open: Option<(f64, f64)> = None;
    for (start, end) in spans {
        match open {
            Some((from, to)) if start <= to => open = Some((from, to.max(end))),
            Some((from, to)) => {
                total += to - from;
                open = Some((start, end));
            }
            None => open = Some((start, end)),
        }
    }
    total + open.map_or(0.0, |(from, to)| to - from)
}

pub(super) fn cross_sections(rows: &[Expanded], tracks: &[Option<Track>]) -> Vec<Vec<usize>> {
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
    let mut rows_of_way: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        if track.is_some() {
            rows_of_way
                .entry(rows[index].osm_id)
                .or_default()
                .push(index);
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
            let span_m = (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
            let min_overlap = MIN_OVERLAP_M.max(MIN_OVERLAP_FRACTION * span_m);
            siblings.retain(|&other| {
                way_overlap_m(
                    &rows[index],
                    track,
                    &rows_of_way[&rows[other].osm_id],
                    rows,
                    tracks,
                    true,
                ) >= min_overlap
            });
            siblings.sort_by_key(|&other| rows[other].osm_id);
            members.extend(siblings);
        }
        sections.push(members);
    }
    // Transitive closure over ways: for each row, every way reachable
    // through members' direct sections joins with the representative its
    // discoverer already holds (nearest beside a member midpoint), provided
    // it passes every non-lateral sibling gate against the section owner:
    // corridor identity, heading, shared nodes (sibling_limit) and the
    // owner's overlap bar. Discovery order is fixed (self first, then
    // members in section order), so closures ignore input row order like
    // the direct sections do.
    sections
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let mut seen: HashSet<i64> = HashSet::from([rows[index].osm_id]);
            let mut grown = vec![index];
            let mut queue = vec![index];
            let own = tracks[index].as_ref();
            let min_overlap = own.map(|track| {
                let span_m = (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
                MIN_OVERLAP_M.max(MIN_OVERLAP_FRACTION * span_m)
            });
            while let Some(member) = queue.pop() {
                for &other in &sections[member] {
                    if !seen.insert(rows[other].osm_id) {
                        continue;
                    }
                    let (Some(own), Some(candidate), Some(bar)) =
                        (own, tracks[other].as_ref(), min_overlap)
                    else {
                        continue;
                    };
                    if sibling_limit(&rows[index], own, &rows[other], candidate).is_none() {
                        continue;
                    }
                    if way_overlap_m(
                        &rows[index],
                        own,
                        &rows_of_way[&rows[other].osm_id],
                        rows,
                        tracks,
                        false,
                    ) < bar
                    {
                        continue;
                    }
                    queue.push(other);
                    grown.push(other);
                }
            }
            grown[1..].sort_by_key(|&other| rows[other].osm_id);
            grown
        })
        .collect()
}
