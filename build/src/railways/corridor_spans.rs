//! Symmetric parallel edge-pair projection and contiguous run-pair overlap spans.
use super::corridor_runs::{
    EdgeGrid, Run, TokenRel, shares_node, to_local_m, tokens_match, usage_family,
};
use super::network::Network;
use rayon::prelude::*;
use std::collections::HashMap;

pub(super) const TOKENLESS_LATERAL_M: f64 = 15.0;
const TOKENLESS_MIN_COS: f64 = 0.984_807_753; // cos(10 deg)
pub(super) const SAME_TOKEN_LATERAL_M: f64 = 50.0;
const SAME_TOKEN_MIN_COS: f64 = 0.939_692_621; // cos(20 deg)
const SAME_WAY_MIN_SEP_M: f64 = 1.5;
pub(super) const MIN_OVERLAP_M: f64 = 30.0;
const SEAM_BRIDGE_M: f64 = 5.0;

#[derive(Clone, Copy)]
struct RawLink {
    run_a: usize,
    s_a: [f64; 2],
    run_b: usize,
    s_b: [f64; 2],
    same_dir: bool,
    sep_m: f64,
    len_m: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Span {
    pub run_a: usize,
    pub s_a: [f64; 2],
    pub run_b: usize,
    pub s_b: [f64; 2],
    pub same_dir: bool,
    pub avg_sep: f64,
}

impl Span {
    pub(super) fn map_from(&self, run_id: usize, s: f64) -> Option<(usize, f64, [f64; 2], bool)> {
        let map = |src: [f64; 2], dst: [f64; 2]| {
            let span = (src[1] - src[0]).max(1e-9);
            let frac = ((s - src[0]) / span).clamp(0.0, 1.0);
            let f = if self.same_dir { frac } else { 1.0 - frac };
            dst[0] + f * (dst[1] - dst[0])
        };
        if self.run_a == run_id && s >= self.s_a[0] - 1e-6 && s <= self.s_a[1] + 1e-6 {
            return Some((self.run_b, map(self.s_a, self.s_b), self.s_b, self.same_dir));
        }
        if self.run_b == run_id && s >= self.s_b[0] - 1e-6 && s <= self.s_b[1] + 1e-6 {
            return Some((self.run_a, map(self.s_b, self.s_a), self.s_a, self.same_dir));
        }
        None
    }
}

fn pair_overlap(
    i: usize,
    j: usize,
    network: &Network,
    runs: &[Run],
    edge_run: &[Option<(usize, usize)>],
) -> Option<RawLink> {
    let (ei, ej) = (&network.edges[i], &network.edges[j]);
    if usage_family(ei.usage) != usage_family(ej.usage) {
        return None;
    }
    let rel = tokens_match(ei, ej);
    if rel == TokenRel::Conflict {
        return None;
    }
    let (ri, pi) = edge_run[i]?;
    let (rj, pj) = edge_run[j]?;
    if (ei.way == ej.way || ri == rj) && shares_node(ei.nodes, ej.nodes) {
        return None;
    }
    let same_token = rel == TokenRel::Same;
    let run_shares = ri != rj
        && match (runs[ri].endpoints, runs[rj].endpoints) {
            (Some(ea), Some(eb)) => shares_node(ea, eb),
            _ => false,
        };
    let max_lat = if same_token {
        SAME_TOKEN_LATERAL_M
    } else {
        TOKENLESS_LATERAL_M
    };
    let min_cos = if !same_token || run_shares {
        TOKENLESS_MIN_COS
    } else {
        SAME_TOKEN_MIN_COS
    };

    let (mi, mj) = (ei.midpoint(), ej.midpoint());
    let origin = [0.5 * (mi[0] + mj[0]), 0.5 * (mi[1] + mj[1])];
    let cos_lat = origin[0].to_radians().cos().max(0.05);
    let (a0, a1) = (
        to_local_m(origin, cos_lat, ei.points[0]),
        to_local_m(origin, cos_lat, ei.points[1]),
    );
    let (b0, b1) = (
        to_local_m(origin, cos_lat, ej.points[0]),
        to_local_m(origin, cos_lat, ej.points[1]),
    );
    let len_a = (a1[0] - a0[0]).hypot(a1[1] - a0[1]);
    let len_b = (b1[0] - b0[0]).hypot(b1[1] - b0[1]);
    if len_a < 1e-3 || len_b < 1e-3 {
        return None;
    }
    let dir_a = [(a1[0] - a0[0]) / len_a, (a1[1] - a0[1]) / len_a];
    let dir_b = [(b1[0] - b0[0]) / len_b, (b1[1] - b0[1]) / len_b];
    let dot = dir_a[0] * dir_b[0] + dir_a[1] * dir_b[1];
    if dot.abs() < min_cos {
        return None;
    }
    let sign = if dot >= 0.0 { 1.0 } else { -1.0 };
    let (ux, uy) = (dir_a[0] + sign * dir_b[0], dir_a[1] + sign * dir_b[1]);
    let ulen = ux.hypot(uy);
    if ulen < 1e-6 {
        return None;
    }
    let u_hat = [ux / ulen, uy / ulen];
    let n_hat = [-u_hat[1], u_hat[0]];
    let proj = |p: [f64; 2]| {
        (
            p[0] * u_hat[0] + p[1] * u_hat[1],
            p[0] * n_hat[0] + p[1] * n_hat[1],
        )
    };
    let ((ua0, na0), (ua1, na1)) = (proj(a0), proj(a1));
    let ((ub0, nb0), (ub1, nb1)) = (proj(b0), proj(b1));
    let u_lo = ua0.min(ua1).max(ub0.min(ub1));
    let u_hi = ua0.max(ua1).min(ub0.max(ub1));
    if u_hi - u_lo <= 1e-3 {
        return None;
    }
    let d_at = |u: f64| {
        let ta = (u - ua0) / (ua1 - ua0);
        let tb = (u - ub0) / (ub1 - ub0);
        (nb0 + tb * (nb1 - nb0)) - (na0 + ta * (na1 - na0))
    };
    let d0 = d_at(u_lo);
    let slope = (d_at(u_hi) - d0) / (u_hi - u_lo);
    let (lo_x, hi_x) = if slope.abs() <= 1e-12 {
        if d0.abs() >= max_lat {
            return None;
        }
        (0.0, u_hi - u_lo)
    } else {
        let x1 = (-max_lat - d0) / slope;
        let x2 = (max_lat - d0) / slope;
        (x1.min(x2).max(0.0), x1.max(x2).min(u_hi - u_lo))
    };
    if hi_x - lo_x <= 1e-3 {
        return None;
    }
    let (u_a, u_b) = (u_lo + lo_x, u_lo + hi_x);
    let avg_sep = 0.5 * (d_at(u_a).abs() + d_at(u_b).abs());
    if (ei.way == ej.way || ri == rj) && avg_sep < SAME_WAY_MIN_SEP_M {
        return None;
    }
    let si0 = runs[ri].s_of_edge_t(pi, (u_a - ua0) / (ua1 - ua0));
    let si1 = runs[ri].s_of_edge_t(pi, (u_b - ua0) / (ua1 - ua0));
    let sj0 = runs[rj].s_of_edge_t(pj, (u_a - ub0) / (ub1 - ub0));
    let sj1 = runs[rj].s_of_edge_t(pj, (u_b - ub0) / (ub1 - ub0));
    let same_dir = (si1 >= si0) == (sj1 >= sj0);
    let (sa, sb) = ([si0.min(si1), si0.max(si1)], [sj0.min(sj1), sj0.max(sj1)]);
    let swap = ri > rj || (ri == rj && sa[0] > sb[0]);
    let (run_a, s_a, run_b, s_b) = if swap {
        (rj, sb, ri, sa)
    } else {
        (ri, sa, rj, sb)
    };
    Some(RawLink {
        run_a,
        s_a,
        run_b,
        s_b,
        same_dir,
        sep_m: avg_sep,
        len_m: u_b - u_a,
    })
}

pub(super) fn build_spans(
    network: &Network,
    runs: &[Run],
    edge_run: &[Option<(usize, usize)>],
    grid: &EdgeGrid,
) -> Vec<Span> {
    let links: Vec<RawLink> = (0..network.edges.len())
        .into_par_iter()
        .flat_map(|i| {
            if edge_run[i].is_none() {
                return Vec::new();
            }
            let edge = &network.edges[i];
            let radius = if edge.corridor.is_empty() {
                TOKENLESS_LATERAL_M
            } else {
                SAME_TOKEN_LATERAL_M
            };
            grid.candidates_for_edge(edge, i, radius)
                .into_iter()
                .filter_map(|j| pair_overlap(i, j, network, runs, edge_run))
                .collect()
        })
        .collect();
    let mut grouped: HashMap<(usize, usize, bool), Vec<RawLink>> = HashMap::new();
    for link in links {
        grouped
            .entry((link.run_a, link.run_b, link.same_dir))
            .or_default()
            .push(link);
    }
    let mut spans = Vec::new();
    for ((run_a, run_b, same_dir), mut list) in grouped {
        list.sort_by(|a, b| a.s_a[0].total_cmp(&b.s_a[0]));
        let mut merged: Vec<(Span, f64, f64)> = Vec::new();
        for link in list {
            let can_merge = merged.last().is_some_and(|(cur, _, _)| {
                let a_close = link.s_a[0] <= cur.s_a[1] + SEAM_BRIDGE_M;
                let b_close = link.s_b[0] <= cur.s_b[1] + SEAM_BRIDGE_M
                    && cur.s_b[0] <= link.s_b[1] + SEAM_BRIDGE_M;
                let disjoint =
                    run_a != run_b || cur.s_a[1].max(link.s_a[1]) <= cur.s_b[0].min(link.s_b[0]);
                a_close && b_close && disjoint
            });
            if can_merge {
                let (cur, sep_sum, len_sum) = merged.last_mut().unwrap();
                cur.s_a = [cur.s_a[0].min(link.s_a[0]), cur.s_a[1].max(link.s_a[1])];
                cur.s_b = [cur.s_b[0].min(link.s_b[0]), cur.s_b[1].max(link.s_b[1])];
                *sep_sum += link.sep_m * link.len_m;
                *len_sum += link.len_m;
                cur.avg_sep = *sep_sum / (*len_sum).max(1e-6);
            } else {
                merged.push((
                    Span {
                        run_a,
                        s_a: link.s_a,
                        run_b,
                        s_b: link.s_b,
                        same_dir,
                        avg_sep: link.sep_m,
                    },
                    link.sep_m * link.len_m,
                    link.len_m,
                ));
            }
        }
        let bar = MIN_OVERLAP_M.min(0.8 * runs[run_a].total_len.min(runs[run_b].total_len));
        // A closed loop's parallel span can cross the 0/total_len origin on either s_a or s_b.
        let mut keep_seam = vec![false; merged.len()];
        if run_a == run_b && runs[run_a].closed && merged.len() >= 2 {
            let total = runs[run_a].total_len;
            let span_len = |sp: &Span| (sp.s_a[1] - sp.s_a[0]).min(sp.s_b[1] - sp.s_b[0]);
            for i in 0..merged.len() {
                let a = &merged[i].0;
                if a.s_a[0] > SEAM_BRIDGE_M {
                    continue;
                }
                for j in 0..merged.len() {
                    if i == j {
                        continue;
                    }
                    let b = &merged[j].0;
                    let wraps_a = b.s_a[1] >= total - SEAM_BRIDGE_M
                        && ((a.s_b[0] - b.s_b[1]).abs() <= SEAM_BRIDGE_M
                            || (a.s_b[1] - b.s_b[0]).abs() <= SEAM_BRIDGE_M);
                    let wraps_b = b.s_b[1] >= total - SEAM_BRIDGE_M
                        && ((a.s_b[0] - b.s_a[1]).abs() <= SEAM_BRIDGE_M
                            || (a.s_b[1] - b.s_a[0]).abs() <= SEAM_BRIDGE_M);
                    if (wraps_a || wraps_b) && span_len(a) + span_len(b) >= bar {
                        keep_seam[i] = true;
                        keep_seam[j] = true;
                    }
                }
            }
        }
        for (idx, (sp, _, _)) in merged.into_iter().enumerate() {
            let len = (sp.s_a[1] - sp.s_a[0]).min(sp.s_b[1] - sp.s_b[0]);
            if len >= bar || keep_seam[idx] {
                spans.push(sp);
            }
        }
    }
    spans.sort_by(|a, b| {
        (a.run_a, a.run_b, a.same_dir)
            .cmp(&(b.run_a, b.run_b, b.same_dir))
            .then_with(|| a.s_a[0].total_cmp(&b.s_a[0]))
    });
    spans
}
