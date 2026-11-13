//! Every year's registry has the US timetables: the systems the audit found missing feed its
//! movements under one source the rail model knows, and Amtrak's archive leaves MARC to MARC's own.
use super::config::RailObject;
use super::model::RailModel;
use crate::year::Manifest;
use std::path::Path;

/// Publishers of systems whose track carried the class prior where no timetable fed them.
const AUDITED_US_PUBLISHERS: [&str; 11] = [
    "Amtrak",
    "Metro-North Railroad",
    "Metropolitan Transit Authority (MTA) (Long Island Rail Road)",
    "Metropolitan Transit Authority (MTA) (NYC Subway)",
    "New Jersey Transit (NJ Transit) (Rail)",
    "Southeastern Pennsylvania Transportation Authority (SEPTA) (Rail)",
    "Massachusetts Bay Transportation Authority (MBTA)",
    "Metra",
    "Chicago Transit Authority (CTA)",
    "Caltrain",
    "Maryland Transit Administration (MARC Train)",
];

/// 90 % of the US heavy-rail and metro rows of the 2026 city squares carried the class prior, as
/// only Amtrak's timetable was registered and its archive starts after the window (audit
/// 2026-11-12, finding 2); the 2024 registry repeated it with Amtrak's 2024 archive alone. In every
/// year's manifest the systems it named are admitted GTFS objects of source 2079, which the year's
/// rail model bounds and owns for US rows only; one source's claims add on a shared track, so
/// Amtrak's archive leaves MARC's Penn Line to MARC's own archive of all three lines.
#[test]
fn us_commuter_and_metro_timetables_feed_every_years_movements() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    let mut years: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let year = name.strip_suffix(".json")?;
            year.parse::<u16>().is_ok().then(|| year.to_owned())
        })
        .collect();
    years.sort();
    assert!(years.len() >= 2, "years: {years:?}");
    for year in years {
        let manifest = Manifest::read(&root.join(format!("{year}.json"))).unwrap();
        let model = RailModel::read(&root.join(format!("tables/{year}/rail-model.json"))).unwrap();
        let plan = manifest.railways.as_ref().unwrap();
        let us_timetables: Vec<(&str, &str, &[String])> = plan
            .objects
            .iter()
            .filter_map(|object| match object {
                RailObject::Gtfs {
                    input,
                    source_id: 2079,
                    country,
                    excluded_agency_names,
                    ..
                } if country == "US" => {
                    let publisher = &manifest.inputs.iter().find(|i| &i.id == input)?.publisher;
                    Some((
                        input.as_str(),
                        publisher.as_str(),
                        excluded_agency_names.as_slice(),
                    ))
                }
                _ => None,
            })
            .collect();
        for publisher in AUDITED_US_PUBLISHERS {
            let admitted = us_timetables
                .iter()
                .filter(|(_, object_publisher, _)| *object_publisher == publisher)
                .any(|(input, _, _)| manifest.admitted(input).unwrap());
            assert!(
                admitted,
                "{year}: no admitted US GTFS object of source 2079 from {publisher}"
            );
        }
        for (input, _, excluded) in us_timetables
            .iter()
            .filter(|(_, publisher, _)| *publisher == "Amtrak")
        {
            assert!(excluded.contains(&"MARC".to_owned()), "{year}: {input}");
        }
        assert!(model.network_bounds.contains_key("2079:US"), "{year}");
        assert!(model.source_applies_to_row(2079, *b"US"), "{year}");
        assert!(!model.source_applies_to_row(2079, *b"CA"), "{year}");
        assert!(
            model.should_overwrite(100, 2079),
            "{year}: the US feeds outrank the continental one"
        );
    }
}
