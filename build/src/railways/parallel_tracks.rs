//! Allocate category evidence once per corridor and share it among its tracks.
use super::encode::Expanded;
use super::merge::{CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_KNOWN, STATUS_UNKNOWN};
use super::model::RailModel;
use super::parallel_geometry::{cross_sections, project_tracks};
use super::types::CrossingFreight;
fn scope_winner(
    model: &RailModel,
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
            || !model.source_applies_to_row(flow.source_id, own_iso)
            || (line_ranked && model.is_residual(flow.source_id))
        {
            continue;
        }
        if winner != 0 && model.blocks_foreign_national(winner, flow.source_id) {
            continue;
        }
        if model.should_overwrite(winner, flow.source_id) {
            winner = flow.source_id;
        }
    }
    winner
}

fn crossing_scope_value(
    members: &[usize],
    carriers: &[CategoryFlow],
    own_flow: CategoryFlow,
    winner: u16,
) -> CategoryFlow {
    // When global cross-section sharing already stamped every track in the section,
    // keep this track's own globally-balanced claim directly to avoid double-averaging
    // across mismatched segment splits. Otherwise divide carrier sums across members.
    if carriers.len() == members.len()
        && own_flow.status != STATUS_UNKNOWN
        && own_flow.source_id == winner
        && own_flow.matched_tracks > 0
    {
        return own_flow;
    }
    let denom = members.len() as f64;
    let mut periods = [0.0; 3];
    let mut pax_sum = 0.0;
    let mut pax_all_known = true;
    let mut measured = true;
    let mut has_freight = false;
    for flow in carriers {
        for (total, value) in periods.iter_mut().zip(flow.periods) {
            *total += value / denom;
        }
        if let Some(cf) = flow.crossing_freight {
            has_freight = true;
            measured = measured && cf.measured_periods;
            match cf.inventory_passenger {
                Some(pax) => pax_sum += pax / denom,
                None => pax_all_known = false,
            }
        }
    }
    CategoryFlow {
        periods,
        missing_calendar_estimate: [0.0; 3],
        status: if carriers.iter().any(|flow| flow.status == STATUS_KNOWN) {
            STATUS_KNOWN
        } else {
            STATUS_ESTIMATED
        },
        source_id: winner,
        matching: carriers.iter().fold(0, |mask, flow| mask | flow.matching),
        stop: 0,
        matched_tracks: 1,
        crossing_freight: has_freight.then_some(CrossingFreight {
            inventory_passenger: pax_all_known.then_some(pax_sum),
            measured_periods: measured,
        }),
    }
}

fn scope_value(
    model: &RailModel,
    members: &[usize],
    flows: &[RowTraffic],
    own: usize,
    winner: u16,
    category: fn(&RowTraffic) -> CategoryFlow,
) -> CategoryFlow {
    // A stop's departures stamp every track near it (`local_match`): the line carries each stop's
    // once, both directions where they share one, each direction's where its platform is its own
    // stop. Summed once a track, a line counted every train once a track (Stockholm's Alvik, three
    // lines at 1,430 a track each, audit 2026-11-12; New York's three-track lines three times).
    let mut stops = std::collections::HashSet::new();
    let carriers: Vec<CategoryFlow> = members
        .iter()
        .map(|&member| category(&flows[member]))
        .filter(|flow| flow.status != STATUS_UNKNOWN && flow.source_id == winner)
        .filter(|flow| flow.stop == 0 || stops.insert(flow.stop))
        .collect();
    if carriers.iter().any(|flow| flow.matched_tracks > 0) {
        return crossing_scope_value(members, &carriers, category(&flows[own]), winner);
    }
    let divisor = if model.stamps_whole_line(winner) {
        carriers.len() as f64
    } else {
        1.0
    };
    let mut periods = [0.0; 3];
    let mut missing_calendar_estimate = [0.0; 3];
    for flow in &carriers {
        for (total, value) in periods.iter_mut().zip(flow.periods) {
            *total += value / divisor;
        }
        for (total, value) in missing_calendar_estimate
            .iter_mut()
            .zip(flow.missing_calendar_estimate)
        {
            *total += value / divisor;
        }
    }
    CategoryFlow {
        periods,
        missing_calendar_estimate,
        status: if carriers.iter().any(|flow| flow.status == STATUS_KNOWN) {
            STATUS_KNOWN
        } else {
            STATUS_ESTIMATED
        },
        source_id: winner,
        matching: carriers.iter().fold(0, |mask, flow| mask | flow.matching),
        stop: 0,
        matched_tracks: 0,
        crossing_freight: None,
    }
}

fn line_prior(
    members: &[usize],
    priors: &[RowTraffic],
    category: fn(&RowTraffic) -> CategoryFlow,
) -> CategoryFlow {
    members
        .iter()
        .map(|&member| category(&priors[member]))
        .fold(CategoryFlow::default(), |best, flow| {
            if best.status == STATUS_UNKNOWN || daily_total(&flow) > daily_total(&best) {
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

struct TrackEvidence<'a> {
    model: &'a RailModel,
    domestic: &'a [RowTraffic],
    foreign: &'a [RowTraffic],
    priors: &'a [RowTraffic],
    modes: &'a [u8],
}

fn track_share(
    evidence: &TrackEvidence<'_>,
    own_iso: [u8; 2],
    members: &[usize],
    category: fn(&RowTraffic) -> CategoryFlow,
    own: usize,
    allows: fn(u8) -> bool,
) -> CategoryFlow {
    let TrackEvidence {
        model,
        domestic,
        foreign,
        priors,
        modes,
    } = *evidence;
    let tracks = members.len() as f64;
    // Ranked evidence from either timetable marks the line active; a no-service stamp beside
    // it is a walk gap on that piece, not silence.
    let line_ranked = members.iter().any(|&member| {
        [
            domestic[member].passenger,
            domestic[member].freight,
            foreign[member].passenger,
            foreign[member].freight,
        ]
        .iter()
        .any(|flow| flow.status != STATUS_UNKNOWN && !model.is_residual(flow.source_id))
    });
    let capable: Vec<usize> = members
        .iter()
        .copied()
        .filter(|&member| allows(modes[member]))
        .collect();
    let prior = line_prior(&capable, priors, category);
    let winner = scope_winner(model, own_iso, members, domestic, line_ranked, category);
    let (line, divisor, from_prior) = if winner != 0 && !model.is_residual(winner) {
        let value = scope_value(model, members, domestic, own, winner, category);
        if value.matched_tracks > 0 {
            // Crossing observations are already normalised to per-track rates.
            (value, 1.0, false)
        } else if model.stamps_whole_line(winner) {
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
    } else if model.is_residual(winner) {
        (
            scope_value(model, members, domestic, own, winner, category),
            tracks,
            false,
        )
    } else {
        let abroad = scope_winner(model, own_iso, members, foreign, line_ranked, category);
        let value = if abroad == 0 {
            CategoryFlow::default()
        } else {
            scope_value(model, members, foreign, own, abroad, category)
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
            missing_calendar_estimate: [0.0; 3],
            status: STATUS_ESTIMATED,
            source_id: 0,
            matching: 0,
            stop: 0,
            matched_tracks: 0,
            crossing_freight: None,
        };
    }
    CategoryFlow {
        periods: line.periods.map(|value| value / divisor),
        missing_calendar_estimate: line.missing_calendar_estimate.map(|value| value / divisor),
        ..line
    }
}

/// Subtract per-period passenger trains from a crossing's measured END totals,
/// conserving `target_daily` without creating phantom daytime or night freight.
pub(super) fn subtract_passenger_periods(
    total: [f64; 3],
    passenger: [f64; 3],
    target_daily: f64,
) -> [f64; 3] {
    if target_daily <= 0.0 {
        return [0.0; 3];
    }
    let raw = [
        (total[0] - passenger[0]).max(0.0),
        (total[1] - passenger[1]).max(0.0),
        (total[2] - passenger[2]).max(0.0),
    ];
    let raw_sum: f64 = raw.iter().sum();
    if raw_sum <= 1e-12 {
        return [0.0; 3];
    }
    let scale = target_daily / raw_sum;
    [raw[0] * scale, raw[1] * scale, raw[2] * scale]
}

fn prior_freight_share(
    evidence: &TrackEvidence<'_>,
    members: &[usize],
    own: usize,
) -> CategoryFlow {
    if !carries_freight(evidence.modes[own]) {
        return CategoryFlow {
            periods: [0.0; 3],
            missing_calendar_estimate: [0.0; 3],
            status: STATUS_ESTIMATED,
            source_id: 0,
            matching: 0,
            stop: 0,
            matched_tracks: 0,
            crossing_freight: None,
        };
    }
    let capable: Vec<usize> = members
        .iter()
        .copied()
        .filter(|&member| carries_freight(evidence.modes[member]))
        .collect();
    let prior = line_prior(&capable, evidence.priors, |t| t.freight);
    let divisor = capable.len().max(1) as f64;
    CategoryFlow {
        periods: prior.periods.map(|value| value / divisor),
        missing_calendar_estimate: prior.missing_calendar_estimate.map(|value| value / divisor),
        ..prior
    }
}

pub(crate) fn allocate_over_parallel_tracks(
    model: &RailModel,
    rows: &mut [Expanded],
    square: crate::osm::geometry::Square,
) {
    let tracks = project_tracks(rows, square);
    let sections = cross_sections(rows, &tracks);
    let domestic: Vec<RowTraffic> = rows.iter().map(|row| row.child.traffic).collect();
    let foreign: Vec<RowTraffic> = rows.iter().map(|row| row.child.foreign).collect();
    let priors: Vec<RowTraffic> = rows.iter().map(|row| row.prior).collect();
    let modes: Vec<u8> = rows.iter().map(|row| row.traffic_mode).collect();
    let evidence = TrackEvidence {
        model,
        domestic: &domestic,
        foreign: &foreign,
        priors: &priors,
        modes: &modes,
    };
    for (index, row) in rows.iter_mut().enumerate() {
        if row.service != 0 {
            continue;
        }
        let members = &sections[index];
        let mut passenger = track_share(
            &evidence,
            row.country_iso,
            members,
            |t| t.passenger,
            index,
            carries_passengers,
        );
        let mut freight = track_share(
            &evidence,
            row.country_iso,
            members,
            |t| t.freight,
            index,
            carries_freight,
        );
        if let Some(cf) = freight.crossing_freight {
            let shares = model.period_shares(row.country_iso, row.rail_type);
            if passenger.source_id != 0 {
                let total_daily = daily_total(&freight);
                let pax_daily = daily_total(&passenger);
                let effective_pax = cf
                    .inventory_passenger
                    .map_or(pax_daily, |inv| pax_daily.max(inv));
                let target_daily = (total_daily - effective_pax).max(0.0);
                freight.periods = if cf.measured_periods {
                    subtract_passenger_periods(freight.periods, passenger.periods, target_daily)
                } else {
                    shares.frt.map(|share| target_daily * share)
                };
                freight.crossing_freight = None;
            } else if let Some(inv_pax) = cf.inventory_passenger {
                // Passenger fell back to source 0 (e.g. a below-prior timetable on an
                // incomplete cross-section), while the crossing inventory provides a
                // complete passenger/freight split for the corridor: keep both.
                let total_daily = daily_total(&freight);
                let pax_periods = if cf.measured_periods
                    && total_daily > 0.0
                    && (inv_pax - total_daily).abs() <= 1e-6
                {
                    freight.periods
                } else {
                    shares.pax.map(|share| inv_pax * share)
                };
                let target_daily = (total_daily - inv_pax).max(0.0);
                freight.periods = if cf.measured_periods {
                    subtract_passenger_periods(freight.periods, pax_periods, target_daily)
                } else {
                    shares.frt.map(|share| target_daily * share)
                };
                freight.crossing_freight = None;
                passenger = CategoryFlow {
                    periods: pax_periods,
                    missing_calendar_estimate: [0.0; 3],
                    status: freight.status,
                    source_id: freight.source_id,
                    matching: freight.matching,
                    stop: 0,
                    matched_tracks: freight.matched_tracks,
                    crossing_freight: None,
                };
            } else {
                // Unknown passenger split without a timetable: keep the class freight prior
                // rather than subtracting a guessed passenger prior or stamping all-freight.
                freight = prior_freight_share(&evidence, members, index);
            }
        }
        row.child.traffic = RowTraffic { passenger, freight };
    }
}

#[cfg(test)]
#[path = "allocation_tests.rs"]
mod allocation_tests;
