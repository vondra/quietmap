//! Resolve each canonical-reference crossing section once per timetable window and fan out claims.
use super::{
    corridor::{Section, SectionLeg},
    corridor_runs::Run,
    merge::{CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_UNKNOWN, row_evidence},
    model::RailModel,
    network::Network,
    parallel_tracks::{
        carries_freight, carries_passengers, scope_value, scope_winner, subtract_passenger_periods,
    },
    square_intervals::Interval,
};
use std::collections::{HashMap, HashSet};

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

/// Distribute `pax_daily` across END periods respecting `0 <= pax[p] <= cap[p]`, weighted by `weights`.
pub(super) fn allocate_passenger_in_periods(
    cap: [f64; 3],
    pax_daily: f64,
    weights: [f64; 3],
) -> [f64; 3] {
    let mut remaining = pax_daily.clamp(0.0, cap.iter().sum());
    let mut out = [0.0; 3];
    for _ in 0..3 {
        let rooms = [0, 1, 2].map(|p| (cap[p] - out[p]).max(0.0));
        let headroom: f64 = rooms.iter().copied().filter(|&r| r > 1e-12).sum();
        if remaining <= 1e-12 || headroom <= 1e-12 {
            break;
        }
        let active_w: f64 = (0..3)
            .filter(|&p| rooms[p] > 1e-12)
            .map(|p| weights[p])
            .sum();
        let (mut spilled, mut distributed) = (0.0, 0.0);
        for p in 0..3 {
            if rooms[p] <= 1e-12 {
                continue;
            }
            let share = if active_w > 0.0 {
                weights[p] / active_w
            } else {
                rooms[p] / headroom
            };
            let want = remaining * share;
            let add = want.min(rooms[p]);
            out[p] += add;
            distributed += add;
            spilled += (want - rooms[p]).max(0.0);
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

#[allow(clippy::too_many_arguments)]
fn winning_category(
    model: &RailModel,
    country: [u8; 2],
    source_id: u16,
    idx_vec: &[usize],
    domestic: &[RowTraffic],
    foreign: &[RowTraffic],
    is_passenger: bool,
    exclude_self: bool,
    get: fn(&RowTraffic) -> CategoryFlow,
) -> Option<CategoryFlow> {
    let ok = |src: u16, is_foreign: bool| {
        src != 0
            && (!exclude_self || src != source_id)
            && !model.is_residual(src)
            && (!is_foreign || !model.blocks_foreign_national(source_id, src))
            && model.should_overwrite(source_id, src)
    };
    let dom = scope_winner(model, country, idx_vec, domestic, is_passenger, get);
    if ok(dom, false) {
        return Some(scope_value(model, idx_vec, domestic, dom, get));
    }
    let for_ = scope_winner(model, country, idx_vec, foreign, is_passenger, get);
    ok(for_, true).then(|| scope_value(model, idx_vec, foreign, for_, get))
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
    if e_hi - e_lo > 1e-4
        && ivs
            .iter()
            .any(|iv| (iv.from_m - p).abs() < 1e-4 || (iv.to_m - p).abs() < 1e-4)
    {
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

fn scale_leg_flow(
    flow: CategoryFlow,
    fallback_source: u16,
    eligible: bool,
    eligible_phys: f64,
    slot_weight: f64,
    phys_tracks: f64,
) -> CategoryFlow {
    if eligible_phys > 0.0 && !eligible {
        return CategoryFlow {
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
        };
    }
    let factor = slot_weight
        / if eligible_phys > 0.0 {
            eligible_phys
        } else {
            phys_tracks
        };
    CategoryFlow {
        periods: flow.periods.map(|v| v * factor),
        missing_calendar_estimate: flow.missing_calendar_estimate.map(|v| v * factor),
        ..flow
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
) -> Vec<Vec<f64>> {
    let ref_leg = &section.legs[0];
    let mut u_entries: Vec<(f64, Option<(usize, f64)>)> =
        vec![(ref_leg.s0, None), (ref_leg.s1, None)];
    let mut push_cut = |leg_idx: usize, leg: &SectionLeg, s: f64| {
        let u = if leg_idx == 0 {
            s
        } else {
            project_leg_s(network, runs, leg, s, ref_leg)
        };
        u_entries.push((u, Some((leg_idx, s))));
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
            if let Some(ivs) = piece_claims.get(&(edge.square, edge.way, edge.segment)) {
                for iv in ivs {
                    for m in [iv.from_m, iv.to_m] {
                        push_cut(
                            leg_idx,
                            leg,
                            run.edge_extent_to_s(pos, edge, m).clamp(leg.s0, leg.s1),
                        );
                    }
                }
            }
        }
    }
    u_entries.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts: Vec<(f64, Vec<(usize, f64)>)> = Vec::with_capacity(u_entries.len());
    for (u, anchor) in u_entries {
        if let Some((prev_u, anchors)) = cuts.last_mut()
            && u - *prev_u <= 1e-4
            && anchor.is_none_or(|(li, os)| {
                anchors
                    .iter()
                    .all(|&(ali, aos)| ali != li || (aos - os).abs() <= 1e-4)
            })
        {
            if let Some(a) = anchor {
                anchors.push(a);
            }
        } else {
            cuts.push((u, anchor.into_iter().collect()));
        }
    }
    if cuts.last().map_or(ref_leg.s0, |c| c.0) < ref_leg.s1 - 1e-4 {
        cuts.push((ref_leg.s1, Vec::new()));
    }
    if cuts.len() < 2 {
        cuts = vec![(ref_leg.s0, Vec::new()), (ref_leg.s1, Vec::new())];
    }
    section
        .legs
        .iter()
        .enumerate()
        .map(|(leg_idx, leg)| {
            if leg_idx == 0 {
                return cuts.iter().map(|(u, _)| *u).collect();
            }
            let mut pts = Vec::with_capacity(cuts.len());
            pts.push(if leg.same_dir { leg.s0 } else { leg.s1 });
            for (u, anchors) in &cuts[1..cuts.len() - 1] {
                // Keep originating sibling cuts at their exact coordinate instead of round-tripping.
                let raw = anchors.iter().find(|&&(li, _)| li == leg_idx).map_or_else(
                    || project_leg_s(network, runs, ref_leg, *u, leg),
                    |&(_, s)| s,
                );
                let prev = *pts.last().unwrap();
                pts.push(if leg.same_dir {
                    raw.clamp(prev, leg.s1)
                } else {
                    raw.clamp(leg.s0, prev)
                });
            }
            pts.push(if leg.same_dir { leg.s1 } else { leg.s0 });
            pts
        })
        .collect()
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
    let est = |periods: [f64; 3]| CategoryFlow {
        periods,
        missing_calendar_estimate: [0.0; 3],
        status: STATUS_ESTIMATED,
        source_id,
        matching: 2,
        stop: 0,
    };
    let split_sub = |sub: [f64; 3], target: f64, shares: [f64; 3]| {
        if totals.has_measured {
            subtract_passenger_periods(totals.total_periods, sub, target)
        } else {
            shares.map(|s| target * s)
        }
    };
    if let Some(raw_frt) = high_frt {
        let passenger = if let Some(raw_pax) = win_tt {
            raw_pax
        } else if totals.has_explicit_pax {
            let rem = (totals.total_daily - raw_frt.periods.iter().sum::<f64>()).max(0.0);
            let pax_line = totals.known_pax_floor.clamp(0.0, rem);
            let periods = if totals.has_measured {
                let rem_cap =
                    [0, 1, 2].map(|p| (totals.total_periods[p] - raw_frt.periods[p]).max(0.0));
                allocate_passenger_in_periods(rem_cap, pax_line, totals.pax_shares)
            } else {
                totals.pax_shares.map(|s| pax_line * s)
            };
            est(periods)
        } else {
            let mut rem = if pax_eligible {
                (totals.total_daily - raw_frt.periods.iter().sum::<f64>()).max(0.0)
            } else {
                0.0
            };
            if !carriers.is_empty() && carriers.iter().all(|c| c.passenger_cap.is_some()) {
                let cap: f64 = carriers.iter().filter_map(|c| c.passenger_cap).sum();
                if rem >= cap && rem > 0.0 {
                    rem = cap.next_down().max(0.0);
                }
            }
            est(split_sub(raw_frt.periods, rem, totals.pax_shares))
        };
        return RowTraffic {
            passenger,
            freight: raw_frt,
        };
    }
    if let Some(raw_pax) = win_tt {
        let effective_pax = raw_pax
            .periods
            .iter()
            .sum::<f64>()
            .max(totals.known_pax_floor);
        let target_frt = (totals.total_daily - effective_pax).max(0.0);
        return RowTraffic {
            passenger: raw_pax,
            freight: est(split_sub(raw_pax.periods, target_frt, totals.frt_shares)),
        };
    }
    let pax_line: f64 = carriers
        .iter()
        .map(|c| {
            if let Some(p) = c.measured_passenger {
                return p;
            }
            let base = c.passenger_cap.map_or(c.total, |cap| c.total.min(cap));
            let mut val = base * prior_ratio;
            if let Some(cap) = c.passenger_cap
                && val >= cap
                && c.total > 0.0
            {
                val = cap.next_down().max(0.0);
            }
            val.min(c.total)
        })
        .sum::<f64>()
        .clamp(0.0, totals.total_daily);
    let pax_periods = if totals.has_measured {
        allocate_passenger_in_periods(totals.total_periods, pax_line, totals.pax_shares)
    } else {
        totals.pax_shares.map(|s| pax_line * s)
    };
    let frt_periods = if totals.has_measured {
        [0, 1, 2].map(|p| (totals.total_periods[p] - pax_periods[p]).max(0.0))
    } else {
        let target_frt = (totals.total_daily - pax_line).max(0.0);
        totals.frt_shares.map(|s| target_frt * s)
    };
    RowTraffic {
        passenger: est(pax_periods),
        freight: est(frt_periods),
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
    source_id: u16,
    country: [u8; 2],
    squares: &HashSet<[u16; 2]>,
) -> Vec<([u16; 2], Interval)> {
    if carriers.is_empty() || !section_touches_squares(section, runs, network, squares) {
        return Vec::new();
    }
    let phys_tracks = section.phys_tracks.max(1) as f64;
    let shares = model.period_shares(country, 0);
    let (mut measured_line, mut unsplit_line, mut known_pax_floor, mut has_explicit_pax) =
        ([0.0; 3], 0.0, 0.0, false);
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
    let total_periods = [0, 1, 2].map(|p| measured_line[p] + unsplit_line * shares.frt[p]);
    let totals = LineTotals {
        total_periods,
        total_daily: total_periods.iter().sum(),
        has_measured: measured_line.iter().sum::<f64>() > 0.0,
        known_pax_floor,
        has_explicit_pax,
        pax_shares: shares.pax,
        frt_shares: shares.frt,
    };
    let leg_cuts = section_leg_cuts(section, runs, network, piece_claims);
    let idx_vec: Vec<usize> = (0..section.legs.len()).collect();
    let (mut domestic, mut foreign, mut can_pax, mut can_frt) = (
        Vec::with_capacity(section.legs.len()),
        Vec::with_capacity(section.legs.len()),
        Vec::with_capacity(section.legs.len()),
        Vec::with_capacity(section.legs.len()),
    );
    type ResolvedSpan = (f64, f64, RowTraffic);
    let mut per_edge: HashMap<([u16; 2], i64, i16), Vec<ResolvedSpan>> = HashMap::new();

    for w in 0..(leg_cuts[0].len() - 1) {
        domestic.clear();
        foreign.clear();
        can_pax.clear();
        can_frt.clear();
        let (mut pax_prior, mut frt_prior, mut pax_phys, mut frt_phys) =
            (0.0_f64, 0.0_f64, 0.0, 0.0);
        for (leg_idx, leg) in section.legs.iter().enumerate() {
            let s_mid = 0.5 * (leg_cuts[leg_idx][w] + leg_cuts[leg_idx][w + 1]);
            let (dom, for_, edge_idx) =
                probe_leg_evidence(model, network, &runs[leg.run_id], s_mid, piece_claims);
            let e = &network.edges[edge_idx];
            let (allow_pax, allow_frt) = (
                carries_passengers(e.traffic_mode),
                carries_freight(e.traffic_mode),
            );
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
        let win_tt = winning_category(
            model,
            country,
            source_id,
            &idx_vec,
            &domestic,
            &foreign,
            true,
            false,
            |t| t.passenger,
        );
        let high_frt = winning_category(
            model,
            country,
            source_id,
            &idx_vec,
            &domestic,
            &foreign,
            false,
            true,
            |t| t.freight,
        );
        let line_flow = resolve_window_line_traffic(
            carriers,
            &totals,
            prior_ratio,
            pax_phys > 0.0,
            source_id,
            win_tt,
            high_frt,
        );
        for (leg_idx, leg) in section.legs.iter().enumerate() {
            let (s_a, s_b) = (
                leg_cuts[leg_idx][w].min(leg_cuts[leg_idx][w + 1]),
                leg_cuts[leg_idx][w].max(leg_cuts[leg_idx][w + 1]),
            );
            if s_b - s_a <= 1e-4 {
                continue;
            }
            let resolved = RowTraffic {
                passenger: scale_leg_flow(
                    line_flow.passenger,
                    source_id,
                    can_pax[leg_idx],
                    pax_phys,
                    leg.slot_weight,
                    phys_tracks,
                ),
                freight: scale_leg_flow(
                    line_flow.freight,
                    source_id,
                    can_frt[leg_idx],
                    frt_phys,
                    leg.slot_weight,
                    phys_tracks,
                ),
            };
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
        let mut merged: Vec<ResolvedSpan> = Vec::with_capacity(spans.len());
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
