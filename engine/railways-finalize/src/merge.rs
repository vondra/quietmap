//! Merge overlapping sidecar claims per category and split daily totals into END periods.

use crate::sidecar::Interval;
use crate::sources::{blocks_foreign_national, should_overwrite, source_applies_to_row};
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

pub fn row_traffic(
    intervals: &[Interval],
    from_m: f64,
    to_m: f64,
    rail_type: u8,
    usage: u8,
    service: u8,
    square_country_city: SquareCountryCity,
) -> RowTraffic {
    let row_iso = square_country_city.country_iso;
    let passenger_claims: Vec<Claim> = overlapping(intervals, from_m, to_m)
        .map(|interval| Claim {
            source_id: interval.source_id,
            trains: interval.passenger,
            status: interval.passenger_status,
            matching: interval.matching,
        })
        .collect();
    let freight_claims: Vec<Claim> = overlapping(intervals, from_m, to_m)
        .map(|interval| Claim {
            source_id: interval.source_id,
            trains: interval.freight,
            status: interval.freight_status,
            matching: interval.matching,
        })
        .collect();
    let rail = RailType::from_u8(rail_type);
    let shares = rail_time_dist(square_country_city, rail);
    let mut traffic = RowTraffic::default();
    if let Some(winner) = pick_winner(passenger_claims, row_iso) {
        traffic.passenger =
            estimated_flow(winner.trains, shares.pax, winner.source_id, winner.matching);
    }
    if let Some(winner) = pick_winner(freight_claims, row_iso) {
        traffic.freight =
            estimated_flow(winner.trains, shares.frt, winner.source_id, winner.matching);
    }
    fill_missing_priors(&mut traffic, rail_type, usage, service, square_country_city);
    traffic
}

pub fn fill_missing_priors(
    traffic: &mut RowTraffic,
    rail_type: u8,
    usage: u8,
    service: u8,
    square_country_city: SquareCountryCity,
) {
    if service != 0 {
        return;
    }
    let rail = RailType::from_u8(rail_type);
    let (passenger, freight) = default_traffic(rail, usage);
    let shares = rail_time_dist(square_country_city, rail);
    if traffic.passenger.status == STATUS_UNKNOWN {
        traffic.passenger = estimated_flow(passenger, shares.pax, 0, 0);
    }
    if traffic.freight.status == STATUS_UNKNOWN {
        traffic.freight = estimated_flow(freight, shares.frt, 0, 0);
    }
}

pub fn share_class_defaults(rows: &mut [RowTraffic], group_size: usize) {
    if group_size <= 1 {
        return;
    }
    let scale = 1.0 / group_size as f64;
    for row in rows {
        if row.passenger.source_id == 0 && row.passenger.status == STATUS_ESTIMATED {
            for value in &mut row.passenger.periods {
                *value *= scale;
            }
        }
        if row.freight.source_id == 0 && row.freight.status == STATUS_ESTIMATED {
            for value in &mut row.freight.periods {
                *value *= scale;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::Interval;

    fn interval(
        from_m: f64,
        to_m: f64,
        passenger: f64,
        freight: f64,
        freight_status: u8,
    ) -> Interval {
        Interval {
            osm_id: 1,
            segment_idx: 0,
            from_m,
            to_m,
            source_id: 100,
            passenger,
            freight,
            passenger_status: STATUS_ESTIMATED,
            freight_status,
            matching: 1,
        }
    }

    #[test]
    fn repeats_sum_and_only_unknown_categories_receive_shared_priors() {
        let intervals = [
            interval(10.0, 40.0, 2.0, 0.0, STATUS_UNKNOWN),
            interval(10.0, 40.0, 3.0, 9.0, STATUS_UNKNOWN),
        ];
        let cz = SquareCountryCity {
            continent: noise_compute::square_country_city::Continent::Europe,
            country_iso: *b"DE",
            city_id: 0,
        };
        let mut traffic = row_traffic(&intervals, 10.0, 40.0, 0, 0, 0, cz);
        let passenger_day = traffic.passenger.periods.iter().sum::<f64>();
        assert!((passenger_day - 5.0).abs() < 1e-9);
        assert_eq!(traffic.passenger.matching, 1);
        assert_eq!(traffic.freight.status, STATUS_ESTIMATED);
        assert!((traffic.freight.periods.iter().sum::<f64>() - 20.0).abs() < 1e-9);
        share_class_defaults(std::slice::from_mut(&mut traffic), 2);
        assert!((traffic.freight.periods.iter().sum::<f64>() - 10.0).abs() < 1e-9);
        assert!((traffic.passenger.periods.iter().sum::<f64>() - 5.0).abs() < 1e-9);
        let service = row_traffic(&intervals, 10.0, 40.0, 0, 0, 2, cz);
        assert_eq!(service.freight.status, STATUS_UNKNOWN);
        let zero = row_traffic(
            &[interval(10.0, 40.0, 0.0, 0.0, STATUS_KNOWN)],
            10.0,
            40.0,
            0,
            0,
            0,
            cz,
        );
        assert_eq!(zero.passenger.periods, [0.0; 3]);
        assert_eq!(zero.freight.periods, [0.0; 3]);
        assert_eq!(traffic.freight.source_id, 0);
        let mut freight_only = interval(10.0, 40.0, 99.0, 7.0, STATUS_KNOWN);
        freight_only.passenger_status = STATUS_UNKNOWN;
        let traffic = row_traffic(&[freight_only], 10.0, 40.0, 0, 0, 0, cz);
        assert!((traffic.passenger.periods.iter().sum::<f64>() - 80.0).abs() < 1e-9);
        assert_eq!(traffic.passenger.source_id, 0);
        assert!((traffic.freight.periods.iter().sum::<f64>() - 7.0).abs() < 1e-9);
        assert_eq!(traffic.freight.source_id, 100);
    }
}
