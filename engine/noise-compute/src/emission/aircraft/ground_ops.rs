//! Ground-operations surface constants — per-kind reference speeds,
//! spectrum shapes, and SEL anchors consumed by the `airport_traffic`
//! emission kernel.

use crate::types::NUM_BANDS;

pub(crate) const SURFACE_RUNWAY_SPEED_KT: f32 = 70.0;
pub(crate) const SURFACE_TAXIWAY_SPEED_KT: f32 = 18.0;
pub(crate) const SURFACE_APRON_SPEED_KT: f32 = 12.0;
pub(crate) const GROUND_OPS_REF_OFFSET_M: f64 = 25.0;
pub(crate) const GROUND_OPS_SPEED_CLAMP_DB: f64 = 3.0;
pub(crate) const GROUND_OPS_RUNWAY_DEPARTURE_BONUS_DB: f64 = 2.0;
pub(crate) const GROUND_OPS_RUNWAY_SPECTRUM_SHAPE: [f64; NUM_BANDS] =
    [17.0, 14.0, 11.0, 8.0, 5.0, 2.0, -1.0, -5.0];
pub(crate) const GROUND_OPS_TAXI_SPECTRUM_SHAPE: [f64; NUM_BANDS] =
    [14.0, 11.0, 8.0, 5.0, 2.0, 0.0, -3.0, -7.0];
pub(crate) const GROUND_OPS_APRON_SPECTRUM_SHAPE: [f64; NUM_BANDS] =
    [12.0, 9.0, 6.0, 3.0, 1.0, -1.0, -4.0, -8.0];
