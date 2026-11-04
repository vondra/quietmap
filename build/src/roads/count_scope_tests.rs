//! Count scope, observed class status and the native prior baseline survive allocation and serialization.
use super::{
    allocation, input,
    matching::Matcher,
    spatial::RoadIndex,
    tests::{model, observation, road},
    write,
};
use arrow_array::{Float64Array, UInt8Array};

#[test]
fn every_one_way_class_halves_a_both_directions_count_but_keeps_directional_and_street_counts() {
    let model = model();
    for class in 0..13 {
        for direction in [1, 2, 3, 4] {
            let mut road = road(1);
            road.class = class;
            road.direction = direction;
            road.roundabout = direction >= 3;
            road.corridor.clear();
            assert_eq!(
                allocation::allocate(&road, &[], &model).0,
                [40.0, 5.0, 2.5, 2.5]
            );
            for basis in [1, 4] {
                road.basis = basis;
                assert_eq!(allocation::allocate(&road, &[], &model).0, road.counts);
            }
        }
    }
}

#[test]
fn reciprocal_directions_keep_their_counts_and_only_the_sum_matches_an_undivided_road() {
    let mut forward = observation();
    forward.direction = 1;
    forward.count_basis = super::observation::CountBasis::Directional;
    forward.vehicle_classes = [80.0, 10.0, 5.0, 5.0].map(Some);
    let mut reverse = forward.clone();
    reverse.identity = "reverse".into();
    reverse.direction = 2;
    reverse.vehicle_classes = [160.0, 20.0, 10.0, 10.0].map(Some);
    reverse.total = Some(200.0);
    let mut both = forward.clone();
    both.identity = "sum".into();
    both.direction = 0;
    both.target.required_oneway = Some(false);
    both.count_basis = super::observation::CountBasis::BothDirections;
    both.vehicle_classes = [240.0, 30.0, 15.0, 15.0].map(Some);
    both.total = Some(300.0);
    let matcher = Matcher::new(vec![both, forward, reverse]).unwrap();
    for (direction, total) in [(0, 300.0), (1, 100.0), (2, 200.0)] {
        let mut road = road(1);
        road.direction = direction;
        matcher.apply(&mut road, &model()).unwrap();
        assert_eq!(road.counts.iter().sum::<f64>(), total);
        assert_eq!(
            allocation::allocate(&road, &[], &model())
                .0
                .iter()
                .sum::<f64>(),
            total
        );
    }
}

#[test]
fn an_allocated_count_keeps_its_observed_truck_mix_in_the_prepared_columns() {
    let directory = tempfile::tempdir().unwrap();
    super::continuity_tests::square(
        directory.path(),
        256,
        &[super::continuity_tests::Piece {
            way: 1,
            nodes: [1, 2],
            eastings: [
                super::continuity_tests::CENTER,
                super::continuity_tests::CENTER + 10000,
            ],
            class: 2,
        }],
    );
    let batches = input::load(&directory.path().join("z9/256/255/roads.arrow")).unwrap();
    let mut roads = input::roads(&batches[0]).unwrap();
    let road = &mut roads[0];
    road.direction = 1;
    road.source_id = 21;
    road.observation_source_id = 21;
    road.observation = "measured section".into();
    road.count_origin_measured = true;
    road.counts = [80.0, 10.0, 5.0, 5.0];
    road.basis = 2;
    road.estimated = 9;
    road.observation_estimated = 9;
    let path = directory.path().join("allocated.arrow");
    write::stage(
        &path,
        &batches,
        &roads,
        &RoadIndex::new(Vec::new()),
        &model(),
    )
    .unwrap();
    let mut reader =
        arrow_ipc::reader::FileReader::try_new(std::fs::File::open(path).unwrap(), None).unwrap();
    let batch = reader.next().unwrap().unwrap();
    assert_eq!(
        input::column::<UInt8Array>(&batch, "traffic_estimated")
            .unwrap()
            .value(0),
        15
    );
    assert_eq!(
        input::column::<UInt8Array>(&batch, "traffic_observation_estimated")
            .unwrap()
            .value(0),
        9
    );
    assert_eq!(
        input::column::<Float64Array>(&batch, "aadt_medium")
            .unwrap()
            .value(0),
        5.0
    );
    assert_eq!(
        input::column::<Float64Array>(&batch, "aadt_heavy")
            .unwrap()
            .value(0),
        2.5
    );
}

#[test]
fn native_prior_normalization_comes_from_the_same_selected_preparation_model() {
    let mut model = model();
    assert_eq!(
        model.native_class_priors(),
        [[3000.0, 2061.0, 6445.0], [1506.0, 1002.0, 2562.0]]
    );
    model.carriageways.clear();
    model.world[3] = [3600.0, 100.0, 200.0, 100.0];
    assert_eq!(model.native_class_priors()[0], [4000.0; 3]);
}

#[test]
fn an_uncounted_roundabout_uses_the_same_one_way_prior_as_an_ordinary_carriageway() {
    let mut ordinary = road(1);
    ordinary.class = 3;
    ordinary.source_id = 0;
    let mut roundabout = ordinary.clone();
    roundabout.direction = 3;
    roundabout.roundabout = true;
    let model = model();
    assert_eq!(
        allocation::allocate(&roundabout, &[], &model),
        allocation::allocate(&ordinary, &[], &model)
    );
}

#[test]
fn historical_weekday_fit_replaces_the_conversion_fallback_but_never_scales_annual_counts() {
    use super::observation::TimeBasis;
    for (basis, expected) in [
        (TimeBasis::WeekdayDaily, 90.0),
        (TimeBasis::AnnualDaily, 100.0),
    ] {
        let mut observed = observation();
        observed.time_basis = basis;
        observed.daily_factor = if basis == TimeBasis::WeekdayDaily {
            0.8
        } else {
            1.0
        };
        observed.vehicle_classes = [80.0, 10.0, 10.0, 0.0].map(Some);
        let matcher = Matcher::new(vec![observed]).unwrap();
        let mut model = model();
        model.weekday_to_annual_factor = Some(0.9);
        let mut road = road(1);
        matcher.apply(&mut road, &model).unwrap();
        assert_eq!(road.counts.iter().sum::<f64>(), expected);
    }
}
