//! V2's single per-node attenuation composition.

use crate::types::NUM_BANDS;

#[derive(Debug, Clone, Copy, Default)]
pub struct NodePathTerms {
    pub atmospheric_db: [f64; NUM_BANDS],
    pub ground_db: [f64; NUM_BANDS],
    pub terrain_db: [f64; NUM_BANDS],
    pub screening_db: [f64; NUM_BANDS],
    pub vegetation_db: [f64; NUM_BANDS],
    pub meteo_db: [f64; NUM_BANDS],
    /// N-11 applies the terrain/screen composite only on a winning barrier path.
    pub barrier_present: bool,
}

#[must_use]
pub fn point_divergence_db(exact_slant_m: f64) -> f64 {
    20.0 * exact_slant_m.max(1.0).log10() + 11.0
}

/// N-10/N-11: complete per-band node attenuation. The barrier composite
/// replaces ground; it is never added to it.
#[must_use]
pub fn evaluate_node_attenuation_bands(
    exact_slant_m: f64,
    terms: NodePathTerms,
) -> [f64; NUM_BANDS] {
    let divergence_db = point_divergence_db(exact_slant_m);
    std::array::from_fn(|band| {
        let ground_or_barrier_db = if terms.barrier_present {
            terms.ground_db[band].max(terms.terrain_db[band] + terms.screening_db[band])
        } else {
            terms.ground_db[band]
        };
        divergence_db
            + terms.atmospheric_db[band]
            + ground_or_barrier_db
            + terms.vegetation_db[band]
            + terms.meteo_db[band]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn barrier_replaces_ground() {
        let terms = NodePathTerms {
            atmospheric_db: [0.5; NUM_BANDS],
            ground_db: [-3.0; NUM_BANDS],
            terrain_db: [2.0; NUM_BANDS],
            screening_db: [4.0; NUM_BANDS],
            vegetation_db: [1.0; NUM_BANDS],
            meteo_db: [0.5; NUM_BANDS],
            barrier_present: true,
        };
        assert_eq!(
            evaluate_node_attenuation_bands(100.0, terms),
            [59.0; NUM_BANDS]
        );
    }

    #[test]
    fn composite_is_selected_independently_per_band() {
        let terms = NodePathTerms {
            ground_db: [3.0, 7.0, 3.0, 7.0, 3.0, 7.0, 3.0, 7.0],
            terrain_db: [1.0; NUM_BANDS],
            screening_db: [5.0; NUM_BANDS],
            barrier_present: true,
            ..NodePathTerms::default()
        };
        let got = evaluate_node_attenuation_bands(1.0, terms);
        assert_eq!(got, [17.0, 18.0, 17.0, 18.0, 17.0, 18.0, 17.0, 18.0]);
    }

    #[test]
    fn no_barrier_keeps_ground_even_if_diagnostic_terms_are_nonzero() {
        let terms = NodePathTerms {
            ground_db: [3.0; NUM_BANDS],
            terrain_db: [20.0; NUM_BANDS],
            screening_db: [20.0; NUM_BANDS],
            ..NodePathTerms::default()
        };
        assert_eq!(
            evaluate_node_attenuation_bands(1.0, terms),
            [14.0; NUM_BANDS]
        );
    }
}
