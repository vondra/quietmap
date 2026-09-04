//! The process-wide admin directory, exercised in its OWN process.
//!
//! `set_admin_square_directory` fills a process-wide cache that the road and
//! rail kernels read as their receiver fallback, so this test CANNOT live in
//! the lib test binary: there it would make admin visible to every
//! concurrently-running lib test, and any test computing with an
//! admin-dependent default would flip between the WORLD and the country arm
//! depending on when the fill landed — an observed flake on data-carrying
//! boxes (`none_channel_is_receiver_path_bit_identical` lost the race between
//! its two compute calls). An integration binary is a separate process, so the
//! lib tests keep their "no admin is visible" assumption and this one keeps the
//! real wiring covered.

use noise_compute::admin::{
    admin_for_latlng, admin_for_square, cell_admin_path, set_admin_square_directory, square_id,
    Continent,
};
use std::fs;

/// Dobříš (49.78 N, 14.17 E) — the z9 square the benchmarks anchor on.
fn dobris_square() -> i64 {
    square_id(grid::square_of(49.78, 14.17))
}

#[test]
fn recorded_directory_resolves_by_square_and_by_latlng() {
    let tree = tempfile::tempdir().unwrap();
    let square = dobris_square();
    let path = cell_admin_path(tree.path(), square);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut record = (square as u64).to_le_bytes().to_vec();
    record.push(Continent::Europe as u8);
    record.extend_from_slice(b"CZ");
    record.extend_from_slice(&31u16.to_le_bytes());
    fs::write(&path, record).unwrap();

    set_admin_square_directory(tree.path());

    assert_eq!(admin_for_square(square).country_code(), Some("CZ"));
    assert_eq!(admin_for_square(square).city_id, 31);
    // The same record through the popup's lat/lng entry point.
    assert_eq!(admin_for_latlng(49.78, 14.17).country_code(), Some("CZ"));
    // A square the tree does not prepare (mid-Pacific) stays unknown.
    assert_eq!(admin_for_latlng(-20.0, -140.0).country_code(), None);
}
