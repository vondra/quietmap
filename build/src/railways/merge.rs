//! Merge overlapping interval claims per category and split daily totals into END periods.

use super::model::RailModel;
use super::square_intervals::Interval;
use super::types::{CrossingFreight, CrossingTotal};

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
    stop: u32,
    crossing: Option<CrossingTotal>,
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
    let mut stops = Vec::new();
    let mut crossing_count = 0usize;
    let mut crossing_periods = [0.0; 3];
    let mut crossing_pax_sum = 0.0;
    let mut crossing_pax_all_known = true;
    let mut crossing_measured = true;
    let mut crossing_tracks = 0u8;
    for claim in claims.iter().filter(|claim| claim.source_id == winner_id) {
        trains += claim.trains;
        missing_calendar_estimate += claim.missing_calendar_estimate;
        matching |= claim.matching;
        stops.push(claim.stop);
        if claim.status == STATUS_KNOWN {
            status = STATUS_KNOWN;
        }
        if let Some(crossing) = claim.crossing {
            crossing_count += 1;
            for (sum, value) in crossing_periods.iter_mut().zip(crossing.periods_per_track) {
                *sum += value;
            }
            match crossing.passenger_per_track {
                Some(pax) => crossing_pax_sum += pax,
                None => crossing_pax_all_known = false,
            }
            crossing_measured = crossing_measured && crossing.measured_periods;
            crossing_tracks = crossing_tracks.max(crossing.matched_tracks);
        }
    }
    let crossing = (crossing_count > 0).then(|| {
        let count = crossing_count as f64;
        trains /= count;
        CrossingTotal {
            periods_per_track: crossing_periods.map(|v| v / count),
            passenger_per_track: crossing_pax_all_known.then_some(crossing_pax_sum / count),
            measured_periods: crossing_measured,
            matched_tracks: crossing_tracks.max(1),
        }
    });
    // A row stamped by one stop alone keeps it, so its line counts the stop once.
    let stop = if stops.windows(2).all(|pair| pair[0] == pair[1]) {
        stops[0]
    } else {
        0
    };
    Some(Claim {
        missing_calendar_estimate,
        source_id: winner_id,
        trains,
        status,
        matching,
        stop,
        crossing,
    })
}

fn to_periods(daily: f64, shares: [f64; 3]) -> [f64; 3] {
    [daily * shares[0], daily * shares[1], daily * shares[2]]
}

fn estimated_flow(daily: f64, shares: [f64; 3], source_id: u16, matching: u8) -> CategoryFlow {
    CategoryFlow {
        periods: to_periods(daily, shares),
        missing_calendar_estimate: [0.0; 3],
        stop: 0,
        status: STATUS_ESTIMATED,
        source_id,
        matching,
        matched_tracks: 0,
        crossing_freight: None,
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
                    stop: interval.stop,
                    crossing: interval.crossing,
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
            scope.passenger.stop = winner.stop;
            if let Some(crossing) = winner.crossing {
                let total_daily: f64 = crossing.periods_per_track.iter().sum();
                if crossing.measured_periods
                    && total_daily > 0.0
                    && (winner.trains - total_daily).abs() <= 1e-6
                {
                    // Passenger-only crossing with measured day/night blocks.
                    scope.passenger.periods = crossing.periods_per_track;
                }
                scope.passenger.matched_tracks = crossing.matched_tracks;
            }
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
            scope.freight.stop = winner.stop;
            if let Some(crossing) = winner.crossing {
                scope.freight.periods = crossing.periods_per_track;
                scope.freight.matched_tracks = crossing.matched_tracks;
                scope.freight.crossing_freight = Some(CrossingFreight {
                    inventory_passenger: crossing.passenger_per_track,
                    measured_periods: crossing.measured_periods,
                });
            }
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
