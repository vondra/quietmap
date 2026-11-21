//! Partition parallel runs into canonical-reference physical sections with local name resolution.
use super::corridor_runs::{EdgeGrid, Run, TokenRel, build_runs, tokens_match};
use super::corridor_spans::{MIN_OVERLAP_M, Span, build_spans};
use super::network::{Edge, Network};
use std::collections::{HashSet, VecDeque};

const SLIVER_SNAP_M: f64 = 2.0;
const CUT_DEDUP_M: f64 = 0.5;
// Two distinct ways within 0.5 m share one physical track (standard gauge alone is 1.435 m).
const COINCIDENT_DUP_SEP_M: f64 = 0.5;

pub(super) type RunSectionSpan = (f64, f64, usize);

#[derive(Clone, Debug)]
pub(super) struct SectionLeg {
    pub run_id: usize,
    pub s0: f64,
    pub s1: f64,
    pub same_dir: bool,
    pub slot: usize,
    pub slot_weight: f64,
}

#[derive(Clone, Debug)]
pub(super) struct Section {
    /// Canonical reference leg is always `legs[0]` (`same_dir == true`).
    pub legs: Vec<SectionLeg>,
    /// Number of distinct physical tracks represented by `legs` (coincident OSM duplicates share a slot).
    pub phys_tracks: usize,
}

pub(super) struct CorridorLayout {
    pub runs: Vec<Run>,
    pub edge_run: Vec<Option<(usize, usize)>>,
    pub sections: Vec<Section>,
    pub run_sections: Vec<Vec<(f64, f64, usize)>>,
    pub grid: EdgeGrid,
}

impl CorridorLayout {
    pub(super) fn build(
        network: &Network,
        country: [u8; 2],
        yard_keys: &HashSet<([u16; 2], i64, i16)>,
    ) -> Self {
        let eligible: Vec<bool> = network
            .edges
            .iter()
            .map(|e| {
                !e.traversal_only
                    && e.rail_type == 0
                    && e.country == country
                    && e.length > 1e-3
                    && !yard_keys.contains(&(e.square, e.way, e.segment))
            })
            .collect();
        let grid = EdgeGrid::build(network, &eligible);
        let (runs, edge_run) = build_runs(network, &eligible);
        let spans = build_spans(network, &runs, &edge_run, &grid);
        let (sections, run_sections) = partition_sections(network, &runs, &spans);
        Self {
            runs,
            edge_run,
            sections,
            run_sections,
            grid,
        }
    }

    pub(super) fn section_at(&self, run_id: usize, s: f64) -> Option<usize> {
        let list = &self.run_sections[run_id];
        let pos = list.partition_point(|&(_, hi, _)| hi < s);
        list.get(pos)
            .or_else(|| list.last())
            .map(|&(_, _, sec_idx)| sec_idx)
    }
}

fn uncovered_containing(unc: &[(f64, f64)], s: f64) -> Option<(f64, f64)> {
    unc.iter()
        .copied()
        .find(|&(lo, hi)| s >= lo - 1e-6 && s <= hi + 1e-6)
}

fn subtract_interval_bounded(
    unc: &mut Vec<(f64, f64)>,
    lo: f64,
    hi: f64,
    snap_min: f64,
    snap_max: f64,
) -> (f64, f64) {
    let (mut adj_lo, mut adj_hi) = (lo, hi);
    let mut next = Vec::with_capacity(unc.len() + 1);
    for &(a, b) in unc.iter() {
        if hi <= a + 1e-9 || lo >= b - 1e-9 {
            next.push((a, b));
            continue;
        }
        let left = lo - a;
        let right = b - hi;
        if left > SLIVER_SNAP_M || a < snap_min - 1e-6 {
            if left > 1e-6 {
                next.push((a, lo));
            }
        } else if left >= 0.0 {
            adj_lo = adj_lo.min(a);
        }
        if right > SLIVER_SNAP_M || b > snap_max + 1e-6 {
            if right > 1e-6 {
                next.push((hi, b));
            }
        } else if right >= 0.0 {
            adj_hi = adj_hi.max(b);
        }
    }
    *unc = next;
    (adj_lo, adj_hi)
}

fn filter_competing_names(
    network: &Network,
    runs: &[Run],
    legs: &[(usize, f64, bool)],
    edges: &[(usize, usize, f64)],
) -> Vec<bool> {
    let n = legs.len();
    let leg_edge = |idx: usize| -> &Edge {
        let (r, s, _) = legs[idx];
        &network.edges[runs[r].edges[runs[r].edge_pos_at(s)].edge_idx]
    };
    let in_conflict: Vec<bool> = (0..n)
        .map(|i| {
            (0..n).any(|j| i != j && tokens_match(leg_edge(i), leg_edge(j)) == TokenRel::Conflict)
        })
        .collect();
    if !in_conflict.iter().any(|&c| c) {
        return vec![true; n];
    }
    // Seed only legs that actually participate in a conflict so a name-only bridge leg (e.g. `Main`
    // between `R1/Main` and `R2/Main`) inherits its nearest competing corridor symmetrically.
    let mut best: Vec<Option<(f64, &str, usize)>> = vec![None; n];
    for (i, slot) in best.iter_mut().enumerate() {
        if in_conflict[i] {
            let e = leg_edge(i);
            *slot = Some((0.0, e.corridor.as_str(), i));
        }
    }
    for _ in 0..n {
        let mut changed = false;
        for &(u, v, w) in edges {
            for (from, to) in [(u, v), (v, u)] {
                let Some((dist, name, seed)) = best[from] else {
                    continue;
                };
                let cand = (dist + w, name, seed);
                let better = best[to].is_none_or(|cur| {
                    cand.0
                        .total_cmp(&cur.0)
                        .then_with(|| cand.1.cmp(cur.1))
                        .is_lt()
                });
                if better {
                    best[to] = Some(cand);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let Some((_, _, ref_seed)) = best[0] else {
        return vec![true; n];
    };
    let ref_edge = leg_edge(ref_seed);
    (0..n)
        .map(|i| match best[i] {
            Some((_, _, seed)) => tokens_match(ref_edge, leg_edge(seed)) != TokenRel::Conflict,
            None => true,
        })
        .collect()
}

/// Group coincident OSM duplicate ways (`< 0.5 m` apart) into shared physical track slots.
fn assign_physical_slots(
    network: &Network,
    runs: &[Run],
    sec_legs: &mut [SectionLeg],
    orig_indices: &[usize],
    adj_edges: &[(usize, usize, f64)],
) -> usize {
    let k = sec_legs.len();
    if k <= 1 {
        return k;
    }
    let mut parent: Vec<usize> = (0..k).collect();
    let find = |parent: &mut [usize], mut x: usize| {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    };
    let unite = |parent: &mut [usize], a: usize, b: usize| {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[rb] = ra;
        }
    };
    for &(u, v, w) in adj_edges {
        if w >= COINCIDENT_DUP_SEP_M {
            continue;
        }
        if let (Some(a), Some(b)) = (
            orig_indices.iter().position(|&o| o == u),
            orig_indices.iter().position(|&o| o == v),
        ) {
            unite(&mut parent, a, b);
        }
    }
    let mids: Vec<[f64; 2]> = sec_legs
        .iter()
        .map(|leg| runs[leg.run_id].point_at(network, 0.5 * (leg.s0 + leg.s1)))
        .collect();
    for a in 0..k {
        for b in (a + 1)..k {
            if sec_legs[a].run_id != sec_legs[b].run_id
                && crate::osm::geometry::flat_dist(mids[a][0], mids[a][1], mids[b][0], mids[b][1])
                    < COINCIDENT_DUP_SEP_M
            {
                unite(&mut parent, a, b);
            }
        }
    }
    let mut root_to_slot: Vec<Option<usize>> = vec![None; k];
    let mut slot_counts: Vec<usize> = Vec::new();
    for (i, leg) in sec_legs.iter_mut().enumerate() {
        let r = find(&mut parent, i);
        let slot = match root_to_slot[r] {
            Some(s) => s,
            None => {
                let s = slot_counts.len();
                root_to_slot[r] = Some(s);
                slot_counts.push(0);
                s
            }
        };
        leg.slot = slot;
        slot_counts[slot] += 1;
    }
    for leg in sec_legs.iter_mut() {
        leg.slot_weight = 1.0 / (slot_counts[leg.slot] as f64);
    }
    slot_counts.len()
}

fn collect_reference_cuts(
    network: &Network,
    runs: &[Run],
    spans: &[Span],
    run_spans: &[Vec<usize>],
    r_ref: usize,
    ua: f64,
    ub: f64,
) -> Vec<f64> {
    let mut u_cuts = vec![ua, ub];
    let mut bfs_q = VecDeque::from([(r_ref, ua, ub)]);
    let mut seen_spans = HashSet::new();
    while let Some((cur_r, lo, hi)) = bfs_q.pop_front() {
        for &sp_idx in &run_spans[cur_r] {
            let sp = &spans[sp_idx];
            let sides = if sp.run_a == sp.run_b {
                vec![(sp.s_a, sp.s_b, sp.run_b), (sp.s_b, sp.s_a, sp.run_a)]
            } else if sp.run_a == cur_r {
                vec![(sp.s_a, sp.s_b, sp.run_b)]
            } else {
                vec![(sp.s_b, sp.s_a, sp.run_a)]
            };
            for (src, dst, next_r) in sides {
                if src[1] <= lo + 1e-6 || src[0] >= hi - 1e-6 {
                    continue;
                }
                if cur_r == r_ref {
                    u_cuts.push(src[0].clamp(ua, ub));
                    u_cuts.push(src[1].clamp(ua, ub));
                } else {
                    for end_s in src {
                        let pt = runs[cur_r].point_at(network, end_s);
                        let (dist, u_proj) =
                            runs[r_ref].project_onto_range(network, pt, ua, ub, None);
                        if dist <= 250.0 {
                            u_cuts.push(u_proj.clamp(ua, ub));
                        }
                    }
                }
                if seen_spans.insert((sp_idx, next_r)) {
                    bfs_q.push_back((next_r, dst[0], dst[1]));
                }
            }
        }
    }
    u_cuts.sort_by(f64::total_cmp);
    let mut cuts = Vec::with_capacity(u_cuts.len());
    for c in u_cuts {
        if cuts.last().is_none_or(|prev: &f64| c - *prev > CUT_DEDUP_M) {
            cuts.push(c);
        }
    }
    if cuts.last().copied().unwrap_or(ua) < ub - 1e-6 {
        cuts.push(ub);
    }
    if cuts.len() < 2 { vec![ua, ub] } else { cuts }
}

fn partition_sections(
    network: &Network,
    runs: &[Run],
    spans: &[Span],
) -> (Vec<Section>, Vec<Vec<RunSectionSpan>>) {
    let mut run_spans: Vec<Vec<usize>> = vec![Vec::new(); runs.len()];
    for (idx, sp) in spans.iter().enumerate() {
        run_spans[sp.run_a].push(idx);
        if sp.run_b != sp.run_a {
            run_spans[sp.run_b].push(idx);
        }
    }
    let mut uncovered: Vec<Vec<(f64, f64)>> =
        runs.iter().map(|r| vec![(0.0, r.total_len)]).collect();
    let mut sections: Vec<Section> = Vec::new();
    let mut run_sections: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); runs.len()];

    for r_ref in 0..runs.len() {
        if run_spans[r_ref].is_empty() {
            let sec_idx = sections.len();
            sections.push(Section {
                legs: vec![SectionLeg {
                    run_id: r_ref,
                    s0: 0.0,
                    s1: runs[r_ref].total_len,
                    same_dir: true,
                    slot: 0,
                    slot_weight: 1.0,
                }],
                phys_tracks: 1,
            });
            run_sections[r_ref].push((0.0, runs[r_ref].total_len, sec_idx));
            continue;
        }
        while let Some((ua, ub)) = uncovered[r_ref].first().copied() {
            let cuts = collect_reference_cuts(network, runs, spans, &run_spans, r_ref, ua, ub);
            let mut prev_sec: Option<usize> = None;
            for win in cuts.windows(2) {
                let (w0, w1) = (win[0], win[1]);
                let u_mid = 0.5 * (w0 + w1);
                if uncovered_containing(&uncovered[r_ref], u_mid).is_none() {
                    prev_sec = None;
                    continue;
                }
                let mut legs_mid: Vec<(usize, f64, bool)> = vec![(r_ref, u_mid, true)];
                let mut leg_bounds: Vec<[f64; 2]> = vec![[w0, w1]];
                let mut adj_edges: Vec<(usize, usize, f64)> = Vec::new();
                let mut queue = VecDeque::from([0usize]);
                while let Some(li) = queue.pop_front() {
                    let (cur_r, cur_s, cur_dir) = legs_mid[li];
                    for &sp_idx in &run_spans[cur_r] {
                        let sp = &spans[sp_idx];
                        let Some((next_r, next_s, dst_bounds, sp_dir)) = sp.map_from(cur_r, cur_s)
                        else {
                            continue;
                        };
                        if uncovered_containing(&uncovered[next_r], next_s).is_none() {
                            continue;
                        }
                        let next_dir = cur_dir == sp_dir;
                        let existing = legs_mid.iter().position(|&(r, s, d)| {
                            r == next_r && d == next_dir && (s - next_s).abs() < MIN_OVERLAP_M
                        });
                        if let Some(ei) = existing {
                            leg_bounds[ei] = [
                                leg_bounds[ei][0].min(dst_bounds[0]),
                                leg_bounds[ei][1].max(dst_bounds[1]),
                            ];
                            adj_edges.push((li, ei, sp.avg_sep));
                        } else {
                            let ni = legs_mid.len();
                            legs_mid.push((next_r, next_s, next_dir));
                            leg_bounds.push(dst_bounds);
                            adj_edges.push((li, ni, sp.avg_sep));
                            queue.push_back(ni);
                        }
                    }
                }
                let keep = filter_competing_names(network, runs, &legs_mid, &adj_edges);
                let p0 = runs[r_ref].point_at(network, w0);
                let p1 = runs[r_ref].point_at(network, w1);
                let half_w = 0.5 * (w1 - w0);
                let mut sec_legs = Vec::with_capacity(legs_mid.len());
                let mut orig_indices = Vec::with_capacity(legs_mid.len());
                for (idx, &(r_m, s_mid, same_dir)) in legs_mid.iter().enumerate() {
                    if !keep[idx] {
                        continue;
                    }
                    let Some((unc_lo, unc_hi)) = uncovered_containing(&uncovered[r_m], s_mid)
                    else {
                        continue;
                    };
                    let (lo, hi) = if idx == 0 {
                        subtract_interval_bounded(
                            &mut uncovered[r_m],
                            w0.max(unc_lo),
                            w1.min(unc_hi),
                            f64::NEG_INFINITY,
                            f64::INFINITY,
                        )
                    } else {
                        let b_lo = unc_lo.max(leg_bounds[idx][0]);
                        let b_hi = unc_hi.min(leg_bounds[idx][1]);
                        if b_hi - b_lo <= 1e-3 {
                            continue;
                        }
                        let (h0, h1) = if same_dir {
                            (
                                (s_mid - half_w).clamp(b_lo, b_hi),
                                (s_mid + half_w).clamp(b_lo, b_hi),
                            )
                        } else {
                            (
                                (s_mid + half_w).clamp(b_lo, b_hi),
                                (s_mid - half_w).clamp(b_lo, b_hi),
                            )
                        };
                        let (_, a) =
                            runs[r_m].project_onto_range(network, p0, b_lo, b_hi, Some(h0));
                        let (_, b) =
                            runs[r_m].project_onto_range(network, p1, b_lo, b_hi, Some(h1));
                        let (lo, hi) = (a.min(b), a.max(b));
                        if hi - lo <= 1e-3 {
                            continue;
                        }
                        let (smin, smax) = if r_m == r_ref {
                            (b_lo, b_hi)
                        } else {
                            (f64::NEG_INFINITY, f64::INFINITY)
                        };
                        subtract_interval_bounded(&mut uncovered[r_m], lo, hi, smin, smax)
                    };
                    sec_legs.push(SectionLeg {
                        run_id: r_m,
                        s0: lo,
                        s1: hi,
                        same_dir,
                        slot: 0,
                        slot_weight: 1.0,
                    });
                    orig_indices.push(idx);
                }
                if sec_legs.is_empty() {
                    continue;
                }
                let mut order: Vec<usize> = (0..sec_legs.len()).collect();
                order[1..].sort_by(|&a, &b| {
                    sec_legs[a]
                        .run_id
                        .cmp(&sec_legs[b].run_id)
                        .then_with(|| sec_legs[a].s0.total_cmp(&sec_legs[b].s0))
                });
                let mut sec_legs: Vec<SectionLeg> =
                    order.iter().map(|&i| sec_legs[i].clone()).collect();
                let orig_indices: Vec<usize> = order.iter().map(|&i| orig_indices[i]).collect();
                let phys_tracks =
                    assign_physical_slots(network, runs, &mut sec_legs, &orig_indices, &adj_edges);
                let merged_into_prev = prev_sec.is_some_and(|pidx| {
                    let prev = &mut sections[pidx];
                    let same = prev.phys_tracks == phys_tracks
                        && prev.legs.len() == sec_legs.len()
                        && prev.legs.iter().zip(&sec_legs).all(|(a, b)| {
                            a.run_id == b.run_id
                                && a.same_dir == b.same_dir
                                && a.slot == b.slot
                                && (a.s1 - b.s0).abs().min((b.s1 - a.s0).abs())
                                    <= SLIVER_SNAP_M + 1e-3
                        });
                    if same {
                        for (a, b) in prev.legs.iter_mut().zip(&sec_legs) {
                            a.s0 = a.s0.min(b.s0);
                            a.s1 = a.s1.max(b.s1);
                        }
                    }
                    same
                });
                if !merged_into_prev {
                    let sec_idx = sections.len();
                    sections.push(Section {
                        legs: sec_legs,
                        phys_tracks,
                    });
                    prev_sec = Some(sec_idx);
                }
            }
        }
    }
    for (sec_idx, sec) in sections.iter().enumerate() {
        for leg in &sec.legs {
            run_sections[leg.run_id].push((leg.s0, leg.s1, sec_idx));
        }
    }
    for list in &mut run_sections {
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    (sections, run_sections)
}
