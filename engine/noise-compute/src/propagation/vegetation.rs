//! ISO 9613-2:2024 Annex A.2.2 — vegetation attenuation.

use crate::constants::*;
use crate::types::NUM_BANDS;

/// Compute vegetation attenuation per band.
/// depth_m = cumulative forest depth along source-receiver path.
pub fn vegetation_attenuation(depth_m: f64) -> [f64; NUM_BANDS] {
    let mut atten = [0.0f64; NUM_BANDS];
    if depth_m <= 0.0 {
        return atten;
    }

    for i in 0..NUM_BANDS {
        atten[i] = (ALPHA_VEG[i] * depth_m).min(MAX_VEG_ATTEN[i]);
    }
    atten
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_vegetation() {
        let a = vegetation_attenuation(0.0);
        assert_eq!(a, [0.0; NUM_BANDS]);
    }

    // Expected values are ISO 9613-2 × 0.5 Central Europe calibration (see constants.rs).

    #[test]
    fn test_100m_forest() {
        let a = vegetation_attenuation(100.0);
        assert!((a[4] - 3.0).abs() < 0.01);
        assert!((a[7] - 6.0).abs() < 0.01);
    }

    #[test]
    fn test_per_band_cap() {
        // 500 m exceeds the 200 m effective-depth ceiling → every band clamps to MAX_VEG_ATTEN.
        let a = vegetation_attenuation(500.0);
        assert_eq!(a[7], 12.0);
        assert_eq!(a[0], 2.0);
        assert_eq!(a[4], 6.0);
    }
}
