//! Reflection regression tests.

use super::*;

#[test]
fn vector_reflection_sampler_overrides_only_enclosure() {
    use crate::types::RasterSampler;
    struct Flat;
    impl RasterSampler for Flat {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            123.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.25
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            99.0 // sentinel: must never surface through the wrapper
        }
        fn build_path_profile(
            &self,
            _: f64,
            _: f64,
            _: f64,
            _: f64,
            dist_m: f64,
            out: &mut crate::propagation::PathProfile,
        ) {
            // sentinel override: dist_m must round-trip through the
            // wrapper's forwarder (a dropped forwarder would fall back
            // to the trait default and lose the inner override).
            out.dist_m = dist_m * 2.0;
        }
    }
    // Dense block around the origin ⇒ all nine probes inside ⇒ 3 dB.
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 200.0), 12.0, ObstacleKind::Building, 0);
    let set = ObstacleSet {
        indexes: vec![std::sync::Arc::new(b.build())],
    };
    let w = VectorReflectionSampler {
        inner: &Flat,
        set: &set,
    };
    assert_eq!(w.elevation(OLAT, OLON), 123.0);
    assert_eq!(w.ground_g(OLAT, OLON), 0.25);
    assert_eq!(w.building_enclosure(OLAT, OLON), 3.0);
    let (far_lat, far_lon) = ll(5_000.0, 5_000.0);
    assert_eq!(w.building_enclosure(far_lat, far_lon), 0.0);
    // The defaultable method must forward to the INNER override, not fall
    // back to the trait default (gg review 1.4b #1).
    let mut prof = crate::propagation::PathProfile::new();
    w.build_path_profile(OLAT, OLON, OLAT, OLON, 100.0, &mut prof);
    assert_eq!(prof.dist_m, 200.0);
}
