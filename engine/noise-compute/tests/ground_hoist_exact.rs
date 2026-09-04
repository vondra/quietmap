//! Bit-exact regression oracle for the CPU ground-algebra hoist.
//!
//! The reference below is a deliberately independent copy of the pre-hoist
//! scalar implementation. It must stay a plain per-band calculation so this
//! test can detect an association, libm, NaN, or edge-behaviour change in the
//! cached implementation.

use noise_compute::constants::{BAND_FREQ, GROUND_HARD_FLOOR_DB, P_FAV, SPEED_OF_SOUND};
use noise_compute::propagation::iso9613::{
    cnossos_ground_homogeneous_atten_bands, cnossos_ground_homogeneous_atten_db,
    ground_atten_bands, ground_atten_db, GroundPath, CNOSSOS_GROUND_ALPHA0,
    CNOSSOS_GROUND_DELTA_ZT_COEFF,
};
use noise_compute::types::NUM_BANDS;

/// Untouched pre-hoist CNOSSOS state calculation used only as a test oracle.
fn reference_ground_state_db(band: usize, path: GroundPath, favourable: bool) -> f64 {
    let g_path = path.ground_path_g;
    if g_path == 0.0 {
        return GROUND_HARD_FLOOR_DB;
    }

    let (zs_h, zr_h) = (path.zs_h_m, path.zr_h_m);
    let height_sum = zs_h + zr_h;
    let test_form_h = path.dp_m / (30.0 * height_sum);
    let g_prime = if test_form_h <= 1.0 {
        g_path * test_form_h + path.source_ground_g * (1.0 - test_form_h)
    } else {
        g_path
    };
    let (zs, zr, gw) = if favourable {
        let delta_zt = CNOSSOS_GROUND_DELTA_ZT_COEFF * path.dp_m / height_sum;
        let dp_sq_half = path.dp_m * path.dp_m * 0.5;
        let zs_f =
            zs_h + CNOSSOS_GROUND_ALPHA0 * (zs_h / height_sum).powi(2) * dp_sq_half + delta_zt;
        let zr_f =
            zr_h + CNOSSOS_GROUND_ALPHA0 * (zr_h / height_sum).powi(2) * dp_sq_half + delta_zt;
        (zs_f, zr_f, g_path)
    } else {
        (zs_h, zr_h, g_prime)
    };

    let f = BAND_FREQ[band];
    let k = 2.0 * std::f64::consts::PI * f / SPEED_OF_SOUND;
    let gw13 = gw.powf(1.3);
    let gw26 = gw13 * gw13;
    let w =
        0.0185 * f.powf(2.5) * gw26 / (f.powf(1.5) * gw26 + 1.3e3 * f.powf(0.75) * gw13 + 1.16e6);
    let wd = w * path.dp_m;
    let cf = path.dp_m * (1.0 + 3.0 * wd * (-wd.sqrt()).exp()) / (1.0 + wd);
    let image_product = (zs * zs - (2.0 * cf / k).sqrt() * zs + cf / k)
        * (zr * zr - (2.0 * cf / k).sqrt() * zr + cf / k);
    let analytic = -10.0 * (4.0 * k * k / (path.dp_m * path.dp_m) * image_product).log10();
    analytic.max(GROUND_HARD_FLOOR_DB * (1.0 - g_prime))
}

fn reference_homogeneous_ground_db(band: usize, path: GroundPath) -> f64 {
    reference_ground_state_db(band, path, false)
}

fn reference_ground_db(band: usize, path: GroundPath) -> f64 {
    if path.ground_path_g == 0.0 {
        return GROUND_HARD_FLOOR_DB;
    }
    let homogeneous = reference_ground_state_db(band, path, false);
    let favourable = reference_ground_state_db(band, path, true);
    let energy = P_FAV * 10.0_f64.powf(-favourable / 10.0)
        + (1.0 - P_FAV) * 10.0_f64.powf(-homogeneous / 10.0);
    -10.0 * energy.log10()
}

fn assert_same_bits(
    tag: &str,
    path_index: usize,
    band: usize,
    path: GroundPath,
    got: f64,
    expected: f64,
) {
    assert_eq!(
        got.to_bits(),
        expected.to_bits(),
        "{tag}: path={path_index}, band={band}, path={path:?}, got {got:?} ({:#018x}), expected {expected:?} ({:#018x})",
        got.to_bits(),
        expected.to_bits()
    );
}

#[test]
fn hoisted_ground_matches_untouched_reference_on_corner_grid() {
    // Stay inside GroundPath::new's valid post-normalization domain here so
    // every grid point proves a distinct production value. Raw constructor
    // bypasses, including signed zero and nonfinite inputs, live below.
    let distances_m = [
        1e-6,
        0.01,
        1.0,
        29.999,
        30.0,
        300.0,
        3_000.0,
        10_000.0,
        1_000_000.0,
    ];
    let source_heights_m = [0.05, 0.5, 1.0, 4.0, 105.0, 15_615.5];
    let receiver_heights_m = [0.05, 0.5, 1.0, 4.0, 105.0, 414.0, 15_615.5];
    let path_ground = [0.0, -0.0, 1e-12, 0.01, 0.5, 0.99, 1.0];
    let source_ground = [0.0, -0.0, 0.5, 1.0];

    let mut paths = Vec::new();
    for &distance_m in &distances_m {
        for &source_height_m in &source_heights_m {
            for &receiver_height_m in &receiver_heights_m {
                for &ground_path_g in &path_ground {
                    for &source_ground_g in &source_ground {
                        paths.push(GroundPath::new(
                            distance_m,
                            source_height_m,
                            receiver_height_m,
                            ground_path_g,
                            source_ground_g,
                        ));
                    }
                }
            }
        }
    }
    assert_eq!(paths.len(), 10_584);

    for (path_index, path) in paths.into_iter().enumerate() {
        let actual_ground_bands = ground_atten_bands(path);
        let actual_homogeneous_bands = cnossos_ground_homogeneous_atten_bands(path);
        for band in 0..NUM_BANDS {
            assert_same_bits(
                "ground bands",
                path_index,
                band,
                path,
                actual_ground_bands[band],
                reference_ground_db(band, path),
            );
            assert_same_bits(
                "scalar ground",
                path_index,
                band,
                path,
                ground_atten_db(band, path),
                reference_ground_db(band, path),
            );
            assert_same_bits(
                "homogeneous bands",
                path_index,
                band,
                path,
                actual_homogeneous_bands[band],
                reference_homogeneous_ground_db(band, path),
            );
            assert_same_bits(
                "scalar homogeneous",
                path_index,
                band,
                path,
                cnossos_ground_homogeneous_atten_db(band, path),
                reference_homogeneous_ground_db(band, path),
            );
        }
    }
}

#[test]
fn hoisted_ground_preserves_nonfinite_and_unclamped_reference_edges() {
    let paths = [
        GroundPath {
            dp_m: 0.0,
            zs_h_m: 1.0,
            zr_h_m: 4.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: -1.0,
            zs_h_m: 1.0,
            zr_h_m: 4.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: -1.0,
            zr_h_m: 0.5,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: -0.0,
            zr_h_m: -0.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: f64::NAN,
            zs_h_m: 1.0,
            zr_h_m: 4.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: f64::NAN,
            zr_h_m: 4.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: f64::INFINITY,
            zr_h_m: 4.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: f64::INFINITY,
            zs_h_m: 1.0,
            zr_h_m: 4.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: 0.0,
            zr_h_m: 0.0,
            ground_path_g: 0.5,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: 1.0,
            zr_h_m: 1.0,
            ground_path_g: f64::NAN,
            source_ground_g: 0.5,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: 1.0,
            zr_h_m: 1.0,
            ground_path_g: 0.5,
            source_ground_g: f64::NAN,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: 1.0,
            zr_h_m: 1.0,
            ground_path_g: -1.0,
            source_ground_g: 2.0,
        },
        GroundPath {
            dp_m: 1.0,
            zs_h_m: 1.0,
            zr_h_m: 1.0,
            ground_path_g: 2.0,
            source_ground_g: -1.0,
        },
    ];

    for (path_index, path) in paths.into_iter().enumerate() {
        let actual_ground_bands = ground_atten_bands(path);
        let actual_homogeneous_bands = cnossos_ground_homogeneous_atten_bands(path);
        for band in 0..NUM_BANDS {
            assert_same_bits(
                "edge ground bands",
                path_index,
                band,
                path,
                actual_ground_bands[band],
                reference_ground_db(band, path),
            );
            assert_same_bits(
                "edge homogeneous bands",
                path_index,
                band,
                path,
                actual_homogeneous_bands[band],
                reference_homogeneous_ground_db(band, path),
            );
            assert_same_bits(
                "edge scalar ground",
                path_index,
                band,
                path,
                ground_atten_db(band, path),
                reference_ground_db(band, path),
            );
            assert_same_bits(
                "edge scalar homogeneous",
                path_index,
                band,
                path,
                cnossos_ground_homogeneous_atten_db(band, path),
                reference_homogeneous_ground_db(band, path),
            );
        }
    }
}
