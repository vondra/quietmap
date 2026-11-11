//! The popup's façade ranking skips by ceiling only what cannot rank.

use super::*;
use crate::candidates::{AttributeRef, DisplayRef, SourceAttribute};
use crate::selection::SplitMix;
use physics::bands::{BANDS, energy};
use tiles::sources::Layer;

/// The sources skipped by their ceiling are never among the ranked: the same as bounding every
/// candidate, points and lines near and far, of many powers, more than and fewer than the limit,
/// equal bounds at the limit to the earlier place.
#[test]
fn the_ranking_by_ceiling_equals_bounding_every_candidate() {
    let mut draw = SplitMix(7);
    let mut uniform = || (draw.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let attributes_list: Vec<SourceAttribute> = (0..40)
        .map(|_| {
            let level = 40.0 + 50.0 * uniform();
            let energy = std::array::from_fn(|period| {
                std::array::from_fn(|band| energy(level - 3.0 * (period + band % 3) as f64))
            });
            SourceAttribute {
                layer: Layer::Road,
                height_m: 0.5,
                ground_percent: 0,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: 0,
                energy,
                lden_power: lden_energy(&energy.map(|bands: [f64; BANDS]| bands.iter().sum())),
            }
        })
        .collect();
    let mut attributes = Attributes::default();
    attributes.push(attributes_list);
    for count in [30, 3_000] {
        let candidates: Vec<Candidate> = (0..count)
            .map(|_| {
                let line = uniform() < 0.6;
                let range = if uniform() < 0.5 { 200.0 } else { 8_000.0 };
                let a = [range * (uniform() - 0.5), range * (uniform() - 0.5)];
                let length = if line { 300.0 * uniform() } else { 0.0 };
                let heading = 6.3 * uniform();
                let b = [a[0] + length * heading.cos(), a[1] + length * heading.sin()];
                Candidate {
                    layer: Layer::Road,
                    line,
                    attribute: AttributeRef {
                        list: 0,
                        index: (40.0 * uniform()) as u32,
                    },
                    ends_m: [a, b],
                    deck_m: None,
                    distance_m: 0.0,
                    bound: [0.0; PERIODS],
                    order: 0.0,
                    group_key: 0,
                    display: DisplayRef {
                        ring: 0,
                        tile: 0,
                        attribute: 0,
                    },
                }
            })
            .collect();
        // Every piece twice: equal bounds straddle the limit, and the earlier place ranks.
        let candidates: Vec<&Candidate> = candidates.iter().chain(&candidates).collect();
        let facades: Vec<Facade> = [[0.0, 0.0], [12.0, 0.0], [12.0, 9.0], [0.0, 9.0]]
            .into_iter()
            .enumerate()
            .map(|(index, position)| Facade {
                position,
                outward_bearing_deg: 0.0,
                index,
                altitude_m: 4.0,
                reflection_db: if index == 2 { 2.5 } else { 0.0 },
            })
            .collect();
        let gain = receiver_bound([0.3, 0.5, 0.6], 2.5, *physics::atmosphere::ALPHA_DB_PER_KM);
        let mut every: Vec<(f64, usize)> = (candidates.iter().enumerate())
            .map(|(index, candidate)| {
                let nearest = (facades.iter())
                    .map(|facade| candidate.distance_from(facade.position))
                    .fold(f64::INFINITY, f64::min);
                let source = &attributes[candidate.attribute];
                let bound = candidate.bound_at_distance(nearest, source, &gain);
                (lden_energy(&bound), index)
            })
            .collect();
        every.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        every.truncate(FACADE_SOURCE_LIMIT);
        let ranked = facade_sources(&facades, &candidates, &attributes, &gain);
        assert_eq!(ranked, every, "{count} candidates");
    }
}
