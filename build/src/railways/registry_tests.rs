//! The 2026 registry's US timetables: the systems the audit found missing feed the movements under
//! one source the rail model knows, and Amtrak's archive leaves MARC to MARC's own.
use super::config::RailObject;
use super::model::RailModel;
use crate::year::Manifest;
use std::path::Path;

/// 90 % of the US heavy-rail and metro rows of the 2026 city squares carried the class prior, as
/// only Amtrak's timetable was registered and its archive starts after the window (audit
/// 2026-11-12, finding 2). The systems it named are admitted GTFS objects of source 2079, which
/// the rail model bounds and owns for US rows only; one source's claims add on a shared track, so
/// Amtrak's archive leaves MARC's Penn Line to MARC's own archive of all three lines.
#[test]
fn us_commuter_and_metro_timetables_feed_the_2026_movements() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    let manifest = Manifest::read(&root.join("2026.json")).unwrap();
    let model = RailModel::read(&root.join("tables/2026/rail-model.json")).unwrap();
    let plan = manifest.railways.as_ref().unwrap();
    let us_timetables: Vec<(&str, &[String])> = plan
        .objects
        .iter()
        .filter_map(|object| match object {
            RailObject::Gtfs {
                input,
                source_id: 2079,
                country,
                excluded_agency_names,
                ..
            } if country == "US" => Some((input.as_str(), excluded_agency_names.as_slice())),
            _ => None,
        })
        .collect();
    for input in [
        "us-timetable-amtrak-2026-08",
        "us-timetable-mta-metro-north",
        "us-timetable-mta-long-island-rail-road",
        "us-timetable-mta-nyc-subway",
        "us-timetable-nj-transit-rail",
        "us-timetable-septa-regional-rail",
        "us-timetable-mbta",
        "us-timetable-metra",
        "us-timetable-cta",
        "us-timetable-caltrain",
        "us-timetable-mta-maryland-marc",
    ] {
        assert!(
            us_timetables.iter().any(|(object, _)| *object == input),
            "{input}: no US GTFS object of source 2079"
        );
        assert!(manifest.admitted(input).unwrap(), "{input}: not admitted");
    }
    let (_, amtrak_excluded) = us_timetables
        .iter()
        .find(|(object, _)| *object == "us-timetable-amtrak-2026-08")
        .unwrap();
    assert_eq!(*amtrak_excluded, ["MARC".to_owned()]);
    assert!(model.network_bounds.contains_key("2079:US"));
    assert!(model.source_applies_to_row(2079, *b"US"));
    assert!(!model.source_applies_to_row(2079, *b"CA"));
    assert!(
        model.should_overwrite(100, 2079),
        "the US feeds outrank the continental one"
    );
}
