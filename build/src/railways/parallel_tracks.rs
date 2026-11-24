//! Allocate category evidence once per corridor and share it among its tracks.
use super::encode::Expanded;
use super::merge::{CategoryFlow, RowTraffic, STATUS_ESTIMATED, STATUS_KNOWN, STATUS_UNKNOWN};
use super::model::RailModel;
use super::parallel_geometry::{cross_sections, project_tracks};
use tiles::sources::GUESSED_TRAIN_SOURCES;

pub(super) fn scope_winner(
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

pub(super) fn scope_value(
    model: &RailModel,
    members: &[usize],
    flows: &[RowTraffic],
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
    }
}

fn daily_total(flow: &CategoryFlow) -> f64 {
    flow.periods.iter().sum()
}

pub(super) fn carries_passengers(mode: u8) -> bool {
    mode != 2
}

pub(super) fn carries_freight(mode: u8) -> bool {
    mode != 1
}

struct TrackEvidence<'a> {
    model: &'a RailModel,
    domestic: &'a [RowTraffic],
    foreign: &'a [RowTraffic],
    priors: &'a [RowTraffic],
}

fn track_share(
    evidence: &TrackEvidence<'_>,
    own_iso: [u8; 2],
    members: &[usize],
    category: fn(&RowTraffic) -> CategoryFlow,
    own: usize,
    is_freight: bool,
    allows: impl Fn(usize) -> bool,
) -> CategoryFlow {
    let TrackEvidence {
        model,
        domestic,
        foreign,
        priors,
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
    let mut capable_count = 0usize;
    let mut prior = CategoryFlow::default();
    for &member in members {
        if !allows(member) {
            continue;
        }
        capable_count += 1;
        let flow = category(&priors[member]);
        if prior.status == STATUS_UNKNOWN || daily_total(&flow) > daily_total(&prior) {
            prior = flow;
        }
    }
    let winner = scope_winner(model, own_iso, members, domestic, line_ranked, category);
    let (line, from_prior) = if winner != 0 && !model.is_residual(winner) {
        let value = scope_value(model, members, domestic, winner, category);
        if model.stamps_whole_line(winner) {
            // A proxy estimates the line itself, not the tracks it stamps.
            (value, false)
        } else {
            let fully_covered = members
                .iter()
                .all(|&member| category(&domestic[member]).status != STATUS_UNKNOWN);
            if fully_covered || daily_total(&value) > daily_total(&prior) {
                (value, false)
            } else {
                (prior, true)
            }
        }
    } else if model.is_residual(winner) {
        (
            scope_value(model, members, domestic, winner, category),
            false,
        )
    } else {
        let abroad = scope_winner(model, own_iso, members, foreign, line_ranked, category);
        let value = if abroad == 0 {
            CategoryFlow::default()
        } else {
            scope_value(model, members, foreign, abroad, category)
        };
        if daily_total(&value) > daily_total(&prior) {
            (value, false)
        } else {
            (prior, true)
        }
    };
    // Class priors and whole-line guessed freight proxies are shared only among eligible tracks
    // so downstream eligibility checks do not throw away the share of an ineligible twin.
    let split_among_eligible =
        from_prior || (is_freight && GUESSED_TRAIN_SOURCES.contains(&line.source_id));
    if split_among_eligible && (!allows(own) || capable_count == 0) {
        return CategoryFlow {
            periods: [0.0; 3],
            missing_calendar_estimate: [0.0; 3],
            status: STATUS_ESTIMATED,
            source_id: 0,
            matching: 0,
            stop: 0,
        };
    }
    let divisor = if split_among_eligible {
        capable_count as f64
    } else {
        tracks
    };
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

pub(crate) fn allocate_over_parallel_tracks(
    model: &RailModel,
    rows: &mut [Expanded],
    square: crate::osm::geometry::Square,
) {
    if !rows
        .iter()
        .any(|row| row.service == 0 && row.child.resolved_section.is_none())
    {
        for row in rows.iter_mut() {
            if row.service == 0
                && let Some(resolved) = row.child.resolved_section
            {
                row.child.traffic = resolved;
            }
        }
        return;
    }
    let tracks = project_tracks(rows, square);
    let sections = cross_sections(rows, &tracks);
    let domestic: Vec<RowTraffic> = rows.iter().map(|row| row.child.traffic).collect();
    let foreign: Vec<RowTraffic> = rows.iter().map(|row| row.child.foreign).collect();
    let priors: Vec<RowTraffic> = rows.iter().map(|row| row.prior).collect();
    let modes: Vec<u8> = rows.iter().map(|row| row.traffic_mode).collect();
    let takes_freight: Vec<bool> = rows.iter().map(|row| row.takes_guessed_freight).collect();
    let evidence = TrackEvidence {
        model,
        domestic: &domestic,
        foreign: &foreign,
        priors: &priors,
    };
    for (index, row) in rows.iter_mut().enumerate() {
        if row.service != 0 {
            continue;
        }
        if let Some(resolved) = row.child.resolved_section {
            row.child.traffic = resolved;
            continue;
        }
        let members = &sections[index];
        let passenger = track_share(
            &evidence,
            row.country_iso,
            members,
            |t| t.passenger,
            index,
            false,
            |member| carries_passengers(modes[member]),
        );
        let freight = track_share(
            &evidence,
            row.country_iso,
            members,
            |t| t.freight,
            index,
            true,
            |member| takes_freight[member],
        );
        row.child.traffic = RowTraffic { passenger, freight };
    }
}

#[cfg(test)]
#[path = "allocation_tests.rs"]
mod allocation_tests;
