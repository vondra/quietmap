//! A road's count held against the road it lies on. dev4 joined counts to roads by route number,
//! by the nearest station and by tables of class values; the builder kept every one. A count is
//! the road's where it fits the road: along a run where nothing joins it is one traffic (`runs`);
//! no lane carries more than the busiest counted lanes do; and a station's count is not a small
//! fraction of what the road's class, lanes and surroundings predict (Warsaw's Trasa Łazienkowska
//! carried 32 vehicles a carriageway a day, a five-hundredth of the model, from a route number
//! shared with a country road; evidence 2026-11-12, data-1). Annual prepared trees record per row
//! whether the observation was measured (`traffic_observation_measured`) and whether it carried
//! point/line geometry (`traffic_observation_located`); only guessed class fallbacks and
//! unmeasured, unlocated route/network estimates face the tighter estimate band and stay out of
//! runs. Located link estimates (US HPMS B/E, UK DfT prior-year links, Amsterdam VMA) stay counts
//! so quiet roads are not overwritten by the global model. A row that fails takes its model.

use super::road::{DEFAULT_LANES, guessed};
use super::road_traffic::{BuildingTraffic, major_group};
use super::runs::{RunRow, run_medians};
use crate::dev4::{Table, column};
use crate::source_tables::RoadModels;
use arrow_array::cast::AsArray;
use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int32Array, RecordBatch, UInt8Array,
    UInt16Array,
};
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
/// An estimate (an unlocated route/network table or a country default) below it is not the road's.
const ESTIMATE_LEAST_OF_MODEL: [[f64; 2]; 4] = [
    [0.218, 0.320],
    [0.205, 0.213],
    [0.175, 0.274],
    [0.067, 0.093],
];

/// What a road row's traffic is, for the rules: a link count/observation, an unlocated estimate
/// (guessed class fallback, unmeasured unlocated route/network median, or legacy dev4
/// `NETWORK_ESTIMATE_SOURCE_IDS`), or neither (a class prior, a copy, a street's routed trips).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Traffic {
    Count,
    Estimate,
    Other,
}

/// Plausibility outcome for a row's traffic against its lanes and major-road model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disposition {
    Keep,
    Cap,
    Replace,
}

/// Classify a row and whether over-capacity values are capped (`true`) rather than replaced
/// (`false`). Guessed class fallbacks (e.g. MLIT `9865`) and frozen dev4 network estimates cap over
/// capacity; annual unlocated unmeasured estimates replace over capacity; measured or located
/// observations stay `Count`.
fn classify_row(
    is_copy: bool,
    source: u16,
    estimated: u8,
    observation_measured: Option<bool>,
    observation_located: Option<bool>,
) -> (Traffic, bool) {
    if is_copy || source == PRIOR_SOURCE_ID || source == SERVICE_TREE_SOURCE_ID {
        return (Traffic::Other, false);
    }
    if guessed(source, estimated) {
        return (Traffic::Estimate, true);
    }
    match (observation_measured, observation_located) {
        (Some(true), _) | (Some(false), Some(true)) => (Traffic::Count, false),
        (Some(false), Some(false)) => (Traffic::Estimate, false),
        (Some(false) | None, None) if NETWORK_ESTIMATE_SOURCE_IDS.contains(&source) => {
            (Traffic::Estimate, observation_measured.is_none())
        }
        (Some(false) | None, None) | (None, Some(_)) => (Traffic::Count, false),
    }
}

fn optional_bool_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<Option<&'a BooleanArray>, String> {
    let Some(array) = batch.column_by_name(name) else {
        return Ok(None);
    };
    let values = array
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| format!("column {name} is not boolean"))?;
    if values.null_count() > 0 {
        return Err(format!("column {name} contains nulls"));
    }
    Ok(Some(values))
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

/// Whether a row's `vehicles` stand (`Keep`), are cut to lane capacity (`Cap`), or take the
/// model (`Replace`): an over-capacity row caps only when `cap_over_capacity` is true (legacy
/// dev4 estimates and `guessed` fallbacks) and otherwise replaces; held against the major model's
/// `predicted` traffic of a major row (asked only then; `None` where the buildings give none), a
/// count under [`LEAST_COUNT_OF_MODEL`] of it or an estimate under its band's low edge replaces.
pub fn disposition(
    traffic: Traffic,
    cap_over_capacity: bool,
    vehicles: f64,
    (class, oneway, lanes, built_up): (u8, bool, u8, u8),
    predicted: impl FnOnce() -> Option<f64>,
) -> Disposition {
    if traffic == Traffic::Other || vehicles <= 0.0 {
        return Disposition::Keep;
    }
    if vehicles > capacity(oneway, lanes) {
        return if cap_over_capacity {
            Disposition::Cap
        } else {
            Disposition::Replace
        };
    }
    let Some(group) = major_group(class) else {
        return Disposition::Keep;
    };
    let Some(predicted) = predicted().filter(|p| *p > 0.0) else {
        return Disposition::Keep;
    };
    let least = match traffic {
        Traffic::Count => LEAST_COUNT_OF_MODEL,
        _ => ESTIMATE_LEAST_OF_MODEL[group][usize::from(built_up == 2)],
    };
    if vehicles < least * predicted {
        Disposition::Replace
    } else {
        Disposition::Keep
    }
}

/// A row's traffic as the prepared tree gives it: its flows (light, medium, heavy, motorcycles),
/// its road's cross-section, its source, which categories are estimates and which of its
/// observation's categories were.
#[derive(Clone, Copy, Debug)]
pub struct RowTraffic {
    pub flows: [f64; 4],
    pub cross_section: f64,
    pub source: u16,
    pub estimated: u8,
    pub observation_estimated: u8,
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
    building_traffic: Option<(&BuildingTraffic, &RoadModels)>,
    copies: &HashSet<usize>,
) -> Result<HeldCounts, String> {
    held_counts_with_prediction(table, copies, |middle, key| {
        building_traffic.map(|(traffic, models)| traffic.major_prediction(models, middle, key))
    })
}

fn held_counts_with_prediction(
    table: &Table,
    copies: &HashSet<usize>,
    predict: impl Fn((i32, i32), (u8, u8, bool, u8)) -> Option<f64>,
) -> Result<HeldCounts, String> {
    struct Row {
        traffic: Traffic,
        cap_over_capacity: bool,
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
        let observation_estimated = batch
            .column_by_name("traffic_observation_estimated")
            .map(|a| a.as_primitive::<arrow_array::types::UInt8Type>())
            .unwrap_or(estimated);
        let observation_measured = optional_bool_column(batch, "traffic_observation_measured")?;
        let observation_located = optional_bool_column(batch, "traffic_observation_located")?;
        if observation_located.is_some() && observation_measured.is_none() {
            return Err(
                "column traffic_observation_located requires traffic_observation_measured".into(),
            );
        }
        let source = column::<UInt16Array>(batch, "source_id")?;
        let length = column::<Float32Array>(batch, "length_m")?;
        let tunnel = column::<BooleanArray>(batch, "tunnel")?;
        for row in 0..batch.num_rows() {
            let index = rows.len();
            let [a, b] = ends.map(|(x, y)| (x.value(row), y.value(row)));
            let flow: [f64; 4] = std::array::from_fn(|k| aadt[k].value(row));
            let vehicles: f64 = flow.iter().sum();
            let (traffic, cap_over_capacity) = classify_row(
                copies.contains(&index),
                source.value(row),
                estimated.value(row),
                observation_measured.map(|m| m.value(row)),
                observation_located.map(|l| l.value(row)),
            );
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
                observation_estimated: observation_estimated.value(row),
                lanes: lanes.value(row),
                oneway: oneway.value(row) != 0,
            });
            rows.push(Row {
                traffic,
                cap_over_capacity,
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
    for (index, row) in rows.iter().enumerate() {
        let vehicles: f64 = held.of(index).flows.iter().sum();
        let (class, oneway, lanes, built_up) = row.shape;
        let held_shape = (class, oneway, held.lanes(index), built_up);
        match disposition(
            row.traffic,
            row.cap_over_capacity,
            vehicles,
            held_shape,
            || predict(row.middle, (class, built_up, oneway, lanes)),
        ) {
            Disposition::Keep => {}
            Disposition::Cap => {
                held.capped.insert(index);
            }
            Disposition::Replace => {
                held.implausible.insert(index);
            }
        }
    }
    Ok(held)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn road_table(
        rows: &[([(i32, i32); 2], u8, u8, u8, u8, u16, f64)],
        measured: Option<ArrayRef>,
        located: Option<ArrayRef>,
    ) -> Table {
        let mut fields = vec![
            Field::new("start_gx", DataType::Int32, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
            Field::new("road_class", DataType::UInt8, false),
            Field::new("oneway", DataType::UInt8, false),
            Field::new("lanes", DataType::UInt8, false),
            Field::new("built_up", DataType::UInt8, false),
            Field::new("traffic_estimated", DataType::UInt8, false),
            Field::new("aadt_light", DataType::Float64, false),
            Field::new("aadt_medium", DataType::Float64, false),
            Field::new("aadt_heavy", DataType::Float64, false),
            Field::new("aadt_moto", DataType::Float64, false),
            Field::new("cross_section_aadt", DataType::Float64, false),
            Field::new("source_id", DataType::UInt16, false),
            Field::new("length_m", DataType::Float32, false),
            Field::new("tunnel", DataType::Boolean, false),
        ];
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.0[0].0))),
            Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.0[0].1))),
            Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.0[1].0))),
            Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.0[1].1))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.1))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.2))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.3))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.4))),
            Arc::new(UInt8Array::from_iter_values(rows.iter().map(|_| 15u8))),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|r| r.6))),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|_| 0.0))),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|_| 0.0))),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|_| 0.0))),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|r| r.6))),
            Arc::new(UInt16Array::from_iter_values(rows.iter().map(|r| r.5))),
            Arc::new(Float32Array::from_iter_values(rows.iter().map(|_| 100.0))),
            Arc::new(BooleanArray::from_iter(rows.iter().map(|_| Some(false)))),
        ];
        for (name, opt) in [
            ("traffic_observation_measured", measured),
            ("traffic_observation_located", located),
        ] {
            if let Some(array) = opt {
                fields.push(Field::new(name, array.data_type().clone(), true));
                columns.push(array);
            }
        }
        Table {
            metadata: HashMap::new(),
            batches: vec![RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()],
        }
    }

    #[test]
    fn no_lane_carries_more_than_the_busiest_counted_lanes() {
        let at = |v, ow, ln| disposition(Traffic::Count, false, v, (1, ow, ln, 1), || None);
        assert_eq!(at(70_000.0, false, 2), Disposition::Replace);
        assert_eq!(at(35_000.0, false, 0), Disposition::Replace);
        assert_eq!(at(15_000.0, false, 0), Disposition::Keep);
        assert_eq!(at(80_000.0, true, 1), Disposition::Replace);
        assert_eq!(at(287_789.0, false, 1), Disposition::Replace);
        assert_eq!(at(80_000.0, true, 3), Disposition::Keep);
        assert_eq!(at(70_000.0, true, 0), Disposition::Keep);
        assert_eq!(at(80_000.0, true, 0), Disposition::Replace);
        assert_eq!(
            disposition(Traffic::Estimate, true, 70_000.0, (1, false, 2, 1), || {
                Some(5_000.0)
            }),
            Disposition::Cap
        );
        assert_eq!(
            disposition(Traffic::Estimate, false, 70_000.0, (1, false, 2, 1), || {
                Some(5_000.0)
            }),
            Disposition::Replace
        );
        assert_eq!(
            disposition(Traffic::Other, false, 287_789.0, (1, false, 1, 1), || None),
            Disposition::Keep
        );
    }

    #[test]
    fn a_count_far_under_the_model_and_an_estimate_under_the_band_take_the_model() {
        let warsaw = (2, true, 3, 2);
        assert_eq!(
            disposition(Traffic::Count, false, 32.0, warsaw, || Some(16_964.0)),
            Disposition::Replace
        );
        assert_eq!(
            disposition(Traffic::Count, false, 569.0, (0, true, 2, 1), || Some(
                15_600.0
            )),
            Disposition::Keep
        );
        assert_eq!(
            disposition(Traffic::Estimate, false, 7_346.0, (0, true, 3, 2), || Some(
                54_610.0
            )),
            Disposition::Replace
        );
        assert_eq!(
            disposition(Traffic::Estimate, false, 1_000.0, (0, true, 2, 1), || Some(
                12_816.0
            )),
            Disposition::Replace
        );
        assert_eq!(
            disposition(Traffic::Estimate, true, 25_000.0, (1, false, 0, 1), || {
                Some(5_000.0)
            }),
            Disposition::Keep
        );
        assert_eq!(
            disposition(Traffic::Count, false, 32.0, warsaw, || None),
            Disposition::Keep
        );
        assert_eq!(
            disposition(Traffic::Estimate, false, 100.0, (4, false, 2, 2), || Some(
                5_000.0
            )),
            Disposition::Keep
        );
    }

    /// Annual trees with measured + located metadata: located stations (`1102`) and located link
    /// estimates (`21`, `1041`) stay `Count`; unlocated unmeasured route medians (`1097`, `9392`)
    /// face the estimate band and replace over capacity; `guessed` class fallbacks (`9865`) cap.
    #[test]
    fn annual_measured_and_located_metadata_decides_count_vs_estimate() {
        let rows = [
            ([(0, 0), (10, 0)], 1, 1, 2, 1, 1102, 3_800.0), // 0: SA 1102 located station -> Count
            ([(100, 0), (110, 0)], 2, 0, 2, 2, 1097, 800.0), // 1: PL 1097 unlocated DW median -> Estimate
            ([(200, 0), (210, 0)], 1, 0, 2, 1, 21, 298.0), // 2: US 21 located B/E (Klondike) -> Count
            ([(300, 0), (310, 0)], 2, 0, 2, 1, 1041, 240.0), // 3: GB 1041 located prior-year -> Count
            ([(400, 0), (410, 0)], 1, 1, 1, 2, 9392, 40_000.0), // 4: JP 9392 unlocated est over cap -> Replace
            ([(500, 0), (510, 0)], 1, 0, 2, 1, 9865, 40_000.0), // 5: JP 9865 guessed over cap -> Cap
        ];
        let measured: ArrayRef = Arc::new(BooleanArray::from(vec![
            true, false, false, false, false, false,
        ]));
        let located: ArrayRef = Arc::new(BooleanArray::from(vec![
            true, false, true, true, false, false,
        ]));
        let held = held_counts_with_prediction(
            &road_table(&rows, Some(measured), Some(located)),
            &HashSet::new(),
            |m, _| Some(if m.0 < 150 { 20_000.0 } else { 2_000.0 }),
        )
        .unwrap();
        assert!(!held.implausible.contains(&0), "SA 1102 station stands");
        assert!(
            held.implausible.contains(&1),
            "PL 1097 unlocated DW replaced"
        );
        assert!(!held.implausible.contains(&2), "US 21 located B/E stands");
        assert!(
            !held.implausible.contains(&3),
            "GB 1041 located link stands"
        );
        assert!(held.implausible.contains(&4) && !held.capped.contains(&4));
        assert!(held.capped.contains(&5) && !held.implausible.contains(&5));
    }

    /// Trees without `traffic_observation_located` (frozen dev4 or earlier annual trees) keep
    /// legacy `NETWORK_ESTIMATE_SOURCE_IDS` without replacing located US/GB estimates, while
    /// `measured=true` already upgrades located stations like `1102` and malformed columns fail.
    #[test]
    fn legacy_and_partial_metadata_and_run_isolation_behave_safely() {
        let rows = [
            ([(0, 0), (10, 0)], 1, 0, 2, 1, 9865, 40_000.0),
            ([(100, 0), (110, 0)], 1, 0, 2, 1, 1050, 70_000.0),
            ([(200, 0), (210, 0)], 1, 1, 2, 1, 1102, 3_800.0),
        ];
        // Frozen dev4 (neither column): 9865 & 1050 cap, 1102 replaced as legacy estimate.
        let dev4 =
            held_counts_with_prediction(&road_table(&rows, None, None), &HashSet::new(), |_, _| {
                Some(20_000.0)
            })
            .unwrap();
        assert!(
            dev4.capped.contains(&0) && dev4.capped.contains(&1) && dev4.implausible.contains(&2)
        );

        // Annual without `located` column: measured=true upgrades SA 1102 to Count immediately.
        let m: ArrayRef = Arc::new(BooleanArray::from(vec![false, false, true]));
        let pre_loc = held_counts_with_prediction(
            &road_table(&rows, Some(m), None),
            &HashSet::new(),
            |_, _| Some(20_000.0),
        )
        .unwrap();
        assert!(!pre_loc.implausible.contains(&2));

        // Unlocated unmeasured row does not bridge a measured run; located estimate joins it.
        let run = [
            ([(0, 0), (10, 0)], 0, 1, 2, 1, 21, 40_000.0),
            ([(10, 0), (20, 0)], 0, 1, 2, 1, 21, 10_000.0),
            ([(20, 0), (30, 0)], 0, 1, 2, 1, 21, 20_000.0),
        ];
        let rm = || -> ArrayRef { Arc::new(BooleanArray::from(vec![true, false, true])) };
        let unloc = held_counts_with_prediction(
            &road_table(
                &run,
                Some(rm()),
                Some(Arc::new(BooleanArray::from(vec![true, false, true]))),
            ),
            &HashSet::new(),
            |_, _| None,
        )
        .unwrap();
        assert_eq!(
            [
                unloc.of(0).flows[0],
                unloc.of(1).flows[0],
                unloc.of(2).flows[0]
            ],
            [40_000.0, 10_000.0, 20_000.0]
        );
        let loc = held_counts_with_prediction(
            &road_table(
                &run,
                Some(rm()),
                Some(Arc::new(BooleanArray::from(vec![true, true, true]))),
            ),
            &HashSet::new(),
            |_, _| None,
        )
        .unwrap();
        assert_eq!(
            [loc.of(0).flows[0], loc.of(1).flows[0], loc.of(2).flows[0]],
            [20_000.0, 20_000.0, 20_000.0]
        );

        // Schema errors are refused loudly.
        let one = [([(0, 0), (10, 0)], 1, 1, 2, 1, 21, 10_000.0)];
        let bad_type: ArrayRef = Arc::new(UInt8Array::from(vec![1u8]));
        assert!(
            held_counts_with_prediction(
                &road_table(&one, Some(bad_type), None),
                &HashSet::new(),
                |_, _| None
            )
            .is_err()
        );
        let with_null: ArrayRef = Arc::new(BooleanArray::from(vec![None]));
        assert!(
            held_counts_with_prediction(
                &road_table(&one, Some(with_null), None),
                &HashSet::new(),
                |_, _| None
            )
            .is_err()
        );
        let loc_only: ArrayRef = Arc::new(BooleanArray::from(vec![true]));
        assert!(
            held_counts_with_prediction(
                &road_table(&one, None, Some(loc_only)),
                &HashSet::new(),
                |_, _| None
            )
            .is_err()
        );
    }

    #[test]
    fn a_cross_section_follows_its_own_carriageway() {
        let row = |vehicles: f64, cross_section: f64, oneway: bool| RowTraffic {
            flows: [vehicles, 0.0, 0.0, 0.0],
            cross_section,
            source: 23,
            estimated: 0,
            observation_estimated: 0,
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
