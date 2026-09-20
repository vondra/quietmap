//! Shared query fixtures and regression suites.

use super::*;
use crate::structure_test_fixture as fx;

/// Click point and its square: Prague (50.0, 14.25) is z9/276/173.
const LAT: f64 = 50.0;
const LON: f64 = 14.25;

fn prague() -> grid::Square {
    grid::square_of(LAT, LON)
}

mod layer_contract_tests;
mod reach_tests;
mod road_tests;
mod settlement_tests;
