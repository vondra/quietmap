//! Match crossing through-train counts to physical corridor sections and aggregate per section/run.
use super::{
    corridor::CorridorLayout,
    crossing_resolve::{AggregatedFlow, resolve_section},
    matcher::Claims,
    model::RailModel,
    network::Network,
    square_intervals::Interval,
    types::Crossing,
};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

#[cfg(test)]
use super::crossing_resolve::allocate_passenger_in_periods;
#[cfg(test)]
use super::merge::{STATUS_ESTIMATED, STATUS_UNKNOWN};

#[derive(Clone, Copy)]
struct CrossingObs<'a> {
    crossing_id: &'a str,
    measured_periods: [f64; 3],
    unsplit_daily: f64,
    total: f64,
    passenger: Option<f64>,
    passenger_cap: Option<f64>,
    has_measured: bool,
    obs_tracks: usize,
}

impl CrossingObs<'_> {
    fn has_split_info(&self) -> bool {
        self.passenger.is_some() || self.passenger_cap.is_some()
    }

    /// Numeric passenger key for sorting equal-total ties (`None` < `0` < `LT1` < `1` < ...).
    fn passenger_order(&self) -> f64 {
        if let Some(p) = self.passenger {
            p
        } else if let Some(cap) = self.passenger_cap {
            0.5 * cap
        } else {
            -1.0
        }
    }
}

fn aggregate_obs(mut obs: Vec<CrossingObs<'_>>) -> AggregatedFlow {
    if obs.iter().any(|item| item.total > 0.0) {
        obs.retain(|item| item.total > 0.0);
    }
    obs.sort_by(|a, b| a.total.total_cmp(&b.total));
    let chosen_total = obs[obs.len() / 2].total;
    if chosen_total <= 0.0 {
        let first = obs
            .iter()
            .copied()
            .min_by(|a, b| a.crossing_id.cmp(b.crossing_id))
            .unwrap_or(obs[0]);
        return AggregatedFlow {
            measured_periods: [0.0; 3],
            unsplit_daily: 0.0,
            total: 0.0,
            measured_passenger: Some(0.0),
            passenger_cap: None,
            obs_tracks: first.obs_tracks,
        };
    }
    // Resolve the passenger split within the equal-total subset so higher/lower totals do not push
    // a minority passenger count onto the overall median boundary.
    let tied: Vec<CrossingObs<'_>> = obs
        .into_iter()
        .filter(|item| (item.total - chosen_total).abs() <= 1e-6)
        .collect();
    let mut split_pool: Vec<CrossingObs<'_>> = tied
        .iter()
        .copied()
        .filter(CrossingObs::has_split_info)
        .collect();
    if split_pool.is_empty() {
        split_pool = tied.clone();
    }
    split_pool.sort_by(|a, b| {
        a.passenger_order()
            .total_cmp(&b.passenger_order())
            // Sort unsplit weekly observations first so measured period splits win ties.
            .then_with(|| a.has_measured.cmp(&b.has_measured))
            .then_with(|| a.crossing_id.cmp(b.crossing_id))
    });
    let chosen = split_pool[split_pool.len() / 2];
    let timing = if chosen.has_measured {
        chosen
    } else {
        tied.iter()
            .copied()
            .filter(|item| item.has_measured)
            .min_by(|a, b| a.crossing_id.cmp(b.crossing_id))
            .unwrap_or(chosen)
    };
    AggregatedFlow {
        measured_periods: timing.measured_periods,
        unsplit_daily: timing.unsplit_daily,
        total: chosen.total,
        measured_passenger: chosen.passenger,
        passenger_cap: chosen.passenger_cap,
        obs_tracks: chosen.obs_tracks,
    }
}

/// A run that exists only inside `sec_idx`, connects to no external running tracks at either end,
/// and is either switch-connected to `sec_idx` at both ends or strictly interior to a continuing
/// leg of `sec_idx` is a bounded passing siding; its traffic returns to the continuing tracks.
fn is_bounded_siding(
    run_id: usize,
    sec_idx: usize,
    layout: &CorridorLayout,
    network: &Network,
) -> bool {
    if layout.run_sections[run_id].len() != 1 {
        return false;
    }
    let Some(ends) = layout.runs[run_id].endpoints else {
        return false;
    };
    let sec_legs = &layout.sections[sec_idx].legs;
    let no_external = ends.into_iter().all(|node| {
        network.adjacency[node].iter().all(|&edge_idx| {
            layout.edge_run[edge_idx]
                .is_none_or(|(other_run, _)| sec_legs.iter().any(|leg| leg.run_id == other_run))
        })
    });
    if !no_external {
        return false;
    }
    let switched_to_main = ends.into_iter().all(|node| {
        network.adjacency[node].iter().any(|&edge_idx| {
            layout.edge_run[edge_idx].is_some_and(|(other_run, _)| {
                other_run != run_id && sec_legs.iter().any(|leg| leg.run_id == other_run)
            })
        })
    });
    let interior_to_continuing = sec_legs.iter().any(|leg| {
        leg.run_id != run_id && leg.s0 > 1.0 && leg.s1 < layout.runs[leg.run_id].total_len - 1.0
    });
    switched_to_main || interior_to_continuing
}

type PieceKey = ([u16; 2], i64, i16);

fn index_piece_claims(existing_claims: &Claims) -> HashMap<PieceKey, Vec<&Interval>> {
    let mut grouped: HashMap<PieceKey, Vec<(&Interval, i64)>> = HashMap::new();
    for (&square, rows) in existing_claims {
        for (iv, occ) in rows {
            grouped
                .entry((square, iv.osm_id, iv.segment_idx))
                .or_default()
                .push((iv, *occ));
        }
    }
    grouped
        .into_iter()
        .map(|(key, mut list)| {
            list.sort_by(|(a, ao), (b, bo)| {
                (a.source_id, a.country)
                    .cmp(&(b.source_id, b.country))
                    .then_with(|| a.from_m.total_cmp(&b.from_m))
                    .then_with(|| a.to_m.total_cmp(&b.to_m))
                    .then_with(|| ao.cmp(bo))
            });
            (key, list.into_iter().map(|(iv, _)| iv).collect())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn match_crossings(
    model: &RailModel,
    network: &Network,
    crossings: &[Crossing],
    source_id: u16,
    country: [u8; 2],
    squares: &HashSet<[u16; 2]>,
    existing_claims: &Claims,
    yard_keys: &HashSet<([u16; 2], i64, i16)>,
) -> Vec<([u16; 2], Interval)> {
    let layout = CorridorLayout::build(network, country, yard_keys);
    let mut direct_obs: Vec<Vec<CrossingObs<'_>>> = vec![Vec::new(); layout.sections.len()];
    let mut per_track_obs: Vec<Vec<CrossingObs<'_>>> = vec![Vec::new(); layout.runs.len()];
    let mut section_prop: Vec<Option<(Vec<usize>, usize)>> = vec![None; layout.sections.len()];
    let mut matched_crossings = 0usize;
    let mut unmatched_crossings = 0usize;

    for crossing in crossings {
        let Some(total_periods) = crossing.total_periods else {
            continue;
        };
        let Some((anchor_edge, frac)) = layout
            .grid
            .nearest_edge([crossing.lat, crossing.lon], network)
        else {
            unmatched_crossings += 1;
            continue;
        };
        let Some((run_id, pos)) = layout.edge_run[anchor_edge] else {
            unmatched_crossings += 1;
            continue;
        };
        let s = layout.runs[run_id].s_of_edge_t(pos, frac);
        let Some(sec_idx) = layout.section_at(run_id, s) else {
            unmatched_crossings += 1;
            continue;
        };
        matched_crossings += 1;
        let total: f64 = total_periods.iter().sum();
        let measured_periods = if crossing.measured_periods {
            total_periods
        } else {
            [0.0; 3]
        };
        let unsplit_daily = if crossing.measured_periods {
            0.0
        } else {
            total
        };
        let cap = crossing.passenger_lt1.then_some(1.0);
        let section = &layout.sections[sec_idx];
        let k_phys = section.phys_tracks.max(1);
        direct_obs[sec_idx].push(CrossingObs {
            crossing_id: &crossing.id,
            measured_periods,
            unsplit_daily,
            total,
            passenger: crossing.passenger_daily,
            passenger_cap: cap,
            has_measured: crossing.measured_periods,
            obs_tracks: k_phys,
        });
        let (prop_runs, prop_tracks) = section_prop[sec_idx].get_or_insert_with(|| {
            let mut seen_runs: Vec<usize> = Vec::with_capacity(section.legs.len());
            for leg in &section.legs {
                if !seen_runs.contains(&leg.run_id) {
                    seen_runs.push(leg.run_id);
                }
            }
            let bounded: Vec<bool> = seen_runs
                .iter()
                .map(|&rid| {
                    seen_runs.len() > 1 && is_bounded_siding(rid, sec_idx, &layout, network)
                })
                .collect();
            let cont_phys_f: f64 = if seen_runs.len() > 1 {
                section
                    .legs
                    .iter()
                    .filter(|leg| {
                        let pos = seen_runs.iter().position(|&r| r == leg.run_id).unwrap();
                        !bounded[pos]
                    })
                    .map(|leg| leg.slot_weight)
                    .sum()
            } else {
                0.0
            };
            let cont_phys = cont_phys_f.round() as usize;
            if cont_phys > 0 {
                let runs = seen_runs
                    .into_iter()
                    .zip(bounded)
                    .filter_map(|(rid, is_b)| (!is_b).then_some(rid))
                    .collect();
                (runs, cont_phys)
            } else {
                (seen_runs, k_phys)
            }
        });
        let k = *prop_tracks as f64;
        for &rid in prop_runs.iter() {
            per_track_obs[rid].push(CrossingObs {
                crossing_id: &crossing.id,
                measured_periods: measured_periods.map(|v| v / k),
                unsplit_daily: unsplit_daily / k,
                total: total / k,
                passenger: crossing.passenger_daily.map(|p| p / k),
                passenger_cap: cap.map(|c| c / k),
                has_measured: crossing.measured_periods,
                obs_tracks: *prop_tracks,
            });
        }
    }

    let direct_resolved: Vec<Option<AggregatedFlow>> = direct_obs
        .into_iter()
        .map(|obs| (!obs.is_empty()).then(|| aggregate_obs(obs)))
        .collect();
    let run_per_track: Vec<Option<AggregatedFlow>> = per_track_obs
        .into_iter()
        .map(|obs| (!obs.is_empty()).then(|| aggregate_obs(obs)))
        .collect();
    let active_runs = run_per_track.iter().filter(|r| r.is_some()).count();

    let piece_claims = index_piece_claims(existing_claims);
    let claims: Vec<([u16; 2], Interval)> = layout
        .sections
        .par_iter()
        .enumerate()
        .flat_map(|(sec_idx, section)| {
            let carriers: Vec<AggregatedFlow> = if let Some(direct) = direct_resolved[sec_idx] {
                vec![direct]
            } else {
                let raw: Vec<(AggregatedFlow, f64)> = section
                    .legs
                    .iter()
                    .filter_map(|leg| {
                        run_per_track[leg.run_id]
                            .map(|c| (c.scaled(leg.slot_weight), leg.slot_weight))
                    })
                    .collect();
                if raw.is_empty() {
                    Vec::new()
                } else {
                    // When a multi-track corridor loses some track observations across a switch
                    // while keeping the same corridor width, scale up to `min(k_phys, max_obs)`
                    // so the unbroken sibling does not halve the corridor.
                    let raw_phys: f64 = raw.iter().map(|(_, w)| *w).sum();
                    let max_obs = raw.iter().map(|(c, _)| c.obs_tracks).max().unwrap_or(1) as f64;
                    let k_known = (section.phys_tracks.max(1) as f64)
                        .min(max_obs)
                        .max(raw_phys);
                    let scale = k_known / raw_phys;
                    if (scale - 1.0).abs() > 1e-9 {
                        raw.into_iter().map(|(c, _)| c.scaled(scale)).collect()
                    } else {
                        raw.into_iter().map(|(c, _)| c).collect()
                    }
                }
            };
            resolve_section(
                section,
                &carriers,
                model,
                network,
                &layout.runs,
                &piece_claims,
                source_id,
                country,
                squares,
            )
        })
        .collect();

    eprintln!(
        "railways: crossings source {source_id}: {matched_crossings} matched, {unmatched_crossings} unmatched, {active_runs} runs, {} sections, {} edge claims",
        layout.sections.len(),
        claims.len()
    );
    claims
}

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod tests;
