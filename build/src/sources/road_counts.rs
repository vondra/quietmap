//! A road's count held against the road it lies on. dev4 joined counts to roads by route number,
//! by the nearest station and by tables of class values; the builder kept every one. A count is
//! the road's where it fits the road: along a run where nothing joins it is one traffic (`runs`);
//! no lane carries more than the busiest counted lanes do; and a station's count is not a small
//! fraction of what the road's class, lanes and surroundings predict (Warsaw's Trasa Łazienkowska
//! carried 32 vehicles a carriageway a day, a five-hundredth of the model, from a route number
//! shared with a country road; evidence 2026-11-12, data-1). A row that fails takes its model.

use super::road::{DEFAULT_LANES, guessed};
use super::road_traffic::{BuildingTraffic, major_group};
use super::runs::{RunRow, run_medians};
use crate::dev4::{Table, column};
use arrow_array::{BooleanArray, Float32Array, Float64Array, Int32Array, UInt8Array, UInt16Array};
use std::collections::HashSet;
use tiles::sources::{NETWORK_ESTIMATE_SOURCE_IDS, PRIOR_SOURCE_ID, SERVICE_TREE_SOURCE_ID};

/// Vehicles a lane a day that counted lanes exceed but rarely: the 99.5th km-percentile of the
/// counted one-way motorway carriageways of 2-6 lanes (329,702 km in 26 sources: 29,419 a lane, and
/// 27,900-30,400 at each lane count) and of the counted two-way trunk and primary roads of two
/// lanes (292,469 km in 31 sources: 14,019; four-lane ones carry 11,723 a lane). Every class shares
/// them: they are a lane's, not a class's.
pub const ONE_WAY_LANE_CAPACITY: f64 = 29_400.0;
pub const TWO_WAY_LANE_CAPACITY: f64 = 14_000.0;
/// A one-way carriageway that tags no lanes carries at most what the counted untagged ones do: the
/// 99.5th km-percentile of the counted one-way motorway carriageways without a lane count (3,939 km,
/// 15 sources), two and a half tagged lanes. A two-way road that tags none counts two lanes (its
/// counted untagged trunk and primary roads: 27,392 and 33,328).
const UNTAGGED_ONE_WAY_CAPACITY: f64 = 72_900.0;

/// The fraction of the major-road model under which a station's count is not the road's: the
/// 0.1st km-percentile of every counted major row's count over the model (12.1 million rows with a
/// model, table sources left out; 987 km fall under it: 32 a carriageway on Warsaw's expressway, 163
/// on a three-lane Los Angeles arterial). Spain's emptiest toll motorway, the AP-41, reads 0.034-0.046.
pub const LEAST_COUNT_OF_MODEL: f64 = 0.025;

/// The low edge of the band of counts around the major model, by class group (motorway, trunk,
/// primary, link) and surroundings (rural, urban; unknown takes rural): three scaled median
/// absolute deviations of ln(count / model) under its median over the counted rows of the group.
/// An estimate (a table's value or a country default) below it is not the road's.
const ESTIMATE_LEAST_OF_MODEL: [[f64; 2]; 4] = [
    [0.218, 0.320],
    [0.205, 0.213],
    [0.175, 0.274],
    [0.067, 0.093],
];

/// What a road row's traffic is, for the rules: a station's count, an estimate (a network's table
/// of values, `NETWORK_ESTIMATE_SOURCE_IDS`, or a country default) or neither (a class prior, a
/// copy, a street's routed trips).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Traffic {
    Count,
    Estimate,
    Other,
}

/// The most vehicles a day a row of `lanes` (0 untagged) carries, one-way or not.
pub fn capacity(oneway: bool, lanes: u8) -> f64 {
    match (oneway, lanes) {
        (true, 0) => UNTAGGED_ONE_WAY_CAPACITY,
        (true, lanes) => f64::from(lanes) * ONE_WAY_LANE_CAPACITY,
        (false, 0) => f64::from(DEFAULT_LANES) * TWO_WAY_LANE_CAPACITY,
        (false, lanes) => f64::from(lanes) * TWO_WAY_LANE_CAPACITY,
    }
}

/// Whether a row's `vehicles` are not the road's and it takes its model: a count over its lanes'
/// capacity (an estimate over it is only cut to it, [`HeldCounts::capped`]: India's national table
/// put 35,000 on two-lane roads the model, untested there, puts at a seventh of it), or, held
/// against the major model's `predicted` traffic of a major row (asked only then; `None` where the
/// buildings give none), a count under [`LEAST_COUNT_OF_MODEL`] of it or an estimate under its
/// band's low edge.
pub fn implausible(
    traffic: Traffic,
    vehicles: f64,
    (class, oneway, lanes, built_up): (u8, bool, u8, u8),
    predicted: impl FnOnce() -> Option<f64>,
) -> bool {
    if traffic == Traffic::Other || vehicles <= 0.0 {
        return false;
    }
    if vehicles > capacity(oneway, lanes) {
        return traffic == Traffic::Count;
    }
    let Some(group) = major_group(class) else {
        return false;
    };
    let Some(predicted) = predicted().filter(|p| *p > 0.0) else {
        return false;
    };
    let least = match traffic {
        Traffic::Count => LEAST_COUNT_OF_MODEL,
        _ => ESTIMATE_LEAST_OF_MODEL[group][usize::from(built_up == 2)],
    };
    vehicles < least * predicted
}

/// A row's traffic as dev4 gave it: its flows (light, medium, heavy, motorcycles), its road's
/// cross-section, its source and which categories are estimates.
#[derive(Clone, Copy, Debug)]
pub struct RowTraffic {
    pub flows: [f64; 4],
    pub cross_section: f64,
    pub source: u16,
    pub estimated: u8,
    lanes: u8,
    oneway: bool,
}

/// A square's counts as the rules hold them: per row the row whose traffic it carries (its run's
/// median, else itself), every row's own traffic, the rows whose traffic is not the road's (they
/// take their model) and the estimates over their lanes' capacity (cut to it).
pub struct HeldCounts {
    carried: Vec<usize>,
    rows: Vec<RowTraffic>,
    pub implausible: HashSet<usize>,
    pub capped: HashSet<usize>,
}

impl HeldCounts {
    /// The traffic `row` carries: its run median's, with that row's source and estimate flags.
    pub fn of(&self, row: usize) -> RowTraffic {
        self.rows[self.carried[row]]
    }

    /// The cross-section `row` shows when it carries `vehicles` a day: a replaced row's own (none
    /// for a one-way one, whose other carriageway's count may be real); a row carrying its run's
    /// count its own pair's with its carriageway's change added (a carriageway of 1 vehicle a day in
    /// a run of 46,000 multiplied its pair's 50,000 into 2.3 billion); any other its own, scaled.
    pub fn cross_section(&self, row: usize, vehicles: f64, replaced: bool) -> f64 {
        let own = self.rows[row];
        let own_vehicles: f64 = own.flows.iter().sum();
        match (replaced, own.oneway) {
            (true, true) => 0.0,
            (true, false) => vehicles,
            _ if self.carried[row] == row => {
                own.cross_section * vehicles / own_vehicles.max(f64::MIN_POSITIVE)
            }
            (false, true) if own.cross_section > own_vehicles => {
                own.cross_section - own_vehicles + vehicles
            }
            (false, true) => 0.0,
            (false, false) => vehicles,
        }
    }

    /// The row's lanes, or where it tags none (a bridge of a run, often) its run median's.
    fn lanes(&self, row: usize) -> u8 {
        match self.rows[row].lanes {
            0 => self.rows[self.carried[row]].lanes,
            lanes => lanes,
        }
    }

    /// The most vehicles a day the row carries ([`capacity`] of its lanes).
    pub fn capacity(&self, row: usize) -> f64 {
        capacity(self.rows[row].oneway, self.lanes(row))
    }
}

/// The rules over a square's road rows: runs of counted rows where nothing joins carry their
/// median, then every count and estimate is held against its lanes and, where the buildings give
/// it, the major model; `copies` (`road_copies`) take their model already.
pub fn held_counts(
    table: &Table,
    building_traffic: Option<&BuildingTraffic>,
    copies: &HashSet<usize>,
) -> Result<HeldCounts, String> {
    struct Row {
        traffic: Traffic,
        shape: (u8, bool, u8, u8),
        middle: (i32, i32),
    }
    let (mut rows, mut runs, mut traffics) = (Vec::new(), Vec::new(), Vec::new());
    for batch in &table.batches {
        let i32s = |name| column::<Int32Array>(batch, name);
        let u8s = |name| column::<UInt8Array>(batch, name);
        let f64s = |name| column::<Float64Array>(batch, name);
        let ends = [
            (i32s("start_gx")?, i32s("start_gy")?),
            (i32s("end_gx")?, i32s("end_gy")?),
        ];
        let (class, oneway, lanes, built_up, estimated) = (
            u8s("road_class")?,
            u8s("oneway")?,
            u8s("lanes")?,
            u8s("built_up")?,
            u8s("traffic_estimated")?,
        );
        let aadt = [
            f64s("aadt_light")?,
            f64s("aadt_medium")?,
            f64s("aadt_heavy")?,
            f64s("aadt_moto")?,
        ];
        let cross_section = f64s("cross_section_aadt")?;
        let source = column::<UInt16Array>(batch, "source_id")?;
        let length = column::<Float32Array>(batch, "length_m")?;
        let tunnel = column::<BooleanArray>(batch, "tunnel")?;
        for row in 0..batch.num_rows() {
            let index = rows.len();
            let [a, b] = ends.map(|(x, y)| (x.value(row), y.value(row)));
            let flow: [f64; 4] = std::array::from_fn(|k| aadt[k].value(row));
            let vehicles: f64 = flow.iter().sum();
            let traffic = if copies.contains(&index)
                || source.value(row) == PRIOR_SOURCE_ID
                || source.value(row) == SERVICE_TREE_SOURCE_ID
            {
                Traffic::Other
            } else if guessed(source.value(row), estimated.value(row))
                || NETWORK_ESTIMATE_SOURCE_IDS.contains(&source.value(row))
            {
                Traffic::Estimate
            } else {
                Traffic::Count
            };
            let counted = traffic == Traffic::Count && vehicles > 0.0 && !tunnel.value(row);
            runs.push(RunRow {
                meets: 0,
                joins: counted.then_some(u8::from(oneway.value(row) != 0)),
                ends: [a, b],
                length_m: f64::from(length.value(row)),
                value: vehicles,
            });
            traffics.push(RowTraffic {
                flows: flow,
                cross_section: cross_section.value(row),
                source: source.value(row),
                estimated: estimated.value(row),
                lanes: lanes.value(row),
                oneway: oneway.value(row) != 0,
            });
            rows.push(Row {
                traffic,
                shape: (
                    class.value(row),
                    oneway.value(row) != 0,
                    lanes.value(row),
                    built_up.value(row),
                ),
                middle: (
                    ((i64::from(a.0) + i64::from(b.0)) / 2) as i32,
                    ((i64::from(a.1) + i64::from(b.1)) / 2) as i32,
                ),
            });
        }
    }
    let mut held = HeldCounts {
        carried: run_medians(&runs),
        rows: traffics,
        implausible: HashSet::new(),
        capped: HashSet::new(),
    };
    held.capped = rows
        .iter()
        .enumerate()
        .filter(|(index, row)| {
            row.traffic == Traffic::Estimate
                && held.rows[*index].flows.iter().sum::<f64>() > held.capacity(*index)
        })
        .map(|(index, _)| index)
        .collect();
    held.implausible = rows
        .iter()
        .enumerate()
        .filter(|(index, row)| {
            let vehicles: f64 = held.of(*index).flows.iter().sum();
            let (class, oneway, lanes, built_up) = row.shape;
            let held_shape = (class, oneway, held.lanes(*index), built_up);
            implausible(row.traffic, vehicles, held_shape, || {
                building_traffic.map(|traffic| {
                    traffic.major_prediction(row.middle, (class, built_up, oneway, lanes))
                })
            })
        })
        .map(|(index, _)| index)
        .collect();
    Ok(held)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// India's national-highway table puts 70,000 vehicles a day on a two-lane trunk road (and
    /// 35,000 where it tags no lanes), Indiana's ramps carry their interstate's 80,000 on one lane,
    /// Houston's HOV lane both carriageways' 287,789: over capacity. Three-lane carriageways at
    /// 80,000 are not.
    #[test]
    fn no_lane_carries_more_than_the_busiest_counted_lanes() {
        let at = |vehicles, oneway, lanes| {
            implausible(Traffic::Count, vehicles, (1, oneway, lanes, 1), || None)
        };
        assert!(at(70_000.0, false, 2));
        assert!(at(35_000.0, false, 0));
        assert!(!at(15_000.0, false, 0));
        assert!(at(80_000.0, true, 1));
        assert!(at(287_789.0, false, 1));
        assert!(!at(80_000.0, true, 3));
        // An untagged carriageway carries what the counted untagged ones do (72,900).
        assert!(!at(70_000.0, true, 0) && at(80_000.0, true, 0));
        // An estimate over capacity keeps its own traffic, cut to the capacity (`capped`).
        assert!(!implausible(
            Traffic::Estimate,
            70_000.0,
            (1, false, 2, 1),
            || Some(5_000.0)
        ));
        assert!(
            !implausible(Traffic::Other, 287_789.0, (1, false, 1, 1), || None),
            "a prior"
        );
    }

    /// Warsaw's expressway carriageway with 32 vehicles a day where the model predicts 17,000 is
    /// not counted there; Spain's AP-41 at 569 a day beside the model's 15,600 is (a toll road's
    /// own count); Riyadh's ministry value of 7,346 on an urban motorway carriageway the model
    /// puts at 54,610 is an estimate under the band, as is Caracas's SIGOT tier of 1,000 on a
    /// rural motorway (12,816); a rural estimate at 5 times the model stands (through traffic the
    /// buildings around do not show).
    #[test]
    fn a_count_far_under_the_model_and_an_estimate_under_the_band_take_the_model() {
        let warsaw = (2, true, 3, 2);
        assert!(implausible(Traffic::Count, 32.0, warsaw, || Some(16_964.0)));
        assert!(!implausible(Traffic::Count, 569.0, (0, true, 2, 1), || {
            Some(15_600.0)
        }));
        assert!(implausible(
            Traffic::Estimate,
            7_346.0,
            (0, true, 3, 2),
            || Some(54_610.0)
        ));
        assert!(implausible(
            Traffic::Estimate,
            1_000.0,
            (0, true, 2, 1),
            || Some(12_816.0)
        ));
        assert!(!implausible(
            Traffic::Estimate,
            25_000.0,
            (1, false, 0, 1),
            || Some(5_000.0)
        ));
        assert!(
            !implausible(Traffic::Count, 32.0, warsaw, || None),
            "no model, no floor"
        );
        assert!(
            !implausible(Traffic::Estimate, 100.0, (4, false, 2, 2), || Some(5_000.0)),
            "a tertiary road"
        );
    }

    /// A carriageway of 1 vehicle a day paired into 50,000 carries its run's 46,000: its pair shows
    /// 95,999, not 2.3 billion; a replaced one-way row shows none, a replaced two-way row its own; a
    /// row carrying its own traffic scales its pair (20,000 for 10,000, modelled at 12,000: 24,000).
    #[test]
    fn a_cross_section_follows_its_own_carriageway() {
        let row = |vehicles: f64, cross_section: f64, oneway: bool| RowTraffic {
            flows: [vehicles, 0.0, 0.0, 0.0],
            cross_section,
            source: 23,
            estimated: 0,
            lanes: 2,
            oneway,
        };
        let held = HeldCounts {
            carried: vec![1, 1, 2, 3],
            rows: vec![
                row(1.0, 50_000.0, true),
                row(46_000.0, 92_000.0, true),
                row(10_000.0, 20_000.0, true),
                row(30_000.0, 30_000.0, false),
            ],
            implausible: HashSet::new(),
            capped: HashSet::new(),
        };
        assert_eq!(held.cross_section(0, 46_000.0, false), 95_999.0);
        assert_eq!(held.cross_section(0, 40_000.0, true), 0.0);
        assert_eq!(held.cross_section(3, 25_000.0, true), 25_000.0);
        assert_eq!(held.cross_section(2, 12_000.0, false), 24_000.0);
    }
}
