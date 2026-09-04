//! CNOSSOS-EU road emission model (Annex II).
//!
//! Per-band emission for 4 vehicle categories:
//!   cat1 (light), cat2 (medium), cat3 (heavy), cat4b (motorcycles)
//!
//! Formula per vehicle:
//!   L_WR,i = A_R,i + B_R,i × log₁₀(v/70)  (rolling)
//!   L_WP,i = A_P,i + B_P,i × (v-70)/70     (propulsion)
//!   L_W,i  = 10×log₁₀(10^(L_WR/10) + 10^(L_WP/10))
//!
//! Line source power per meter:
//!   L_W'/m,i = L_W,i + 10×log₁₀(Q/(1000·v))   where Q = veh/h, v = km/h
//!
//! The `1/(1000·v)` term converts flow + speed into vehicle density per metre.

use crate::constants::*;
use crate::propagation::iso9613::fast_exp_f64;
use crate::types::NUM_BANDS;

/// CNOSSOS coefficients per vehicle category.
struct CnossosCoeffs {
    a_r: Option<[f64; NUM_BANDS]>,
    b_r: Option<[f64; NUM_BANDS]>,
    a_p: [f64; NUM_BANDS],
    b_p: [f64; NUM_BANDS],
}

const CAT1: CnossosCoeffs = CnossosCoeffs {
    a_r: Some([83.1, 89.2, 87.7, 93.1, 100.1, 96.7, 86.8, 76.2]),
    b_r: Some([30.0, 41.5, 38.9, 25.7, 32.5, 37.2, 39.0, 40.0]),
    a_p: [97.9, 92.5, 90.7, 87.2, 84.7, 88.0, 84.4, 77.1],
    b_p: [-1.3, 7.2, 7.7, 8.0, 8.0, 8.0, 8.0, 8.0],
};

const CAT2: CnossosCoeffs = CnossosCoeffs {
    a_r: Some([88.7, 93.2, 95.7, 100.9, 101.7, 95.1, 87.8, 83.6]),
    b_r: Some([30.0, 35.8, 32.6, 23.8, 30.1, 36.2, 38.3, 40.1]),
    a_p: [105.5, 100.2, 100.5, 98.7, 101.0, 97.8, 91.2, 85.0],
    b_p: [-1.9, 4.7, 6.4, 6.5, 6.5, 6.5, 6.5, 6.5],
};

const CAT3: CnossosCoeffs = CnossosCoeffs {
    a_r: Some([91.7, 96.2, 98.2, 104.9, 105.1, 98.5, 91.1, 85.6]),
    b_r: Some([30.0, 33.5, 31.3, 25.4, 31.8, 37.1, 38.6, 40.6]),
    a_p: [108.8, 104.2, 103.5, 102.9, 102.6, 98.5, 93.8, 87.5],
    b_p: [0.0, 3.0, 4.6, 5.0, 5.0, 5.0, 5.0, 5.0],
};

const CAT4B: CnossosCoeffs = CnossosCoeffs {
    a_r: None,
    b_r: None,
    a_p: [99.9, 101.9, 96.7, 94.4, 95.2, 94.7, 92.1, 88.6],
    b_p: [3.2, 5.9, 11.9, 11.6, 11.5, 12.6, 11.1, 12.0],
};

/// Traffic flow for one vehicle category.
pub struct CategoryFlow {
    pub q_per_hour: f64,
    pub speed_kmh: f64,
    pub category: VehicleCategory,
}

#[derive(Debug, Clone, Copy)]
pub enum VehicleCategory {
    Light,
    Medium,
    Heavy,
    Motorcycle,
}

impl VehicleCategory {
    fn coeffs(&self) -> &'static CnossosCoeffs {
        match self {
            Self::Light => &CAT1,
            Self::Medium => &CAT2,
            Self::Heavy => &CAT3,
            Self::Motorcycle => &CAT4B,
        }
    }
}

/// Single-vehicle emission per band in linear energy.
///
/// Surface correction is applied to rolling noise BEFORE combining with propulsion.
/// Keeping the result in energy avoids a hot dB -> energy round-trip in
/// `line_source_emission()`.
fn vehicle_emission_energy_bands(
    category: VehicleCategory,
    speed: f64,
    surface_corr: f64,
) -> [f64; NUM_BANDS] {
    let c = category.coeffs();
    let v = speed.clamp(20.0, 130.0);
    let log_ratio = (v / V_REF_ROAD).log10();
    let speed_delta = (v - V_REF_ROAD) / V_REF_ROAD;
    let db_to_ln = std::f64::consts::LN_10 * 0.1;

    let mut bands = [0.0f64; NUM_BANDS];
    if let (Some(a_r), Some(b_r)) = (&c.a_r, &c.b_r) {
        for i in 0..NUM_BANDS {
            let l_wp = c.a_p[i] + c.b_p[i] * speed_delta;
            let l_wr = a_r[i] + b_r[i] * log_ratio + surface_corr; // surfCorr to rolling only
            bands[i] = fast_exp_f64(l_wr * db_to_ln) + fast_exp_f64(l_wp * db_to_ln);
        }
    } else {
        // Per-band emission: `i` indexes the parallel coefficient arrays
        // (`a_p`/`b_p`) and writes `bands[i]`; kept as an index loop (a multi-array
        // zip is no clearer) and the fast_exp_f64 sum order is byte-parity.
        #[allow(clippy::needless_range_loop)]
        for i in 0..NUM_BANDS {
            let l_wp = c.a_p[i] + c.b_p[i] * speed_delta;
            bands[i] = fast_exp_f64(l_wp * db_to_ln); // propulsion-only categories: no surface correction
        }
    }
    bands
}

/// Compute line source emission [dB/m] for a mix of vehicle categories.
pub fn line_source_emission(flows: &[CategoryFlow], surface_corr_db: f64) -> [f64; NUM_BANDS] {
    let mut total = [f64::NEG_INFINITY; NUM_BANDS];

    for flow in flows {
        if flow.q_per_hour <= 0.0 {
            continue;
        }

        let speed = match flow.category {
            VehicleCategory::Heavy => flow.speed_kmh.min(HEAVY_SPEED_CAP),
            _ => flow.speed_kmh,
        };

        // Surface correction applied inside vehicle_emission_energy_bands (to rolling only,
        // before combining with propulsion).
        let bands = vehicle_emission_energy_bands(flow.category, speed, surface_corr_db);

        // CNOSSOS: density = Q / (1000 × v)  where Q = veh/h, v = km/h
        let density = flow.q_per_hour / (1000.0 * speed);

        for i in 0..NUM_BANDS {
            // Energy sum: density × vehicle_energy
            let energy = density * bands[i];
            if total[i] == f64::NEG_INFINITY {
                total[i] = energy;
            } else {
                total[i] += energy;
            }
        }
    }
    // Convert energy sums back to dB, in place per band. Kept as an index loop
    // (rather than `iter_mut()`) verbatim from the byte-exact emission kernel —
    // this output feeds the popup/pipeline parity, so the form stays frozen.
    #[allow(clippy::needless_range_loop)]
    for i in 0..NUM_BANDS {
        total[i] = if total[i] > 0.0 {
            10.0 * total[i].log10()
        } else {
            f64::NEG_INFINITY
        };
    }
    total
}

/// Build flows from AADT + speed + period distribution.
pub fn build_period_flows(
    aadt_light: f64,
    aadt_medium: f64,
    aadt_heavy: f64,
    aadt_moto: f64,
    speed_kmh: f64,
    period_pct: f64,
    period_hours: f64,
) -> Vec<CategoryFlow> {
    let q = period_pct / period_hours;
    vec![
        CategoryFlow {
            q_per_hour: aadt_light * q,
            speed_kmh,
            category: VehicleCategory::Light,
        },
        CategoryFlow {
            q_per_hour: aadt_medium * q,
            speed_kmh,
            category: VehicleCategory::Medium,
        },
        CategoryFlow {
            q_per_hour: aadt_heavy * q,
            speed_kmh,
            category: VehicleCategory::Heavy,
        },
        CategoryFlow {
            q_per_hour: aadt_moto * q,
            speed_kmh,
            category: VehicleCategory::Motorcycle,
        },
    ]
}

pub struct TimeDist {
    pub day_pct: f64,
    pub evening_pct: f64,
    pub night_pct: f64,
}
pub const TIME_DIST_MOTORWAY: TimeDist = TimeDist {
    day_pct: 0.65,
    evening_pct: 0.20,
    night_pct: 0.15,
};
pub const TIME_DIST_URBAN: TimeDist = TimeDist {
    day_pct: 0.70,
    evening_pct: 0.18,
    night_pct: 0.12,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn test_k1_road_emission() {
        let q_day = 10000.0 * 0.70 / 12.0;
        let flows = vec![CategoryFlow {
            q_per_hour: q_day,
            speed_kmh: 50.0,
            category: VehicleCategory::Light,
        }];
        let bands = line_source_emission(&flows, 0.0);
        let aw = a_weighted_total(&bands);
        assert!(
            (aw - 79.11).abs() < 0.15,
            "K1: expected 79.11, got {:.2}",
            aw
        );
    }

    #[test]
    fn test_k2_heavy_cobblestone() {
        let q_day = 500.0 * 0.70 / 12.0;
        let flows = vec![CategoryFlow {
            q_per_hour: q_day,
            speed_kmh: 80.0,
            category: VehicleCategory::Heavy,
        }];
        let bands = line_source_emission(&flows, 4.0);
        let aw = a_weighted_total(&bands);
        assert!(
            (aw - 80.07).abs() < 0.15,
            "K2: expected 80.07, got {:.2}",
            aw
        );
    }

    #[test]
    fn test_surface_only_rolling() {
        let flows = vec![CategoryFlow {
            q_per_hour: 100.0,
            speed_kmh: 50.0,
            category: VehicleCategory::Motorcycle,
        }];
        let without = line_source_emission(&flows, 0.0);
        let with = line_source_emission(&flows, 4.0);
        for i in 0..NUM_BANDS {
            assert!(
                (without[i] - with[i]).abs() < 0.01,
                "Band {}: surfCorr affected moto",
                i
            );
        }
    }

    #[test]
    fn test_low_speed_light_vehicle_bounded() {
        // Lock CNOSSOS rolling-formula output at v=20 km/h (the extractor's
        // service/track default). Rolling drops ~16 dB below the 70 km/h
        // reference; total emission stays bounded. Any refactor that unbounds
        // this should break this test.
        let flows = vec![CategoryFlow {
            q_per_hour: 100.0,
            speed_kmh: 20.0,
            category: VehicleCategory::Light,
        }];
        let aw = a_weighted_total(&line_source_emission(&flows, 0.0));
        assert!(
            (aw - 66.17).abs() < 0.15,
            "low-speed light at 20 km/h: expected 66.17, got {:.2}",
            aw
        );
    }

    #[test]
    fn test_heavy_speed_cap() {
        let f120 = vec![CategoryFlow {
            q_per_hour: 100.0,
            speed_kmh: 120.0,
            category: VehicleCategory::Heavy,
        }];
        let f80 = vec![CategoryFlow {
            q_per_hour: 100.0,
            speed_kmh: 80.0,
            category: VehicleCategory::Heavy,
        }];
        let em120 = line_source_emission(&f120, 0.0);
        let em80 = line_source_emission(&f80, 0.0);
        for i in 0..NUM_BANDS {
            assert!(
                (em120[i] - em80[i]).abs() < 0.01,
                "Band {}: heavy cap failed",
                i
            );
        }
    }
}
