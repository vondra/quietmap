//! Airport ground operations as sources of the aircraft layer: every aeroway line with ground
//! traffic ([`crate::airport`]) a line piece above the pavement carrying its sound power per metre,
//! and one contributor per airport, as dev4 showed them.

use super::{Converted, group_key, split_at_tile_edges};
use crate::airport::file::{self, LineTraffic};
use crate::dev4::Square;
use physics::emission::airport::GroundOperation;
use serde_json::json;
use std::path::Path;
use tiles::geo::{GlobalSteps, Mercator};
use tiles::sources::{Attribute, Layer};

/// dev4's ground-operations source height (`GROUND_OPS_SOURCE_HEIGHT_M`).
const SOURCE_HEIGHT_M: f64 = 4.0;
/// The platform is half the pavement: runways 45 m and taxiways 23 m wide (ICAO Annex 14, code E).
const RUNWAY_HALF_WIDTH_M: f64 = 22.5;
const TAXIWAY_HALF_WIDTH_M: f64 = 11.5;
/// The sources format holds levels above -100 dB: a band this quiet is silence.
const QUIETEST_STORED_DB: f64 = -99.0;

/// The attribute of a line with traffic: 4 m above the pavement (hard ground under the source, as a
/// road's), grouped and named by its airport.
pub fn attribute(line: &LineTraffic) -> Attribute {
    let key = &line.airport.key;
    let [arrivals, departures, vehicles] = line
        .movements_per_day
        .map(|count| (count * 10.0).round() / 10.0);
    Attribute {
        layer: Layer::Aircraft,
        height_m: SOURCE_HEIGHT_M,
        ground_percent: 0,
        platform_half_width_m: match line.operation {
            GroundOperation::RunwayRoll => RUNWAY_HALF_WIDTH_M,
            GroundOperation::Taxi => TAXIWAY_HALF_WIDTH_M,
        },
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: group_key(&["airport", key]),
        emission: line.power_db.map(|bands| {
            bands.map(|level| {
                if level < QUIETEST_STORED_DB {
                    f64::NEG_INFINITY
                } else {
                    level
                }
            })
        }),
        display: json!([
            format!("{key} ground operations"),
            format!("airport_traffic:{key}"),
            line.airport.name,
            arrivals,
            departures,
            vehicles,
        ])
        .to_string(),
    }
}

/// Converts the aeroway lines with traffic of one square (the traffic pass's file under
/// `traffic`); returns how many lines emit.
pub fn convert(traffic: &Path, square: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
    let lines = file::read(traffic, square)?;
    for line in &lines {
        let attribute = attribute(line);
        let [start, end] = line
            .ends
            .map(|[lat, lon]| GlobalSteps::nearest(Mercator::from_degrees(lat, lon)));
        for (tile, ends) in split_at_tile_edges(start, end) {
            out.push(Converted {
                tile,
                ends,
                attribute: attribute.clone(),
            });
        }
    }
    Ok(lines.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airport::lines::Airport;
    use physics::bands::{BANDS, energy};
    use physics::emission::airport::aircraft_pass_energy_db;
    use physics::line::LinePieceGeometry;

    /// A runway of four 250 m lines east from (50.1, 14.25) with `departures` B738 departures a
    /// day rolling at 70 kt, by day only.
    fn runway(departures: f64) -> Vec<LineTraffic> {
        let pass = aircraft_pass_energy_db(2, GroundOperation::RunwayRoll, true, 70.0).unwrap();
        let degrees_per_metre = 1.0 / (111_320.0 * 50.1f64.to_radians().cos());
        (0..4)
            .map(|index| LineTraffic {
                osm_id: 1,
                segment: index,
                ends: [0.0, 250.0].map(|east| {
                    [
                        50.1,
                        14.25 + (250.0 * f64::from(index) + east) * degrees_per_metre,
                    ]
                }),
                operation: GroundOperation::RunwayRoll,
                airport: Airport {
                    key: "TEST".into(),
                    name: "Test Airport".into(),
                },
                movements_per_day: [0.0, departures, 0.0],
                power_db: std::array::from_fn(|period| {
                    pass.map(|level| {
                        if period == 0 {
                            level + 10.0 * (departures / (12.0 * 3_600.0)).log10()
                        } else {
                            f64::NEG_INFINITY
                        }
                    })
                }),
            })
            .collect()
    }

    /// A hundred departures a day on a 1 km runway, 25 m from its middle at the source height:
    /// the free-field day level of the four pieces through the CNOSSOS point sum is dev4's
    /// receiver formula, its per-metre level (anchor 104 + 9.01 + 2 dB) + 10 lg(theta / d) over
    /// a hundred events in 12 hours; one contributor names the airport.
    #[test]
    fn a_runway_of_departures_reads_dev4_level_at_25_m() {
        let lines = runway(100.0);
        let attributes: Vec<Attribute> = lines.iter().map(attribute).collect();
        let mut received = 0.0;
        for (index, attribute) in attributes.iter().enumerate() {
            let west = -500.0 + 250.0 * index as f64;
            let geometry =
                LinePieceGeometry::new([west, -25.0, 0.0], [west + 250.0, -25.0, 0.0]).unwrap();
            let bands: f64 = attribute.emission[0]
                .iter()
                .map(|level| energy(*level))
                .sum();
            received += bands * geometry.subtended_angle_rad() * geometry.divergence_factor();
            assert_eq!(attribute.emission[2], [f64::NEG_INFINITY; BANDS]);
            assert_eq!(attribute.group_key, attributes[0].group_key);
        }
        let theta = 2.0 * (500.0f64 / 25.0).atan();
        let dev4 = 104.0
            + 9.01
            + 2.0
            + 10.0 * (100.0f64 / (12.0 * 3_600.0)).log10()
            + 10.0 * (theta / 25.0).log10();
        assert!((10.0 * received.log10() - dev4).abs() < 0.01);
        let display: serde_json::Value = serde_json::from_str(&attributes[0].display).unwrap();
        assert_eq!(display[0], "TEST ground operations");
        assert_eq!(display[1], "airport_traffic:TEST");
        assert_eq!(display[4], 100.0);
        assert_eq!(attributes[0].height_m, SOURCE_HEIGHT_M);
    }
}
