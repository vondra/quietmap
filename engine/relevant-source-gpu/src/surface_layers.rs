//! The five surface layers this painter owns, and what each one is: the one
//! table every module indexes, so a layer cannot be a road in one file and a
//! building in the next.

use tile_painter::ground_ops::GROUND_LDEN_WEIGHTS;
use tile_painter::scatter_band::LDEN_WEIGHTS;
use tile_painter::wire_hm3::{
    SOURCE_ID_AIRCRAFT, SOURCE_ID_BUILDING, SOURCE_ID_INDUSTRIAL, SOURCE_ID_RAIL, SOURCE_ID_ROAD,
};

use crate::source_frame::PERIOD_COUNT;

/// The layers one preparation paints, in output order: the directory names
/// under `--output` and the whole vocabulary a `layers=` request may name.
pub const LAYER_NAMES: [&str; LAYER_COUNT] =
    ["road", "rail", "industrial", "building", "aircraft-ground"];
pub const LAYER_COUNT: usize = 5;
/// Airport ground ops: the one layer whose sources are dated events.
pub const GROUND_OPS_LAYER: usize = 4;
/// Building emission: the layer whose points come out of the structure table.
pub const BUILDING_LAYER: usize = 3;

pub const LAYER_SOURCE_IDS: [u8; LAYER_COUNT] = [
    SOURCE_ID_ROAD,
    SOURCE_ID_RAIL,
    SOURCE_ID_INDUSTRIAL,
    SOURCE_ID_BUILDING,
    SOURCE_ID_AIRCRAFT,
];
/// Point-grid area sources get the CPU's median footprint fill before the write.
pub const LAYER_AREA_SOURCE: [bool; LAYER_COUNT] = [false, false, true, true, false];
/// Airport ground ops accumulate event energy over n_days; the rest is steady power.
pub const LAYER_EVENT_ENERGY: [bool; LAYER_COUNT] = [false, false, false, false, true];
pub const LAYER_LDEN_WEIGHTS: [[f64; PERIOD_COUNT]; LAYER_COUNT] = [
    LDEN_WEIGHTS,
    LDEN_WEIGHTS,
    LDEN_WEIGHTS,
    LDEN_WEIGHTS,
    GROUND_LDEN_WEIGHTS,
];
