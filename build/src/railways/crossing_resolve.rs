//! Resolve each canonical-reference crossing section once per timetable window and fan out claims.
use super::{
    corridor::{CorridorLayout, Section, SectionLeg},
    corridor_runs::{Run, TokenRel, to_local_m, tokens_match},
    matcher::FailedServiceLeg,
    merge::{CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_UNKNOWN, row_evidence},
    model::RailModel,
    network::{Network, project},
    parallel_tracks::{
        carries_freight, carries_passengers, scope_value, scope_winner, subtract_passenger_periods,
    },
    square_intervals::Interval,
};
use crate::osm::geometry::wrapped_longitude_delta;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Minimum contiguous timetable claim length on each side of a gap to treat it as an active
/// service bracket rather than a sub-metre station-snap bleed from a parallel corridor.
const MIN_TIMETABLE_BRACKET_M: f64 = 5.0;
/// Maximum switch-free interstation gap eligible for failed-leg recovery; well under one z9 square
/// width (~51 km at 49 deg N) so both ends always lie inside the 1-ring claim halo.
const MAX_TIMETABLE_GAP_M: f64 = 20_000.0;
/// Maximum perpendicular distance between every sample along a candidate gap and a failed
/// service leg's archived shape; rejects parallel freight bypasses (hundreds to thousands of m).
const MAX_GAP_SHAPE_DIST_M: f64 = 50.0;
/// Maximum longitudinal distance a gap sample may extend past the start or end tip of a failed
/// service leg's archived shape; stops a terminating service from filling past its endpoint.
const MAX_GAP_TIP_OVERHANG_M: f64 = 10.0;
/// Sampling interval along a candidate gap when checking alignment against a failed service leg.
const GAP_SAMPLE_STEP_M: f64 = 50.0;

fn project_leg_s(
    network: &Network,
    runs: &[Run],
    from: &SectionLeg,
    s: f64,
    to: &SectionLeg,
) -> f64 {
    let frac = ((s - from.s0) / (from.s1 - from.s0).max(1e-9)).clamp(0.0, 1.0);
    let f = if from.same_dir == to.same_dir {
        frac
    } else {
        1.0 - frac
    };
    let hint = to.s0 + f * (to.s1 - to.s0);
    let pt = runs[from.run_id].point_at(network, s);
    runs[to.run_id]
        .project_onto_range(network, pt, to.s0, to.s1, Some(hint))
        .1
        .clamp(to.s0, to.s1)
}

fn project_leg_s_pinned(
    network: &Network,
    runs: &[Run],
    from: &SectionLeg,
    s: f64,
    to: &SectionLeg,
) -> f64 {
    if (s - from.s0).abs() <= 1e-4 {
        if from.same_dir == to.same_dir {
            to.s0
        } else {
            to.s1
        }
    } else if (s - from.s1).abs() <= 1e-4 {
        if from.same_dir == to.same_dir {
            to.s1
        } else {
            to.s0
        }
    } else {
        project_leg_s(network, runs, from, s, to)
    }
}

fn project_onto_polyline(
    pt: [f64; 2],
    shape: &[[f64; 2]],
    cum_m: &[f64],
) -> (f64, f64, [f64; 2], Option<[f64; 2]>) {
    let mut best = (f64::INFINITY, 0.0, [0.0, 0.0], None);
    for i in 0..shape.len().saturating_sub(1) {
        let (a, b) = (shape[i], shape[i + 1]);
        let (dist, frac) = project(pt, a, b);
        if dist < best.0 {
            let dlon = wrapped_longitude_delta(a[1], b[1]);
            let shape_pt = [a[0] + frac * (b[0] - a[0]), a[1] + frac * dlon];
            let cos_lat = a[0].to_radians().cos().max(0.05);
            let v = to_local_m(a, cos_lat, b);
            let vlen = v[0].hypot(v[1]);
            let dir = (vlen > 1e-3).then_some([v[0] / vlen, v[1] / vlen]);
            best = (
                dist,
                cum_m[i] + frac * (cum_m[i + 1] - cum_m[i]),
                shape_pt,
                dir,
            );
        }
    }
    best
}

struct GapPair {
    left_s: f64,
    right_s: f64,
    source_id: u16,
    spans: Vec<(f64, f64)>,
    gap_len: f64,
}

struct CandidateGap {
    run_id: usize,
    spans: Vec<(f64, f64)>,
    source_id: u16,
    gap_len: f64,
    sample_s: Vec<f64>,
    samples: Vec<[f64; 2]>,
    bbox: [f64; 4],
}

fn extract_gap_pairs(run: &Run, mut claimed: Vec<(f64, f64, u16)>) -> Vec<GapPair> {
    if claimed.is_empty() {
        return Vec::new();
    }
    claimed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut groups: Vec<(f64, f64, u16)> = Vec::new();
    for (s0, s1, src) in claimed {
        if let Some(last) = groups.last_mut()
            && s0 <= last.1 + 1e-2
            && last.2 == src
        {
            last.1 = last.1.max(s1);
        } else {
            groups.push((s0, s1, src));
        }
    }
    // Merge a closed run's seam-straddling bracket before checking bracket length.
    if run.closed
        && groups.len() >= 2
        && groups[0].0 <= 1e-2
        && (run.total_len - groups.last().unwrap().1) <= 1e-2
        && groups[0].2 == groups.last().unwrap().2
    {
        let first = groups.remove(0);
        let last = groups.last_mut().unwrap();
        last.1 = run.total_len + first.1;
    }
    groups.retain(|g| g.1 - g.0 >= MIN_TIMETABLE_BRACKET_M);
    if groups.is_empty() {
        return Vec::new();
    }
    let mut pairs: Vec<GapPair> = Vec::new();
    for pair in groups.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        let gap_len = right.0 - left.1;
        if left.2 == right.2 && gap_len > 1e-2 && gap_len <= MAX_TIMETABLE_GAP_M {
            pairs.push(GapPair {
                left_s: left.1,
                right_s: right.0,
                source_id: left.2,
                spans: vec![(left.1, right.0)],
                gap_len,
            });
        }
    }
    if run.closed {
        let left = *groups.last().unwrap();
        let right = groups[0];
        let left_end = left.1.rem_euclid(run.total_len);
        let gap_len = if left.1 >= run.total_len {
            right.0 - (left.1 - run.total_len)
        } else {
            (run.total_len - left.1) + right.0
        };
        if left.2 == right.2 && gap_len > 1e-2 && gap_len <= MAX_TIMETABLE_GAP_M {
            let mut spans = Vec::new();
            if left.1 >= run.total_len {
                spans.push((left.1 - run.total_len, right.0));
            } else {
                if run.total_len - left.1 > 1e-4 {
                    spans.push((left.1, run.total_len));
                }
                if right.0 > 1e-4 {
                    spans.push((0.0, right.0));
                }
            }
            if !spans.is_empty() {
                pairs.push(GapPair {
                    left_s: left_end,
                    right_s: right.0,
                    source_id: left.2,
                    spans,
                    gap_len,
                });
            }
        }
    }
    pairs
}

type SectionRefClaimed = Vec<Vec<(f64, f64, u16)>>;

fn run_has_own_claim(run: &Run, own: &[(f64, f64, u16)], lo: f64, hi: f64, src: u16) -> bool {
    let overlaps = |a: f64, b: f64| {
        own.iter()
            .any(|&(c0, c1, s)| s == src && c0.max(a) + 1e-4 < c1.min(b))
    };
    if overlaps(lo, hi) {
        return true;
    }
    if run.closed && run.total_len > 0.0 {
        if hi > run.total_len && overlaps(0.0, hi - run.total_len) {
            return true;
        }
        if lo < 0.0 && overlaps(run.total_len + lo, run.total_len) {
            return true;
        }
    }
    false
}

fn candidate_run_gaps(
    model: &RailModel,
    network: &Network,
    layout: &CorridorLayout,
    piece_claims: &HashMap<([u16; 2], i64, i16), Vec<&Interval>>,
    source_id: u16,
) -> (Vec<CandidateGap>, SectionRefClaimed) {
    let runs = &layout.runs;
    let sections = &layout.sections;
    let valid_tt = |flow: CategoryFlow, country: [u8; 2], foreign: bool| -> Option<u16> {
        let ok = flow.status != STATUS_UNKNOWN
            && flow.source_id != 0
            && model.source_applies_to_row(flow.source_id, country)
            && !model.is_residual(flow.source_id)
            && model.should_overwrite(source_id, flow.source_id)
            && (!foreign || !model.blocks_foreign_national(source_id, flow.source_id))
            && flow.periods.iter().sum::<f64>() > 0.0;
        ok.then_some(flow.source_id)
    };
    let mut own_claimed: Vec<Vec<(f64, f64, u16)>> = Vec::with_capacity(runs.len());
    for run in runs {
        let mut claimed: Vec<(f64, f64, u16)> = Vec::new();
        for (pos, re) in run.edges.iter().enumerate() {
            let edge = &network.edges[re.edge_idx];
            let Some(ivs) = piece_claims.get(&(edge.square, edge.way, edge.segment)) else {
                continue;
            };
            let (e_lo, e_hi) = (
                edge.extent[0].min(edge.extent[1]),
                edge.extent[0].max(edge.extent[1]),
            );
            let mut cuts = Vec::with_capacity(2 + 2 * ivs.len());
            cuts.push(e_lo);
            cuts.push(e_hi);
            for iv in ivs {
                cuts.push(iv.from_m.clamp(e_lo, e_hi));
                cuts.push(iv.to_m.clamp(e_lo, e_hi));
            }
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() <= 1e-4);
            for win in cuts.windows(2) {
                if win[1] - win[0] <= 1e-4 {
                    continue;
                }
                let (dom, for_) =
                    row_evidence(model, ivs, win[0], win[1], edge.rail_type, edge.country);
                let Some(tt_src) = valid_tt(dom.passenger, edge.country, false)
                    .or_else(|| valid_tt(for_.passenger, edge.country, true))
                else {
                    continue;
                };
                let s_a = run.edge_extent_to_s(pos, edge, win[0]);
                let s_b = run.edge_extent_to_s(pos, edge, win[1]);
                let (s0, s1) = (s_a.min(s_b), s_a.max(s_b));
                if s1 - s0 > 1e-4 {
                    claimed.push((s0, s1, tt_src));
                }
            }
        }
        own_claimed.push(claimed);
    }
    let mut has_parallel = vec![false; runs.len()];
    for section in sections {
        if section.legs.len() >= 2 {
            for leg in &section.legs {
                has_parallel[leg.run_id] = true;
            }
        }
    }
    // Project timetable brackets across parallel legs of the same physical Section using the same
    // geometric projection as `section_leg_cuts`, so sibling station snaps anchor through-track
    // gaps at exact cut coordinates without creating multi-metre overlap spikes.
    let mut section_claimed: Vec<Vec<(f64, f64, u16)>> = (0..runs.len())
        .map(|rid| {
            if has_parallel[rid] {
                own_claimed[rid].clone()
            } else {
                Vec::new()
            }
        })
        .collect();
    let mut section_ref_claimed: SectionRefClaimed = vec![Vec::new(); sections.len()];
    for (sec_idx, section) in sections.iter().enumerate() {
        if section.legs.len() < 2 {
            continue;
        }
        let ref_leg = &section.legs[0];
        let mut ref_shared: Vec<(f64, f64, u16)> = Vec::new();
        for (leg_idx, leg) in section.legs.iter().enumerate() {
            if leg.s1 - leg.s0 <= 1e-4 {
                continue;
            }
            for &(a, b, src) in &own_claimed[leg.run_id] {
                let lo = a.max(leg.s0);
                let hi = b.min(leg.s1);
                if hi - lo <= 1e-4 {
                    continue;
                }
                let (u0, u1) = if leg_idx == 0 {
                    (lo, hi)
                } else {
                    let pa = project_leg_s_pinned(network, runs, leg, lo, ref_leg);
                    let pb = project_leg_s_pinned(network, runs, leg, hi, ref_leg);
                    (pa.min(pb), pa.max(pb))
                };
                if u1 - u0 > 1e-4 {
                    ref_shared.push((u0, u1, src));
                }
            }
        }
        if ref_shared.is_empty() {
            continue;
        }
        ref_shared.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.total_cmp(&b.0)));
        let mut merged: Vec<(f64, f64, u16)> = Vec::with_capacity(ref_shared.len());
        for (u0, u1, src) in ref_shared {
            if let Some(last) = merged.last_mut()
                && last.2 == src
                && u0 <= last.1 + 1e-4
            {
                last.1 = last.1.max(u1);
            } else {
                merged.push((u0, u1, src));
            }
        }
        for (leg_idx, leg) in section.legs.iter().enumerate() {
            if leg.s1 - leg.s0 <= 1e-4 {
                continue;
            }
            for (other_idx, other) in section.legs.iter().enumerate() {
                if other_idx == leg_idx || other.s1 - other.s0 <= 1e-4 {
                    continue;
                }
                for &(a, b, src) in &own_claimed[other.run_id] {
                    let lo = a.max(other.s0);
                    let hi = b.min(other.s1);
                    if hi - lo <= 1e-4 {
                        continue;
                    }
                    let sa = project_leg_s_pinned(network, runs, other, lo, leg);
                    let sb = project_leg_s_pinned(network, runs, other, hi, leg);
                    let (s0, s1) = (sa.min(sb), sa.max(sb));
                    if s1 - s0 > 1e-4 {
                        section_claimed[leg.run_id].push((s0, s1, src));
                    }
                }
            }
        }
        section_ref_claimed[sec_idx] = merged;
    }
    let mut out = Vec::new();
    let mut seen_gaps: HashSet<(usize, u16, i64, i64)> = HashSet::new();
    for (run_id, run) in runs.iter().enumerate() {
        // On parallel-track runs, recover only genuine section-wide timetable gaps (where no
        // track in the corridor section holds a direct claim for the source). Filling an
        // unclaimed companion track alongside an already-claimed sibling would stack coupled or
        // duplicate GTFS train numbers onto the section's existing passenger total.
        let mut pairs = if has_parallel[run_id] {
            extract_gap_pairs(run, std::mem::take(&mut section_claimed[run_id]))
        } else {
            extract_gap_pairs(run, own_claimed[run_id].clone())
        };
        let boundary_from_continuing_sibling = |s_bound: f64, going_higher: bool, tt_src: u16| {
            let (own_lo, own_hi) = if going_higher {
                (s_bound - 0.05, s_bound)
            } else {
                (s_bound, s_bound + 0.05)
            };
            if run_has_own_claim(run, &own_claimed[run_id], own_lo, own_hi, tt_src) {
                return false;
            }
            for &(rs_s0, rs_s1, sec_idx) in &layout.run_sections[run_id] {
                let section = &sections[sec_idx];
                if section.legs.len() < 2 {
                    continue;
                }
                let at_boundary = if going_higher {
                    (s_bound - rs_s1).abs() <= 0.05
                } else {
                    (s_bound - rs_s0).abs() <= 0.05
                };
                if !at_boundary {
                    continue;
                }
                let Some((my_leg_idx, my_leg)) = section.legs.iter().enumerate().find(|(_, sl)| {
                    sl.run_id == run_id
                        && (sl.s0 - rs_s0).abs() <= 1e-3
                        && (sl.s1 - rs_s1).abs() <= 1e-3
                }) else {
                    continue;
                };
                for (sib_idx, sib) in section.legs.iter().enumerate() {
                    if sib_idx == my_leg_idx {
                        continue;
                    }
                    let sib_past_hi = if my_leg.same_dir == sib.same_dir {
                        going_higher
                    } else {
                        !going_higher
                    };
                    let ((in_lo, in_hi), (out_lo, out_hi)) = if sib_past_hi {
                        ((sib.s1 - 0.05, sib.s1), (sib.s1, sib.s1 + 0.05))
                    } else {
                        ((sib.s0, sib.s0 + 0.05), (sib.s0 - 0.05, sib.s0))
                    };
                    let sib_run = &runs[sib.run_id];
                    let sib_own = &own_claimed[sib.run_id];
                    if run_has_own_claim(sib_run, sib_own, in_lo, in_hi, tt_src)
                        && run_has_own_claim(sib_run, sib_own, out_lo, out_hi, tt_src)
                    {
                        return true;
                    }
                }
            }
            false
        };
        for GapPair {
            left_s,
            right_s,
            source_id: tt_src,
            spans,
            gap_len,
        } in pairs.drain(..)
        {
            // A companion track leaving a multi-track section while its sibling keeps carrying the
            // timetable unbroken past the section boundary is a pairing split, not a corridor gap.
            if boundary_from_continuing_sibling(left_s, true, tt_src)
                || boundary_from_continuing_sibling(right_s, false, tt_src)
            {
                continue;
            }
            let key = (
                run_id,
                tt_src,
                (spans[0].0 * 100.0).round() as i64,
                (spans.last().unwrap().1 * 100.0).round() as i64,
            );
            if !seen_gaps.insert(key) {
                continue;
            }
            let left_edge = &network.edges
                [run.edges[run.edge_pos_at((left_s - 1e-3).rem_euclid(run.total_len))].edge_idx];
            let right_edge = &network.edges
                [run.edges[run.edge_pos_at((right_s + 1e-3).rem_euclid(run.total_len))].edge_idx];
            let mut clean = true;
            for &(s_lo, s_hi) in &spans {
                let start = run.edges.partition_point(|e| e.s1 <= s_lo + 1e-6);
                for re in &run.edges[start..] {
                    if re.s0 >= s_hi - 1e-6 {
                        break;
                    }
                    let edge = &network.edges[re.edge_idx];
                    if edge.traversal_only
                        || !carries_passengers(edge.traffic_mode)
                        || tokens_match(edge, left_edge) == TokenRel::Conflict
                        || tokens_match(edge, right_edge) == TokenRel::Conflict
                    {
                        clean = false;
                        break;
                    }
                    let n0 = edge.nodes[usize::from(re.reversed)];
                    let n1 = edge.nodes[usize::from(!re.reversed)];
                    if (re.s0 >= s_lo - 1e-2
                        && re.s0 <= s_hi + 1e-2
                        && network.adjacency[n0].len() > 2)
                        || (re.s1 >= s_lo - 1e-2
                            && re.s1 <= s_hi + 1e-2
                            && network.adjacency[n1].len() > 2)
                    {
                        clean = false;
                        break;
                    }
                }
                if clean {
                    // Reject a multi-track section gap when any parallel sibling track has a
                    // switch (e.g. a traversal-only crossover) inside or bounding the gap span.
                    let switch_tol = 0.15;
                    for &(rs_s0, rs_s1, sec_idx) in &layout.run_sections[run_id] {
                        let clip_lo = s_lo.max(rs_s0);
                        let clip_hi = s_hi.min(rs_s1);
                        if clip_hi - clip_lo <= 1e-4 {
                            continue;
                        }
                        let section = &sections[sec_idx];
                        if section.legs.len() < 2 {
                            continue;
                        }
                        let Some((my_leg_idx, my_leg)) =
                            section.legs.iter().enumerate().find(|(_, sl)| {
                                sl.run_id == run_id
                                    && (sl.s0 - rs_s0).abs() <= 1e-3
                                    && (sl.s1 - rs_s1).abs() <= 1e-3
                            })
                        else {
                            continue;
                        };
                        for (sib_idx, sib) in section.legs.iter().enumerate() {
                            if sib_idx == my_leg_idx {
                                continue;
                            }
                            let sa = project_leg_s_pinned(network, runs, my_leg, clip_lo, sib);
                            let sb = project_leg_s_pinned(network, runs, my_leg, clip_hi, sib);
                            let (sib_lo, sib_hi) = (sa.min(sb), sa.max(sb));
                            let sib_run = &runs[sib.run_id];
                            let sib_start = sib_run
                                .edges
                                .partition_point(|e| e.s1 <= (sib_lo - switch_tol).max(0.0));
                            for re in &sib_run.edges[sib_start..] {
                                if re.s0 > sib_hi + switch_tol {
                                    break;
                                }
                                let edge = &network.edges[re.edge_idx];
                                let n0 = edge.nodes[usize::from(re.reversed)];
                                let n1 = edge.nodes[usize::from(!re.reversed)];
                                if (re.s0 >= sib_lo - switch_tol
                                    && re.s0 <= sib_hi + switch_tol
                                    && network.adjacency[n0].len() > 2)
                                    || (re.s1 >= sib_lo - switch_tol
                                        && re.s1 <= sib_hi + switch_tol
                                        && network.adjacency[n1].len() > 2)
                                {
                                    clean = false;
                                    break;
                                }
                            }
                            if !clean {
                                break;
                            }
                        }
                        if !clean {
                            break;
                        }
                    }
                }
                if !clean {
                    break;
                }
            }
            if !clean {
                continue;
            }
            let n_steps = ((gap_len / GAP_SAMPLE_STEP_M).ceil() as usize).max(4);
            let mut sample_s = Vec::with_capacity(n_steps + 1);
            let mut samples = Vec::with_capacity(n_steps + 1);
            let mut cursor_base = 0.0;
            let mut span_idx = 0usize;
            for step in 0..=n_steps {
                let d = gap_len * (step as f64 / n_steps as f64);
                while span_idx + 1 < spans.len()
                    && d > cursor_base + (spans[span_idx].1 - spans[span_idx].0) + 1e-6
                {
                    cursor_base += spans[span_idx].1 - spans[span_idx].0;
                    span_idx += 1;
                }
                let (s_lo, s_hi) = spans[span_idx];
                let s = (s_lo + (d - cursor_base)).clamp(s_lo, s_hi);
                sample_s.push(s);
                samples.push(run.point_at(network, s));
            }
            let (mut min_lat, mut min_lon) = (f64::INFINITY, f64::INFINITY);
            let (mut max_lat, mut max_lon) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            for &[lat, lon] in &samples {
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
                min_lon = min_lon.min(lon);
                max_lon = max_lon.max(lon);
            }
            let pad_lat = MAX_GAP_SHAPE_DIST_M / 110_540.0 + 1e-4;
            let cos_lat = (0.5 * (min_lat + max_lat)).to_radians().cos().max(0.05);
            let pad_lon = MAX_GAP_SHAPE_DIST_M / (111_320.0 * cos_lat) + 1e-4;
            out.push(CandidateGap {
                run_id,
                spans,
                source_id: tt_src,
                gap_len,
                sample_s,
                samples,
                bbox: [
                    min_lat - pad_lat,
                    min_lon - pad_lon,
                    max_lat + pad_lat,
                    max_lon + pad_lon,
                ],
            });
        }
    }
    (out, section_ref_claimed)
}

struct SectionPiece {
    run_id: usize,
    leg_idx: usize,
    s_lo: f64,
    s_hi: f64,
    u0: f64,
    u1: f64,
}

fn emit_run_span_claims(
    network: &Network,
    run: &Run,
    leg: &FailedServiceLeg,
    s_lo: f64,
    s_hi: f64,
    recovered: &mut Vec<([u16; 2], Interval)>,
) {
    let start = run.edges.partition_point(|e| e.s1 <= s_lo + 1e-6);
    for (offset, re) in run.edges[start..].iter().enumerate() {
        if re.s0 >= s_hi - 1e-6 {
            break;
        }
        let edge = &network.edges[re.edge_idx];
        let pos = start + offset;
        let m0 = run.s_to_edge_extent(pos, edge, s_lo.max(re.s0));
        let m1 = run.s_to_edge_extent(pos, edge, s_hi.min(re.s1));
        let (e_lo, e_hi) = (m0.min(m1), m0.max(m1));
        if e_hi - e_lo > 1e-4 {
            recovered.push((
                edge.square,
                Interval {
                    missing_calendar_estimate: leg.missing_calendar_estimate,
                    osm_id: edge.way,
                    segment_idx: edge.segment,
                    from_m: e_lo,
                    to_m: e_hi,
                    country: leg.country,
                    source_id: leg.source_id,
                    passenger: leg.passenger,
                    freight: 0.0,
                    passenger_status: leg.passenger_status,
                    freight_status: STATUS_UNKNOWN,
                    matching: 2,
                    stop: 0,
                    resolved_section: None,
                },
            ));
        }
    }
}

/// Recover timetable passenger claims along switch-free, same-corridor, bracketed gaps only where
/// a failed shaped service leg from the same source traverses the entire candidate gap within
/// [`MAX_GAP_SHAPE_DIST_M`] and no aligned track outside its section lies closer to the shape.
/// Competing parallel tracks inside a section are resolved locally per elementary window so
/// recovery is strictly subset-invariant with no cross-section chaining.
#[allow(clippy::too_many_arguments)]
pub(super) fn recover_failed_service_gaps(
    model: &RailModel,
    network: &Network,
    layout: &CorridorLayout,
    piece_claims: &HashMap<([u16; 2], i64, i16), Vec<&Interval>>,
    failed_legs: &[FailedServiceLeg],
    source_id: u16,
    country: [u8; 2],
) -> Vec<([u16; 2], Interval)> {
    if failed_legs.is_empty() {
        return Vec::new();
    }
    let (candidates, section_ref_claimed) =
        candidate_run_gaps(model, network, layout, piece_claims, source_id);
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut recovered = Vec::new();
    for leg in failed_legs {
        if leg.shape.len() < 2
            || !model.source_applies_to_row(leg.source_id, country)
            || model.is_residual(leg.source_id)
            || !model.should_overwrite(source_id, leg.source_id)
            || (leg.country != country && model.blocks_foreign_national(source_id, leg.source_id))
        {
            continue;
        }
        let mut by_section: BTreeMap<usize, Vec<SectionPiece>> = BTreeMap::new();
        for gap in &candidates {
            if gap.source_id != leg.source_id
                || gap.bbox[2] < leg.bbox[0]
                || gap.bbox[0] > leg.bbox[2]
                || gap.bbox[3] < leg.bbox[1]
                || gap.bbox[1] > leg.bbox[3]
            {
                continue;
            }
            let leg_len = *leg.cum_m.last().unwrap_or(&0.0);
            let mut ok = true;
            let (mut arc_first, mut arc_last) = (0.0, 0.0);
            let (mut min_arc, mut max_arc) = (f64::INFINITY, f64::NEG_INFINITY);
            for (i, (&s, &pt)) in gap.sample_s.iter().zip(&gap.samples).enumerate() {
                let (d, arc, shape_pt, shape_dir) =
                    project_onto_polyline(pt, &leg.shape, &leg.cum_m);
                if d > MAX_GAP_SHAPE_DIST_M {
                    ok = false;
                    break;
                }
                if (arc <= 1e-3 || arc >= leg_len - 1e-3)
                    && let Some([ux, uy]) = shape_dir
                {
                    let cos_lat = shape_pt[0].to_radians().cos().max(0.05);
                    let off = to_local_m(shape_pt, cos_lat, pt);
                    if (off[0] * ux + off[1] * uy).abs() > MAX_GAP_TIP_OVERHANG_M {
                        ok = false;
                        break;
                    }
                }
                if let Some((best_edge, best_frac, best_dist)) = layout
                    .grid
                    .nearest_aligned_edge(shape_pt, shape_dir, network)
                    && let Some((best_run, best_pos)) = layout.edge_run[best_edge]
                {
                    let my_sec = layout.section_at(gap.run_id, s);
                    let best_s = layout.runs[best_run].s_of_edge_t(best_pos, best_frac);
                    let best_sec = layout.section_at(best_run, best_s);
                    let my_run = &layout.runs[gap.run_id];
                    let my_e = &network.edges[my_run.edges[my_run.edge_pos_at(s)].edge_idx];
                    let best_e = &network.edges[best_edge];
                    let best_key = (
                        (best_dist * 1000.0).round() as i64,
                        best_e.way,
                        best_e.segment,
                        best_e.square,
                    );
                    let my_key = (
                        (d * 1000.0).round() as i64,
                        my_e.way,
                        my_e.segment,
                        my_e.square,
                    );
                    let same_corridor_loc = match (my_sec, best_sec) {
                        (Some(si), Some(bsi)) if si == bsi => {
                            let section = &layout.sections[si];
                            let ref_leg = &section.legs[0];
                            let ref_run = &layout.runs[ref_leg.run_id];
                            let to_ref_u = |rid: usize, pos_s: f64| -> Option<f64> {
                                section
                                    .legs
                                    .iter()
                                    .enumerate()
                                    .find(|(_, sl)| {
                                        sl.run_id == rid
                                            && pos_s >= sl.s0 - 1e-3
                                            && pos_s <= sl.s1 + 1e-3
                                    })
                                    .map(|(li, sl)| {
                                        if li == 0 {
                                            pos_s
                                        } else {
                                            project_leg_s_pinned(
                                                network,
                                                &layout.runs,
                                                sl,
                                                pos_s,
                                                ref_leg,
                                            )
                                        }
                                    })
                            };
                            match (to_ref_u(gap.run_id, s), to_ref_u(best_run, best_s)) {
                                (Some(u_my), Some(u_best)) => {
                                    let diff = (u_best - u_my).abs();
                                    let wrap_diff = if ref_run.closed {
                                        diff.min((ref_run.total_len - diff).abs())
                                    } else {
                                        diff
                                    };
                                    wrap_diff <= MAX_GAP_SHAPE_DIST_M
                                }
                                _ => true,
                            }
                        }
                        _ => false,
                    };
                    if !same_corridor_loc && best_key < my_key {
                        ok = false;
                        break;
                    }
                }
                if i == 0 {
                    arc_first = arc;
                }
                arc_last = arc;
                min_arc = min_arc.min(arc);
                max_arc = max_arc.max(arc);
            }
            if !ok {
                continue;
            }
            let (t0, t1) = (arc_first.min(arc_last), arc_first.max(arc_last));
            if (t1 - t0) < 0.75 * gap.gap_len
                || min_arc < t0 - MAX_GAP_SHAPE_DIST_M
                || max_arc > t1 + MAX_GAP_SHAPE_DIST_M
            {
                continue;
            }
            for &(s_lo, s_hi) in &gap.spans {
                for &(rs_s0, rs_s1, sec_idx) in &layout.run_sections[gap.run_id] {
                    let clip_lo = s_lo.max(rs_s0);
                    let clip_hi = s_hi.min(rs_s1);
                    if clip_hi - clip_lo <= 1e-4 {
                        continue;
                    }
                    let section = &layout.sections[sec_idx];
                    let Some(leg_idx) = section.legs.iter().position(|sl| {
                        sl.run_id == gap.run_id
                            && (sl.s0 - rs_s0).abs() <= 1e-3
                            && (sl.s1 - rs_s1).abs() <= 1e-3
                    }) else {
                        continue;
                    };
                    let (u0, u1) = if leg_idx == 0 {
                        (clip_lo, clip_hi)
                    } else {
                        let ref_leg = &section.legs[0];
                        let sl = &section.legs[leg_idx];
                        let ua = project_leg_s_pinned(network, &layout.runs, sl, clip_lo, ref_leg);
                        let ub = project_leg_s_pinned(network, &layout.runs, sl, clip_hi, ref_leg);
                        (ua.min(ub), ua.max(ub))
                    };
                    if u1 - u0 > 1e-4 {
                        by_section.entry(sec_idx).or_default().push(SectionPiece {
                            run_id: gap.run_id,
                            leg_idx,
                            s_lo: clip_lo,
                            s_hi: clip_hi,
                            u0,
                            u1,
                        });
                    }
                }
            }
        }
        for (sec_idx, pieces) in by_section {
            let section = &layout.sections[sec_idx];
            let ref_claimed = &section_ref_claimed[sec_idx];
            if section.legs.len() < 2 && pieces.len() == 1 {
                let p = &pieces[0];
                emit_run_span_claims(
                    network,
                    &layout.runs[p.run_id],
                    leg,
                    p.s_lo,
                    p.s_hi,
                    &mut recovered,
                );
                continue;
            }
            let ref_leg = &section.legs[0];
            let mut min_u = f64::INFINITY;
            let mut max_u = f64::NEG_INFINITY;
            let mut cuts = Vec::with_capacity(2 * pieces.len() + 2 * ref_claimed.len());
            for p in &pieces {
                min_u = min_u.min(p.u0);
                max_u = max_u.max(p.u1);
                cuts.push(p.u0);
                cuts.push(p.u1);
            }
            for &(ra, rb, rsrc) in ref_claimed {
                if rsrc == leg.source_id {
                    if ra > min_u && ra < max_u {
                        cuts.push(ra);
                    }
                    if rb > min_u && rb < max_u {
                        cuts.push(rb);
                    }
                }
            }
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() <= 1e-4);
            let mut emitted: Vec<(usize, f64, f64)> = Vec::new();
            for win in cuts.windows(2) {
                let (c0, c1) = (win[0], win[1]);
                if c1 - c0 <= 1e-4 {
                    continue;
                }
                let u_mid = 0.5 * (c0 + c1);
                if ref_claimed
                    .iter()
                    .any(|&(ra, rb, rsrc)| rsrc == leg.source_id && u_mid >= ra && u_mid <= rb)
                {
                    continue;
                }
                let mut ref_piece: Option<&SectionPiece> = None;
                let mut best: Option<(f64, usize, usize, &SectionPiece)> = None;
                for p in &pieces {
                    if u_mid < p.u0 - 1e-5 || u_mid > p.u1 + 1e-5 {
                        continue;
                    }
                    if p.leg_idx == 0 {
                        ref_piece = Some(p);
                        break;
                    }
                    let s_mid = project_leg_s(
                        network,
                        &layout.runs,
                        ref_leg,
                        u_mid,
                        &section.legs[p.leg_idx],
                    );
                    let pt = layout.runs[p.run_id].point_at(network, s_mid);
                    let d_mid = project_onto_polyline(pt, &leg.shape, &leg.cum_m).0;
                    if best.is_none_or(|(bd, br, bl, _)| {
                        d_mid < bd - 1e-6
                            || ((d_mid - bd).abs() <= 1e-6 && (p.run_id, p.leg_idx) < (br, bl))
                    }) {
                        best = Some((d_mid, p.run_id, p.leg_idx, p));
                    }
                }
                let Some(win_p) = ref_piece.or_else(|| best.map(|(_, _, _, p)| p)) else {
                    continue;
                };
                let (s0, s1) = if win_p.leg_idx == 0 {
                    (c0.max(win_p.s_lo), c1.min(win_p.s_hi))
                } else {
                    let sl = &section.legs[win_p.leg_idx];
                    let map_cut = |c: f64| {
                        if (c - win_p.u0).abs() <= 1e-4 {
                            if sl.same_dir { win_p.s_lo } else { win_p.s_hi }
                        } else if (c - win_p.u1).abs() <= 1e-4 {
                            if sl.same_dir { win_p.s_hi } else { win_p.s_lo }
                        } else {
                            project_leg_s_pinned(network, &layout.runs, ref_leg, c, sl)
                        }
                    };
                    let (sa, sb) = (map_cut(c0), map_cut(c1));
                    (sa.min(sb).max(win_p.s_lo), sa.max(sb).min(win_p.s_hi))
                };
                if s1 - s0 <= 1e-4 {
                    continue;
                }
                if let Some(last) = emitted.last_mut()
                    && last.0 == win_p.run_id
                    && (s0 - last.2).abs() <= 1e-3
                {
                    last.2 = last.2.max(s1);
                } else {
                    emitted.push((win_p.run_id, s0, s1));
                }
            }
            for (run_id, s_lo, s_hi) in emitted {
                emit_run_span_claims(
                    network,
                    &layout.runs[run_id],
                    leg,
                    s_lo,
                    s_hi,
                    &mut recovered,
                );
            }
        }
    }
    recovered
}

/// Distribute `pax_daily` across END periods respecting `0 <= pax[p] <= cap[p]` in every period,
/// weighted by `weights`; zero-capacity periods stay zero and overflow spills to open periods.
pub(super) fn allocate_passenger_in_periods(
    cap: [f64; 3],
    pax_daily: f64,
    weights: [f64; 3],
) -> [f64; 3] {
    let cap_sum: f64 = cap.iter().sum();
    let mut remaining = pax_daily.clamp(0.0, cap_sum);
    let mut out = [0.0; 3];
    if remaining <= 1e-12 {
        return out;
    }
    for _ in 0..3 {
        let mut active_w = 0.0;
        let mut headroom = 0.0;
        for p in 0..3 {
            let room = (cap[p] - out[p]).max(0.0);
            if room > 1e-12 {
                active_w += weights[p];
                headroom += room;
            }
        }
        if remaining <= 1e-12 || headroom <= 1e-12 {
            break;
        }
        let mut spilled = 0.0;
        let mut distributed = 0.0;
        for p in 0..3 {
            let room = (cap[p] - out[p]).max(0.0);
            if room <= 1e-12 {
                continue;
            }
            let share = if active_w > 0.0 {
                weights[p] / active_w
            } else {
                room / headroom
            };
            let want = remaining * share;
            let add = want.min(room);
            out[p] += add;
            distributed += add;
            if want > room {
                spilled += want - room;
            }
        }
        remaining = (remaining - distributed).max(0.0);
        if spilled <= 1e-12 {
            break;
        }
    }
    out
}

#[derive(Clone, Copy)]
pub(super) struct AggregatedFlow {
    pub measured_periods: [f64; 3],
    pub unsplit_daily: f64,
    pub total: f64,
    pub measured_passenger: Option<f64>,
    pub passenger_cap: Option<f64>,
    pub obs_tracks: usize,
}

impl AggregatedFlow {
    pub(super) fn scaled(self, factor: f64) -> Self {
        Self {
            measured_periods: self.measured_periods.map(|v| v * factor),
            unsplit_daily: self.unsplit_daily * factor,
            total: self.total * factor,
            measured_passenger: self.measured_passenger.map(|v| v * factor),
            passenger_cap: self.passenger_cap.map(|v| v * factor),
            obs_tracks: self.obs_tracks,
        }
    }
}

fn higher_rank_freight(
    model: &RailModel,
    country: [u8; 2],
    source_id: u16,
    idx_vec: &[usize],
    domestic: &[RowTraffic],
    foreign: &[RowTraffic],
) -> Option<CategoryFlow> {
    let dom = scope_winner(model, country, idx_vec, domestic, false, |t| t.freight);
    if dom != 0
        && dom != source_id
        && !model.is_residual(dom)
        && model.should_overwrite(source_id, dom)
    {
        return Some(scope_value(model, idx_vec, domestic, dom, |t| t.freight));
    }
    let for_ = scope_winner(model, country, idx_vec, foreign, false, |t| t.freight);
    if for_ != 0
        && for_ != source_id
        && !model.is_residual(for_)
        && !model.blocks_foreign_national(source_id, for_)
        && model.should_overwrite(source_id, for_)
    {
        return Some(scope_value(model, idx_vec, foreign, for_, |t| t.freight));
    }
    None
}

fn winning_timetable(
    model: &RailModel,
    country: [u8; 2],
    source_id: u16,
    idx_vec: &[usize],
    domestic: &[RowTraffic],
    foreign: &[RowTraffic],
) -> Option<CategoryFlow> {
    let dom = scope_winner(model, country, idx_vec, domestic, true, |t| t.passenger);
    if dom != 0 && !model.is_residual(dom) && model.should_overwrite(source_id, dom) {
        return Some(scope_value(model, idx_vec, domestic, dom, |t| t.passenger));
    }
    let for_ = scope_winner(model, country, idx_vec, foreign, true, |t| t.passenger);
    if for_ != 0
        && !model.is_residual(for_)
        && !model.blocks_foreign_national(source_id, for_)
        && model.should_overwrite(source_id, for_)
    {
        return Some(scope_value(model, idx_vec, foreign, for_, |t| t.passenger));
    }
    None
}

fn probe_leg_evidence(
    model: &RailModel,
    network: &Network,
    run: &Run,
    s_mid: f64,
    piece_claims: &HashMap<([u16; 2], i64, i16), Vec<&Interval>>,
) -> (RowTraffic, RowTraffic, usize) {
    let pos = run.edge_pos_at(s_mid);
    let edge_idx = run.edges[pos].edge_idx;
    let edge = &network.edges[edge_idx];
    let ivs = piece_claims
        .get(&(edge.square, edge.way, edge.segment))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (e_lo, e_hi) = (
        edge.extent[0].min(edge.extent[1]),
        edge.extent[0].max(edge.extent[1]),
    );
    let mut p = run.s_to_edge_extent(pos, edge, s_mid).clamp(e_lo, e_hi);
    let near_cut = ivs
        .iter()
        .any(|iv| (iv.from_m - p).abs() < 1e-4 || (iv.to_m - p).abs() < 1e-4);
    if near_cut && e_hi - e_lo > 1e-4 {
        let step = (0.25 * (e_hi - e_lo)).min(1e-3);
        p = if p + step < e_hi {
            p + step
        } else {
            (p - step).max(e_lo)
        };
    }
    let (q_lo, q_hi) = if e_hi - e_lo <= 1e-4 {
        (e_lo, e_hi)
    } else {
        ((p - 1e-6).max(e_lo), (p + 1e-6).min(e_hi))
    };
    let (dom, for_) = row_evidence(model, ivs, q_lo, q_hi, edge.rail_type, edge.country);
    (dom, for_, edge_idx)
}

fn scale_flow(flow: CategoryFlow, factor: f64) -> CategoryFlow {
    CategoryFlow {
        periods: flow.periods.map(|v| v * factor),
        missing_calendar_estimate: flow.missing_calendar_estimate.map(|v| v * factor),
        ..flow
    }
}

fn zero_like(flow: CategoryFlow, fallback_source: u16) -> CategoryFlow {
    CategoryFlow {
        periods: [0.0; 3],
        missing_calendar_estimate: [0.0; 3],
        status: STATUS_ESTIMATED,
        source_id: if flow.source_id != 0 {
            flow.source_id
        } else {
            fallback_source
        },
        matching: if flow.matching != 0 { flow.matching } else { 2 },
        stop: 0,
    }
}

fn section_touches_squares(
    section: &Section,
    runs: &[Run],
    network: &Network,
    squares: &HashSet<[u16; 2]>,
) -> bool {
    section.legs.iter().any(|leg| {
        let run = &runs[leg.run_id];
        let start = run.edges.partition_point(|e| e.s1 <= leg.s0 + 1e-6);
        run.edges[start..]
            .iter()
            .take_while(|e| e.s0 < leg.s1 - 1e-6)
            .any(|re| squares.contains(&network.edges[re.edge_idx].square))
    })
}

type PieceClaims<'a> = HashMap<([u16; 2], i64, i16), Vec<&'a Interval>>;

fn section_leg_cuts<'a>(
    section: &Section,
    runs: &[Run],
    network: &Network,
    piece_claims: &PieceClaims<'a>,
    gap_claims: Option<&PieceClaims<'a>>,
) -> Vec<Vec<f64>> {
    let ref_leg = &section.legs[0];
    let mut u_entries: Vec<(f64, Option<(usize, f64)>)> =
        vec![(ref_leg.s0, None), (ref_leg.s1, None)];
    let mut push_cut = |leg_idx: usize, leg: &SectionLeg, s: f64| {
        if leg_idx == 0 {
            u_entries.push((s, Some((0, s))));
        } else {
            let u = project_leg_s(network, runs, leg, s, ref_leg);
            u_entries.push((u, Some((leg_idx, s))));
        }
    };
    for (leg_idx, leg) in section.legs.iter().enumerate() {
        let run = &runs[leg.run_id];
        let start = run.edges.partition_point(|e| e.s1 <= leg.s0 + 1e-6);
        let mut prev_mode: Option<u8> = None;
        for (offset, re) in run.edges[start..].iter().enumerate() {
            if re.s0 >= leg.s1 - 1e-6 {
                break;
            }
            let pos = start + offset;
            let edge = &network.edges[re.edge_idx];
            if prev_mode.is_some_and(|m| m != edge.traffic_mode) {
                push_cut(leg_idx, leg, re.s0.clamp(leg.s0, leg.s1));
            }
            prev_mode = Some(edge.traffic_mode);
            let key = (edge.square, edge.way, edge.segment);
            for claims_map in [Some(piece_claims), gap_claims].into_iter().flatten() {
                if let Some(ivs) = claims_map.get(&key) {
                    for iv in ivs {
                        for m in [iv.from_m, iv.to_m] {
                            let s = run.edge_extent_to_s(pos, edge, m).clamp(leg.s0, leg.s1);
                            push_cut(leg_idx, leg, s);
                        }
                    }
                }
            }
        }
    }
    u_entries.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts: Vec<(f64, Vec<(usize, f64)>)> = Vec::with_capacity(u_entries.len());
    for (u, anchor) in u_entries {
        let can_merge = cuts.last().is_some_and(|(prev_u, anchors)| {
            u - *prev_u <= 1e-4
                && anchor.is_none_or(|(li, os)| {
                    anchors
                        .iter()
                        .all(|&(ali, aos)| ali != li || (aos - os).abs() <= 1e-4)
                })
        });
        if can_merge {
            if let Some(a) = anchor {
                cuts.last_mut().unwrap().1.push(a);
            }
        } else {
            cuts.push((u, anchor.into_iter().collect()));
        }
    }
    if cuts.last().map(|c| c.0).unwrap_or(ref_leg.s0) < ref_leg.s1 - 1e-4 {
        cuts.push((ref_leg.s1, Vec::new()));
    }
    if cuts.len() < 2 {
        cuts = vec![(ref_leg.s0, Vec::new()), (ref_leg.s1, Vec::new())];
    }
    let mut leg_cuts: Vec<Vec<f64>> = Vec::with_capacity(section.legs.len());
    for (leg_idx, leg) in section.legs.iter().enumerate() {
        if leg_idx == 0 {
            leg_cuts.push(cuts.iter().map(|(u, _)| *u).collect());
            continue;
        }
        let mut pts = Vec::with_capacity(cuts.len());
        pts.push(if leg.same_dir { leg.s0 } else { leg.s1 });
        for (u, anchors) in &cuts[1..cuts.len() - 1] {
            // Keep originating sibling cuts at their exact coordinate instead of round-tripping.
            let mut s = if let Some(&(_, orig_s)) = anchors.iter().find(|&&(li, _)| li == leg_idx) {
                orig_s
            } else {
                project_leg_s(network, runs, ref_leg, *u, leg)
            };
            let prev = *pts.last().unwrap();
            s = if leg.same_dir {
                s.clamp(prev, leg.s1)
            } else {
                s.clamp(leg.s0, prev)
            };
            pts.push(s);
        }
        pts.push(if leg.same_dir { leg.s1 } else { leg.s0 });
        leg_cuts.push(pts);
    }
    leg_cuts
}

struct LineTotals {
    total_periods: [f64; 3],
    total_daily: f64,
    has_measured: bool,
    known_pax_floor: f64,
    has_explicit_pax: bool,
    pax_shares: [f64; 3],
    frt_shares: [f64; 3],
}

#[allow(clippy::too_many_arguments)]
fn resolve_window_line_traffic(
    carriers: &[AggregatedFlow],
    totals: &LineTotals,
    prior_ratio: f64,
    pax_eligible: bool,
    source_id: u16,
    win_tt: Option<CategoryFlow>,
    high_frt: Option<CategoryFlow>,
) -> RowTraffic {
    let estimated_flow = |periods: [f64; 3]| CategoryFlow {
        periods,
        missing_calendar_estimate: [0.0; 3],
        status: STATUS_ESTIMATED,
        source_id,
        matching: 2,
        stop: 0,
    };
    let pax_periods_for = |pax_line: f64| {
        if totals.has_measured {
            allocate_passenger_in_periods(totals.total_periods, pax_line, totals.pax_shares)
        } else {
            totals.pax_shares.map(|s| pax_line * s)
        }
    };
    if let Some(raw_frt) = high_frt {
        let passenger = if let Some(raw_pax) = win_tt {
            raw_pax
        } else if totals.has_explicit_pax {
            let frt_daily: f64 = raw_frt.periods.iter().sum();
            let rem_daily = (totals.total_daily - frt_daily).max(0.0);
            let pax_line = totals.known_pax_floor.clamp(0.0, rem_daily);
            let pax_periods = if totals.has_measured {
                let rem_cap =
                    [0, 1, 2].map(|p| (totals.total_periods[p] - raw_frt.periods[p]).max(0.0));
                allocate_passenger_in_periods(rem_cap, pax_line, totals.pax_shares)
            } else {
                totals.pax_shares.map(|s| pax_line * s)
            };
            estimated_flow(pax_periods)
        } else {
            let frt_daily: f64 = raw_frt.periods.iter().sum();
            let mut rem = if pax_eligible {
                (totals.total_daily - frt_daily).max(0.0)
            } else {
                0.0
            };
            if !carriers.is_empty() && carriers.iter().all(|c| c.passenger_cap.is_some()) {
                let cap: f64 = carriers.iter().filter_map(|c| c.passenger_cap).sum();
                if rem >= cap && rem > 0.0 {
                    rem = cap.next_down().max(0.0);
                }
            }
            let pax_periods = if totals.has_measured {
                subtract_passenger_periods(totals.total_periods, raw_frt.periods, rem)
            } else {
                totals.pax_shares.map(|s| rem * s)
            };
            estimated_flow(pax_periods)
        };
        return RowTraffic {
            passenger,
            freight: raw_frt,
        };
    }
    if let Some(raw_pax) = win_tt {
        let tt_daily: f64 = raw_pax.periods.iter().sum();
        let effective_pax = tt_daily.max(totals.known_pax_floor);
        let target_frt = (totals.total_daily - effective_pax).max(0.0);
        let frt_line = if totals.has_measured {
            subtract_passenger_periods(totals.total_periods, raw_pax.periods, target_frt)
        } else {
            totals.frt_shares.map(|s| target_frt * s)
        };
        return RowTraffic {
            passenger: raw_pax,
            freight: estimated_flow(frt_line),
        };
    }
    let pax_line: f64 = carriers
        .iter()
        .map(|c| {
            if let Some(p) = c.measured_passenger {
                return p;
            }
            let base = c.passenger_cap.map_or(c.total, |cap| c.total.min(cap));
            let mut est = base * prior_ratio;
            if let Some(cap) = c.passenger_cap
                && est >= cap
                && c.total > 0.0
            {
                est = cap.next_down().max(0.0);
            }
            est.min(c.total)
        })
        .sum::<f64>()
        .clamp(0.0, totals.total_daily);
    let pax_periods = pax_periods_for(pax_line);
    let target_frt = (totals.total_daily - pax_line).max(0.0);
    let frt_periods = if totals.has_measured {
        [
            (totals.total_periods[0] - pax_periods[0]).max(0.0),
            (totals.total_periods[1] - pax_periods[1]).max(0.0),
            (totals.total_periods[2] - pax_periods[2]).max(0.0),
        ]
    } else {
        totals.frt_shares.map(|s| target_frt * s)
    };
    RowTraffic {
        passenger: estimated_flow(pax_periods),
        freight: estimated_flow(frt_periods),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_section(
    section: &Section,
    carriers: &[AggregatedFlow],
    model: &RailModel,
    network: &Network,
    runs: &[Run],
    piece_claims: &HashMap<([u16; 2], i64, i16), Vec<&Interval>>,
    gap_claims: &HashMap<([u16; 2], i64, i16), Vec<&Interval>>,
    source_id: u16,
    country: [u8; 2],
    squares: &HashSet<[u16; 2]>,
) -> Vec<([u16; 2], Interval)> {
    if carriers.is_empty() || !section_touches_squares(section, runs, network, squares) {
        return Vec::new();
    }
    let phys_tracks = section.phys_tracks.max(1) as f64;
    let shares = model.period_shares(country, 0);
    let mut measured_line = [0.0; 3];
    let mut unsplit_line = 0.0;
    let mut known_pax_floor = 0.0;
    let mut has_explicit_pax = false;
    for c in carriers {
        for (sum, val) in measured_line.iter_mut().zip(c.measured_periods) {
            *sum += val;
        }
        unsplit_line += c.unsplit_daily;
        if let Some(p) = c.measured_passenger {
            known_pax_floor += p;
            has_explicit_pax = true;
        }
    }
    let total_periods = [
        measured_line[0] + unsplit_line * shares.frt[0],
        measured_line[1] + unsplit_line * shares.frt[1],
        measured_line[2] + unsplit_line * shares.frt[2],
    ];
    let totals = LineTotals {
        total_periods,
        total_daily: total_periods.iter().sum(),
        has_measured: measured_line.iter().sum::<f64>() > 0.0,
        known_pax_floor,
        has_explicit_pax,
        pax_shares: shares.pax,
        frt_shares: shares.frt,
    };
    // Failed-leg recovery only fills crossing windows whose own passenger count is zero (such as
    // freight-only crossings freezing a timetable gap like Netcong-Lake Hopatcong). When a
    // crossing already reports positive passenger traffic, a partial or coupled failed-leg
    // subtotal must not overwrite or split it.
    let active_gaps =
        (totals.known_pax_floor <= 0.0 && !gap_claims.is_empty()).then_some(gap_claims);
    let leg_cuts = section_leg_cuts(section, runs, network, piece_claims, active_gaps);
    let n_win = leg_cuts[0].len() - 1;
    let idx_vec: Vec<usize> = (0..section.legs.len()).collect();
    let mut domestic = Vec::with_capacity(section.legs.len());
    let mut foreign = Vec::with_capacity(section.legs.len());
    let mut gap_dom = Vec::with_capacity(section.legs.len());
    let mut gap_for = Vec::with_capacity(section.legs.len());
    let mut can_pax = Vec::with_capacity(section.legs.len());
    let mut can_frt = Vec::with_capacity(section.legs.len());
    type EdgeKey = ([u16; 2], i64, i16);
    type ResolvedSpan = (f64, f64, RowTraffic);
    let mut per_edge: HashMap<EdgeKey, Vec<ResolvedSpan>> = HashMap::new();

    for w in 0..n_win {
        domestic.clear();
        foreign.clear();
        can_pax.clear();
        can_frt.clear();
        let (mut pax_prior, mut frt_prior) = (0.0_f64, 0.0_f64);
        let (mut pax_phys, mut frt_phys) = (0.0_f64, 0.0_f64);
        for (leg_idx, leg) in section.legs.iter().enumerate() {
            let s_mid = 0.5 * (leg_cuts[leg_idx][w] + leg_cuts[leg_idx][w + 1]);
            let (dom, for_, edge_idx) =
                probe_leg_evidence(model, network, &runs[leg.run_id], s_mid, piece_claims);
            let e = &network.edges[edge_idx];
            let allow_pax = carries_passengers(e.traffic_mode);
            let allow_frt = carries_freight(e.traffic_mode);
            let (pp, fp) = model.default_traffic(e.rail_type, e.usage, country, e.traffic_mode);
            if allow_pax {
                pax_prior = pax_prior.max(pp);
                pax_phys += leg.slot_weight;
            }
            if allow_frt {
                frt_prior = frt_prior.max(fp);
                frt_phys += leg.slot_weight;
            }
            can_pax.push(allow_pax);
            can_frt.push(allow_frt);
            domestic.push(dom);
            foreign.push(for_);
        }
        let prior_ratio = if pax_prior + frt_prior > 0.0 {
            pax_prior / (pax_prior + frt_prior)
        } else {
            0.0
        };
        let win_tt = winning_timetable(model, country, source_id, &idx_vec, &domestic, &foreign);
        let high_frt =
            higher_rank_freight(model, country, source_id, &idx_vec, &domestic, &foreign);
        let mut line_flow = resolve_window_line_traffic(
            carriers,
            &totals,
            prior_ratio,
            pax_phys > 0.0,
            source_id,
            win_tt,
            high_frt,
        );
        if win_tt.is_none()
            && line_flow.passenger.periods.iter().sum::<f64>() <= 0.0
            && let Some(gc) = active_gaps
        {
            gap_dom.clear();
            gap_for.clear();
            for (leg_idx, leg) in section.legs.iter().enumerate() {
                let s_mid = 0.5 * (leg_cuts[leg_idx][w] + leg_cuts[leg_idx][w + 1]);
                let (dom, for_, _) =
                    probe_leg_evidence(model, network, &runs[leg.run_id], s_mid, gc);
                gap_dom.push(dom);
                gap_for.push(for_);
            }
            if let Some(gap_tt) =
                winning_timetable(model, country, source_id, &idx_vec, &gap_dom, &gap_for)
            {
                line_flow = resolve_window_line_traffic(
                    carriers,
                    &totals,
                    prior_ratio,
                    pax_phys > 0.0,
                    source_id,
                    Some(gap_tt),
                    high_frt,
                );
            }
        }
        let zero_pax = zero_like(line_flow.passenger, source_id);
        let zero_frt = zero_like(line_flow.freight, source_id);
        for (leg_idx, leg) in section.legs.iter().enumerate() {
            let s_a = leg_cuts[leg_idx][w].min(leg_cuts[leg_idx][w + 1]);
            let s_b = leg_cuts[leg_idx][w].max(leg_cuts[leg_idx][w + 1]);
            if s_b - s_a <= 1e-4 {
                continue;
            }
            let passenger = if pax_phys > 0.0 {
                if can_pax[leg_idx] {
                    scale_flow(line_flow.passenger, leg.slot_weight / pax_phys)
                } else {
                    zero_pax
                }
            } else {
                scale_flow(line_flow.passenger, leg.slot_weight / phys_tracks)
            };
            let freight = if frt_phys > 0.0 {
                if can_frt[leg_idx] {
                    scale_flow(line_flow.freight, leg.slot_weight / frt_phys)
                } else {
                    zero_frt
                }
            } else {
                scale_flow(line_flow.freight, leg.slot_weight / phys_tracks)
            };
            let resolved = RowTraffic { passenger, freight };
            let run = &runs[leg.run_id];
            let start = run.edges.partition_point(|e| e.s1 <= s_a + 1e-6);
            for (offset, re) in run.edges[start..].iter().enumerate() {
                if re.s0 >= s_b - 1e-6 {
                    break;
                }
                let edge = &network.edges[re.edge_idx];
                if !squares.contains(&edge.square) {
                    continue;
                }
                let pos = start + offset;
                let m0 = run.s_to_edge_extent(pos, edge, s_a.max(re.s0));
                let m1 = run.s_to_edge_extent(pos, edge, s_b.min(re.s1));
                let (e_lo, e_hi) = (m0.min(m1), m0.max(m1));
                if e_hi - e_lo > 1e-4 {
                    per_edge
                        .entry((edge.square, edge.way, edge.segment))
                        .or_default()
                        .push((e_lo, e_hi, resolved));
                }
            }
        }
    }
    let mut out = Vec::new();
    for ((square, osm_id, segment_idx), mut spans) in per_edge {
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64, RowTraffic)> = Vec::with_capacity(spans.len());
        for (lo, hi, rt) in spans {
            if let Some(prev) = merged.last_mut()
                && (lo - prev.1).abs() <= 1e-3
                && prev.2 == rt
            {
                prev.1 = prev.1.max(hi);
            } else {
                merged.push((lo, hi, rt));
            }
        }
        for (from_m, to_m, rt) in merged {
            out.push((
                square,
                Interval {
                    missing_calendar_estimate: [0.0; 2],
                    osm_id,
                    segment_idx,
                    from_m,
                    to_m,
                    country,
                    source_id,
                    passenger: 0.0,
                    freight: 0.0,
                    passenger_status: STATUS_UNKNOWN,
                    freight_status: STATUS_UNKNOWN,
                    matching: 2,
                    stop: 0,
                    resolved_section: Some(rt),
                },
            ));
        }
    }
    out
}
