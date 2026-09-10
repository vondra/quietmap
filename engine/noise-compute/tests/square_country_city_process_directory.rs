//! The process-wide square-country-city directory, exercised in its OWN process.
//!
//! `set_square_country_city_prepared_directory` fills a process-wide cache that the road and
//! rail kernels read as their receiver fallback, so this test CANNOT live in
//! the lib test binary: there it would make the square-country-city tree visible to every
//! concurrently-running lib test, and any test computing with an
//! country-dependent default would flip between the WORLD and the country arm
//! depending on when the fill landed — an observed flake on data-carrying
//! boxes (`none_channel_is_receiver_path_bit_identical` lost the race between
//! its two compute calls). An integration binary is a separate process, so the
//! lib tests keep their "no square-country-city tree is visible" assumption and this one keeps the
//! real wiring covered.

use grid::square_id;
use noise_compute::square_country_city::{
    cell_square_country_city_path, set_square_country_city_prepared_directory,
    square_country_city_for_latlng, square_country_city_for_square, Continent,
};
use std::{fs, path::Path, process::Command};

/// Dobříš (49.78 N, 14.17 E) — the z9 square the benchmarks anchor on.
fn dobris_square() -> i64 {
    square_id(grid::square_of(49.78, 14.17))
}

#[test]
fn recorded_directory_resolves_by_square_and_by_latlng() {
    let tree = tempfile::tempdir().unwrap();
    let square = dobris_square();
    let path = cell_square_country_city_path(tree.path(), square).unwrap();
    assert_eq!(path, tree.path().join("z9/276/174/square-country-city.bin"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut record = (square as u64).to_le_bytes().to_vec();
    record.push(Continent::Europe as u8);
    record.extend_from_slice(b"CZ");
    record.extend_from_slice(&31u16.to_le_bytes());
    fs::write(&path, record).unwrap();

    set_square_country_city_prepared_directory(tree.path());

    assert_eq!(
        square_country_city_for_square(square).country_code(),
        Some("CZ")
    );
    assert_eq!(square_country_city_for_square(square).city_id, 31);
    // The same record through the popup's lat/lng entry point.
    assert_eq!(
        square_country_city_for_latlng(49.78, 14.17).country_code(),
        Some("CZ")
    );
    // A square the tree does not prepare (mid-Pacific) stays unknown.
    assert_eq!(
        square_country_city_for_latlng(-20.0, -140.0).country_code(),
        None
    );
}

#[test]
#[ignore = "requires the project .venv geospatial Python dependencies"]
fn python_square_country_city_producer_survives_copying_only_the_z9_unit() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let produced = tempfile::tempdir().unwrap();
    let generated = Command::new(project.join(".venv/bin/python"))
        .current_dir(project.join("scripts/square-country-city"))
        .args(["-c", "import sys; from test_square_country_city import write_prepared_square_country_city_roundtrip; write_prepared_square_country_city_roundtrip(sys.argv[1])"])
        .arg(produced.path())
        .output().unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let copied = tempfile::tempdir().unwrap();
    let square = dobris_square();
    let relative = "z9/276/174/square-country-city.bin";
    fs::create_dir_all(copied.path().join("z9/276/174")).unwrap();
    fs::copy(produced.path().join(relative), copied.path().join(relative)).unwrap();
    drop(produced);
    let square_country_city =
        noise_compute::square_country_city::read_cell_square_country_city(copied.path(), square)
            .unwrap();
    assert_eq!(square_country_city.country_code(), Some("CZ"));
    assert_eq!(square_country_city.continent, Continent::Europe);
    assert_eq!(square_country_city.city_id, 31);
}
