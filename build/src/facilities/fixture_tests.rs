//! Optional real-input check exercises the native consumers, including WKB decoding and emission.

use crate::{
    dev4::Square,
    facilities::Dataset,
    sources::{industry, leisure},
    year::YearInputs,
};
use std::path::Path;

#[test]
#[ignore = "requires explicit fixture manifest, source root and square selection"]
fn native_facility_readers_accept_prepared_fixtures() {
    let manifest = std::env::var("QM_FACILITY_TEST_MANIFEST").unwrap();
    let sources = std::env::var("QM_FACILITY_TEST_SOURCES").unwrap();
    let selection = std::env::var("QM_FACILITY_TEST_SQUARES").unwrap();
    let inputs = YearInputs::read(Path::new(&manifest), Path::new(&sources)).unwrap();
    let dataset = Dataset::open(&inputs).unwrap();
    let mut counts = [0_usize; 2];
    for square in selection.split(',') {
        let (x, y) = square.split_once(':').unwrap();
        let square = Square {
            x: x.parse().unwrap(),
            y: y.parse().unwrap(),
        };
        let industrial = industry::read_rows(&dataset, square).unwrap();
        let leisure = leisure::read_rows(&dataset, square).unwrap();
        counts[0] += industrial.len();
        counts[1] += leisure.len();
        let mut joins = crate::sources::facilities::FacilityJoins::default();
        for row in &industrial {
            joins.add_row(
                row.source_type,
                (&row.osm_kind, row.osm_id),
                row.centroid,
                &row.ring,
                &row.tags,
            );
        }
        for row in industrial {
            let emission = industry::row_emission(&row, &joins);
            if row.suppressed {
                assert!(emission.is_none());
            }
            if let Some(emission) = emission {
                assert!(emission.sound.day_dba.is_finite());
            }
        }
        for row in leisure {
            if let Some((sound, _)) = leisure::row_emission(&row) {
                assert!(sound.day_dba.is_finite());
            }
        }
    }
    eprintln!(
        "native facility readers: {} industry rows, {} leisure rows",
        counts[0], counts[1]
    );
}
