//! Merge overlapping interval claims per category and split daily totals into END periods.

use crate::sources::{blocks_foreign_national, should_overwrite, source_applies_to_row};
use crate::square_intervals::Interval;
use noise_compute::emission::railway::{default_traffic, rail_time_dist, RailType};
use noise_compute::square_country_city::SquareCountryCity;

pub const STATUS_UNKNOWN: u8 = 0;
pub const STATUS_KNOWN: u8 = 1;
pub const STATUS_ESTIMATED: u8 = 2;

pub use noise_compute::normalize::{
    RailCategoryTraffic as CategoryFlow, RailTraffic as RowTraffic,
};

#[derive(Clone, Copy)]
struct Claim {
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

fn pick_winner(mut claims: Vec<Claim>, row_iso: [u8; 2]) -> Option<Claim> {
    claims.retain(|claim| {
        claim.status != STATUS_UNKNOWN && source_applies_to_row(claim.source_id, row_iso)
    });
    if claims.is_empty() {
        return None;
    }
    let mut winner_id = 0u16;
    for claim in &claims {
        if winner_id != 0 && blocks_foreign_national(winner_id, claim.source_id) {
            continue;
        }
        if should_overwrite(winner_id, claim.source_id) {
            winner_id = claim.source_id;
        }
    }
    let mut trains = 0.0;
    let mut matching = 0u8;
    let mut status = STATUS_ESTIMATED;
    for claim in claims.iter().filter(|claim| claim.source_id == winner_id) {
        trains += claim.trains;
        matching |= claim.matching;
        if claim.status == STATUS_KNOWN {
            status = STATUS_KNOWN;
        }
    }
    Some(Claim {
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
        status: STATUS_ESTIMATED,
        source_id,
        matching,
    }
}

/// Per-track evidence of one child, split by timetable ownership: the winning source per
/// category among the track's own country files (domestic) and among neighbour files
/// (foreign cross-border services), no priors yet.
pub fn row_evidence(
    intervals: &[Interval],
    from_m: f64,
    to_m: f64,
    rail_type: u8,
    square_country_city: SquareCountryCity,
) -> (RowTraffic, RowTraffic) {
    let row_iso = square_country_city.country_iso;
    let claims =
        |category: fn(&Interval) -> (f64, u8), domestic: bool| -> Vec<Claim> {
            overlapping(intervals, from_m, to_m)
                .filter(|interval| (interval.country == row_iso) == domestic)
                .map(|interval| {
                    let (trains, status) = category(interval);
                    Claim {
                        source_id: interval.source_id,
                        trains,
                        status,
                        matching: interval.matching,
                    }
                })
                .collect()
        };
    let shares = rail_time_dist(square_country_city, RailType::from_u8(rail_type));
    let mut traffic = (RowTraffic::default(), RowTraffic::default());
    for (domestic, scope) in [(true, &mut traffic.0), (false, &mut traffic.1)] {
        if let Some(winner) = pick_winner(claims(|i| (i.passenger, i.passenger_status), domestic), row_iso)
        {
            scope.passenger =
                estimated_flow(winner.trains, shares.pax, winner.source_id, winner.matching);
        }
        if let Some(winner) = pick_winner(claims(|i| (i.freight, i.freight_status), domestic), row_iso)
        {
            scope.freight =
                estimated_flow(winner.trains, shares.frt, winner.source_id, winner.matching);
        }
    }
    traffic
}

/// The labelled class prior of one line (source 0); service tracks and preserved heritage
/// rail (type 5, silent until a heritage model exists) have none.
pub fn class_prior(
    rail_type: u8,
    usage: u8,
    service: u8,
    square_country_city: SquareCountryCity,
) -> RowTraffic {
    let rail = RailType::from_u8(rail_type);
    if service != 0 || matches!(rail, RailType::Preserved) {
        return RowTraffic::default();
    }
    let (passenger, freight) = default_traffic(rail, usage);
    let shares = rail_time_dist(square_country_city, rail);
    RowTraffic {
        passenger: estimated_flow(passenger, shares.pax, 0, 0),
        freight: estimated_flow(freight, shares.frt, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::square_intervals::Interval;

    fn interval(passenger: f64, freight: f64, freight_status: u8) -> Interval {
        Interval {
            osm_id: 1,
            segment_idx: 0,
            from_m: 10.0,
            to_m: 40.0,
            country: *b"DE",
            source_id: 100,
            passenger,
            freight,
            passenger_status: STATUS_ESTIMATED,
            freight_status,
            matching: 1,
        }
    }

    fn foreign_interval(passenger: f64) -> Interval {
        Interval { country: *b"AT", ..interval(passenger, 0.0, STATUS_UNKNOWN) }
    }

    #[test]
    fn repeats_sum_unknown_categories_stay_unknown_and_zero_is_known() {
        let de = SquareCountryCity {
            continent: noise_compute::square_country_city::Continent::Europe,
            country_iso: *b"DE",
            city_id: 0,
        };
        let intervals = [
            interval(2.0, 0.0, STATUS_UNKNOWN),
            interval(3.0, 9.0, STATUS_UNKNOWN),
            foreign_interval(7.0),
        ];
        let (traffic, foreign) = row_evidence(&intervals, 10.0, 40.0, 0, de);
        assert!((traffic.passenger.periods.iter().sum::<f64>() - 5.0).abs() < 1e-9);
        assert_eq!(traffic.passenger.matching, 1);
        assert_eq!(traffic.freight.status, STATUS_UNKNOWN);
        // Neighbour-file claims stay separate: cross-border trains are not the domestic total.
        assert!((foreign.passenger.periods.iter().sum::<f64>() - 7.0).abs() < 1e-9);
        assert_eq!(foreign.passenger.source_id, 100);
        assert_eq!(foreign.freight.status, STATUS_UNKNOWN);
        let (zero, _) = row_evidence(&[interval(0.0, 0.0, STATUS_KNOWN)], 10.0, 40.0, 0, de);
        assert_eq!(zero.freight.periods, [0.0; 3]);
        // Daily evidence, even a known zero, carries an estimated period split.
        assert_eq!(zero.freight.status, STATUS_ESTIMATED);
        let prior = class_prior(0, 0, 0, de);
        assert!((prior.passenger.periods.iter().sum::<f64>() - 80.0).abs() < 1e-9);
        assert!((prior.freight.periods.iter().sum::<f64>() - 85.0).abs() < 1e-9);
        assert_eq!(prior.freight.source_id, 0);
        assert_eq!(class_prior(0, 0, 2, de), RowTraffic::default());
        for usage in [0, 1, 2, 3, 4] {
            assert_eq!(class_prior(5, usage, 0, de), RowTraffic::default());
        }
        let (heritage, _) =
            row_evidence(&[interval(3.0, 0.0, STATUS_UNKNOWN)], 10.0, 40.0, 5, de);
        assert!((heritage.passenger.periods.iter().sum::<f64>() - 3.0).abs() < 1e-9);
        assert_eq!(heritage.passenger.source_id, 100);
        assert_eq!(heritage.freight, CategoryFlow::default());
    }
}
