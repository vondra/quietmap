//! Merge overlapping interval claims per category and split daily totals into END periods.

use super::model::RailModel;
use super::square_intervals::Interval;

pub const STATUS_UNKNOWN: u8 = 0;
pub const STATUS_KNOWN: u8 = 1;
pub const STATUS_ESTIMATED: u8 = 2;

pub use super::types::{CategoryFlow, RowTraffic};

#[derive(Clone, Copy)]
struct Claim {
    missing_calendar_estimate: f64,
    source_id: u16,
    trains: f64,
    status: u8,
    matching: u8,
}

pub fn overlapping(
    intervals: &[Interval],
    from_m: f64,
    to_m: f64,
) -> impl Iterator<Item = &Interval> {
    intervals
        .iter()
        .filter(move |interval| interval.from_m < to_m && interval.to_m > from_m)
}

fn pick_winner(model: &RailModel, mut claims: Vec<Claim>, row_iso: [u8; 2]) -> Option<Claim> {
    claims.retain(|claim| {
        claim.status != STATUS_UNKNOWN && model.source_applies_to_row(claim.source_id, row_iso)
    });
    if claims.is_empty() {
        return None;
    }
    let mut winner_id = 0u16;
    for claim in &claims {
        if winner_id != 0 && model.blocks_foreign_national(winner_id, claim.source_id) {
            continue;
        }
        if model.should_overwrite(winner_id, claim.source_id) {
            winner_id = claim.source_id;
        }
    }
    let mut trains = 0.0;
    let mut missing_calendar_estimate = 0.0;
    let mut matching = 0u8;
    let mut status = STATUS_ESTIMATED;
    for claim in claims.iter().filter(|claim| claim.source_id == winner_id) {
        trains += claim.trains;
        missing_calendar_estimate += claim.missing_calendar_estimate;
        matching |= claim.matching;
        if claim.status == STATUS_KNOWN {
            status = STATUS_KNOWN;
        }
    }
    Some(Claim {
        missing_calendar_estimate,
        source_id: winner_id,
        trains,
        status,
        matching,
    })
}

fn to_periods(daily: f64, shares: [f64; 3]) -> [f64; 3] {
    [daily * shares[0], daily * shares[1], daily * shares[2]]
}

fn estimated_flow(daily: f64, shares: [f64; 3], source_id: u16, matching: u8) -> CategoryFlow {
    CategoryFlow {
        periods: to_periods(daily, shares),
        missing_calendar_estimate: [0.0; 3],
        status: STATUS_ESTIMATED,
        source_id,
        matching,
    }
}

/// Per-track evidence of one child, split by timetable ownership: the winning source per
/// category among the track's own country files (domestic) and among neighbour files
/// (foreign cross-border services), no priors yet.
pub fn row_evidence(
    model: &RailModel,
    intervals: &[Interval],
    from_m: f64,
    to_m: f64,
    rail_type: u8,
    country: [u8; 2],
) -> (RowTraffic, RowTraffic) {
    let row_iso = country;
    let claims = |category: fn(&Interval) -> (f64, f64, u8), domestic: bool| -> Vec<Claim> {
        overlapping(intervals, from_m, to_m)
            .filter(|interval| (interval.country == row_iso) == domestic)
            .map(|interval| {
                let (trains, missing_calendar_estimate, status) = category(interval);
                Claim {
                    missing_calendar_estimate,
                    source_id: interval.source_id,
                    trains,
                    status,
                    matching: interval.matching,
                }
            })
            .collect()
    };
    let shares = model.period_shares(country, rail_type);
    let mut traffic = (RowTraffic::default(), RowTraffic::default());
    for (domestic, scope) in [(true, &mut traffic.0), (false, &mut traffic.1)] {
        if let Some(winner) = pick_winner(
            model,
            claims(
                |i| {
                    (
                        i.passenger,
                        i.missing_calendar_estimate[0],
                        i.passenger_status,
                    )
                },
                domestic,
            ),
            row_iso,
        ) {
            scope.passenger =
                estimated_flow(winner.trains, shares.pax, winner.source_id, winner.matching);
            scope.passenger.missing_calendar_estimate =
                to_periods(winner.missing_calendar_estimate, shares.pax);
        }
        if let Some(winner) = pick_winner(
            model,
            claims(
                |i| (i.freight, i.missing_calendar_estimate[1], i.freight_status),
                domestic,
            ),
            row_iso,
        ) {
            scope.freight =
                estimated_flow(winner.trains, shares.frt, winner.source_id, winner.matching);
            scope.freight.missing_calendar_estimate =
                to_periods(winner.missing_calendar_estimate, shares.frt);
        }
    }
    traffic
}

/// The labelled class prior of one line (source 0); service tracks and preserved heritage
/// rail (type 5, silent until a heritage model exists) have none.
pub fn class_prior(
    model: &RailModel,
    rail_type: u8,
    usage: u8,
    service: u8,
    country: [u8; 2],
    traffic_mode: u8,
) -> RowTraffic {
    if service != 0 || rail_type == 5 {
        return RowTraffic::default();
    }
    let (passenger, freight) = model.default_traffic(rail_type, usage, country, traffic_mode);
    let shares = model.period_shares(country, rail_type);
    RowTraffic {
        passenger: estimated_flow(passenger, shares.pax, 0, 0),
        freight: estimated_flow(freight, shares.frt, 0, 0),
    }
}
