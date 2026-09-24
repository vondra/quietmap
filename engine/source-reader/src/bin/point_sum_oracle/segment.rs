//! One road or rail microsegment evaluated under every audited variant: today's line chain on
//! its characteristic-point ray, the line chain with #5/#28/#7/#27 switched one at a time, and
//! the CNOSSOS point sum with each node on its own ray.

use crate::ray::{ray_terms, validate_against_production, Bands, Method, RayTerms};
use noise_compute::constants::{ALPHA_ATM, M_PER_DEG_LAT};
use noise_compute::propagation::geo;
use noise_compute::propagation::arc_screening::{
    arc_screened_attenuation_with_ground, ArcBounds, ArcScreening, ArcScreeningScratch, ArcSkyline,
};
use noise_compute::propagation::iso9613::{ground_or_barrier_db, GroundPath};
use noise_compute::propagation::obstacle_index::{CrossingCandidate, ObstacleSet};
use noise_compute::propagation::path_effects::cnossos_ground_path_from_profile;
use noise_compute::propagation::point_sum::{line_nodes, point_source_divergence_db, NodeSpacing};
use noise_compute::propagation::PathProfile;
use noise_compute::types::{RasterSampler, NUM_BANDS};

/// The sampled world a ray crosses: rasters for the profile, vector obstacles for crossings.
pub trait World: Sync {
    fn rasters(&self) -> &dyn RasterSampler;
    fn crossings(&self, src_lat: f64, src_lon: f64, rcv_lat: f64, rcv_lon: f64, out: &mut Vec<CrossingCandidate>);
    /// The vector obstacle store, when the world has one: lets `today` also run through the
    /// production arc quadrature as a check of the closest-point replica.
    fn obstacle_set(&self) -> Option<&ObstacleSet> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReceiverPoint {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: f64,
    pub reflection_db: f64,
}

/// A line microsegment with the production loader's precomputed closest point.
#[derive(Debug, Clone)]
pub struct LineSegment {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub closest: (f64, f64),
    pub length_m: f64,
    pub dist_m: f64,
    pub source_height_m: f64,
    pub force_hard_ground: bool,
    /// `L_W′` per metre, dB per octave band, for day / evening / night.
    pub emission_db_per_m: [Bands; 3],
}

/// How a line source becomes received energy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Chain {
    /// Production: `−10·lg(2π d) + 10·lg(θ/π) + 10·lg(d_div/d_perp)` on the closest-point ray.
    Line { slant_angle: bool, normalization: bool },
    /// CNOSSOS §2.5.3: point nodes, `20·lg d + 11`, each node on its own ray.
    PointSum,
}

#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub name: &'static str,
    pub chain: Chain,
    pub method: Method,
}

const FAVOURABLE_FLOOR: Method = Method {
    favourable_ground_floor: true,
    diffraction_positive_slope: 20.0,
};
const CNOSSOS_SLOPE: Method = Method {
    favourable_ground_floor: false,
    diffraction_positive_slope: 40.0,
};
const FLOOR_AND_SLOPE: Method = Method {
    favourable_ground_floor: true,
    diffraction_positive_slope: 40.0,
};
const LINE_METHODS: [Method; 4] = [Method::TODAY, FAVOURABLE_FLOOR, CNOSSOS_SLOPE, FLOOR_AND_SLOPE];
const POINT_SUM_METHODS: [Method; 2] = [Method::TODAY, FLOOR_AND_SLOPE];
const TODAY_LINE: Chain = Chain::Line {
    slant_angle: false,
    normalization: false,
};

pub const VARIANTS: [Variant; 9] = [
    Variant { name: "today", chain: TODAY_LINE, method: Method::TODAY },
    Variant { name: "n5_normalization", chain: Chain::Line { slant_angle: false, normalization: true }, method: Method::TODAY },
    Variant { name: "n28_slant_angle", chain: Chain::Line { slant_angle: true, normalization: false }, method: Method::TODAY },
    Variant { name: "n7_favourable_floor", chain: TODAY_LINE, method: FAVOURABLE_FLOOR },
    Variant { name: "n27_slope_40", chain: TODAY_LINE, method: CNOSSOS_SLOPE },
    Variant { name: "n5_n28_line", chain: Chain::Line { slant_angle: true, normalization: true }, method: Method::TODAY },
    Variant { name: "n5_n28_n7_n27_line", chain: Chain::Line { slant_angle: true, normalization: true }, method: FLOOR_AND_SLOPE },
    Variant { name: "point_sum", chain: Chain::PointSum, method: Method::TODAY },
    Variant { name: "point_sum_n7_n27", chain: Chain::PointSum, method: FLOOR_AND_SLOPE },
];

/// Accumulate one ray's or segment's period band energies.
pub fn add_energy(total: &mut [Bands; 3], energy: &[Bands; 3]) {
    for (total_period, period) in total.iter_mut().zip(energy) {
        for (t, e) in total_period.iter_mut().zip(period) {
            *t += e;
        }
    }
}

/// `10·lg(2π²/10^1.1)`: the point sum over the line chain for a receiver at source height.
pub fn normalization_gap_db() -> f64 {
    10.0 * (2.0 * std::f64::consts::PI.powi(2) / 10f64.powf(1.1)).log10()
}

/// Received band energies (linear, unweighted) per period for every variant, plus the largest
/// |replica − production| of the mixed ray terms on the closest-point ray.
pub struct SegmentResult {
    pub energies: Vec<[Bands; 3]>,
    pub validation_db: f64,
    /// `today` with the production arc quadrature in place of the closest-point screening.
    pub today_with_production_arc: Option<[Bands; 3]>,
    /// Point nodes (own rays) the point sum traced.
    pub point_sum_nodes: usize,
}

pub fn evaluate_segment(
    world: &dyn World,
    segment: &LineSegment,
    receiver: &ReceiverPoint,
    spacing: NodeSpacing,
) -> SegmentResult {
    let mut scratch = RayScratch::default();
    let (cp_lat, cp_lon) = segment.closest;
    let src_alt = world.rasters().elevation(cp_lat, cp_lon) + segment.source_height_m;
    let cp_ray = scratch.trace(world, (cp_lat, cp_lon), segment, receiver, src_alt, &LINE_METHODS, true);
    let d_slant = geo::slant_dist(segment.dist_m, src_alt, receiver.altitude_m);
    let foot = geo::point_to_segment_full(
        receiver.lat,
        receiver.lon,
        segment.start.0,
        segment.start.1,
        segment.end.0,
        segment.end.1,
    );
    let (point_sums, point_sum_nodes) = scratch.point_sums(world, segment, receiver, spacing, &POINT_SUM_METHODS);
    let divergence = 10.0 * (2.0 * std::f64::consts::PI * d_slant.max(1.0)).log10();
    let today_flc =
        geo::finite_line_correction_for_divergence(segment.length_m, foot.d_perp_m, foot.fraction, segment.dist_m);
    let energies = VARIANTS
        .iter()
        .map(|variant| match variant.chain {
            Chain::Line { slant_angle, normalization } => {
                let flc = if slant_angle {
                    let lever = foot.d_perp_m.hypot(receiver.altitude_m - src_alt);
                    geo::finite_line_correction_for_divergence(segment.length_m, lever, foot.fraction, d_slant)
                } else {
                    today_flc
                };
                let gain = flc + if normalization { normalization_gap_db() } else { 0.0 };
                let terms = cp_ray.terms(variant.method);
                let composite = terms.barrier_or_ground_mixed_first();
                received(segment, &composite, &terms.vegetation, divergence, d_slant, receiver.reflection_db + gain)
            }
            Chain::PointSum => {
                let index = POINT_SUM_METHODS.iter().position(|m| *m == variant.method).expect("point-sum method");
                point_sums[index]
            }
        })
        .collect();
    let today_with_production_arc = world.obstacle_set().map(|set| {
        let cp = cp_ray.terms(Method::TODAY);
        let screening = production_arc_screening(world, set, segment, receiver, &cp_ray, src_alt);
        let (ground, terrain) = (cp.ground.mixed(), cp.terrain.mixed());
        let composite: Bands = std::array::from_fn(|b| ground_or_barrier_db(ground[b], terrain[b], screening[b]));
        received(segment, &composite, &cp.vegetation, divergence, d_slant, receiver.reflection_db + today_flc)
    });
    SegmentResult {
        energies,
        validation_db: cp_ray.validation_db,
        today_with_production_arc,
        point_sum_nodes,
    }
}

/// The production arc quadrature (`arc_screened_attenuation_with_ground`) for this segment,
/// fed with the replica's closest-point terms, which equal production's (validated per ray).
fn production_arc_screening(
    world: &dyn World,
    set: &ObstacleSet,
    segment: &LineSegment,
    receiver: &ReceiverPoint,
    cp_ray: &TracedRay,
    src_alt: f64,
) -> Bands {
    let cp = cp_ray.terms(Method::TODAY);
    let (ground, terrain, cp_screening) = (cp.ground.mixed(), cp.terrain.mixed(), cp.screening_mixed());
    let query = ArcScreening {
        receiver_lat: receiver.lat,
        receiver_lon: receiver.lon,
        receiver_alt_m: receiver.altitude_m,
        start_lat: segment.start.0,
        start_lon: segment.start.1,
        end_lat: segment.end.0,
        end_lon: segment.end.1,
        source_height_m: segment.source_height_m,
        cp_lat: segment.closest.0,
        cp_lon: segment.closest.1,
        src_alt_m: src_alt,
        cp_screening: &cp_screening,
        cp_terrain: &terrain,
        ground_g: cp_ray.ground_path.ground_path_g,
        obstacles: set,
        length_m: segment.length_m,
        dist_m: segment.dist_m,
        exclusion_radius_m: 0.0,
        bounds: ArcBounds::shipped(),
    };
    arc_screened_attenuation_with_ground(
        &query,
        world.rasters(),
        &mut ArcSkyline::default(),
        &ground,
        &mut ArcScreeningScratch::new(),
    )
}

/// Band energies (linear, unweighted) per period of one ray: `L_W′ + gain − A_div − A_atm −
/// max(A_ground, A_bar) − A_foliage`, the gain carrying FLC, normalization, node length and the
/// receiver reflection bonus.
fn received(segment: &LineSegment, composite: &Bands, vegetation: &Bands, divergence_db: f64, slant_m: f64, gain_db: f64) -> [Bands; 3] {
    std::array::from_fn(|period| {
        std::array::from_fn(|band| {
            let level = segment.emission_db_per_m[period][band] + gain_db - divergence_db
                - ALPHA_ATM[band] * slant_m / 1000.0
                - composite[band]
                - vegetation[band];
            10f64.powf(level / 10.0)
        })
    })
}

/// One ray's terms for each requested method, sharing one profile and crossing list.
struct TracedRay {
    by_method: Vec<(Method, RayTerms)>,
    ground_path: GroundPath,
    validation_db: f64,
}

impl TracedRay {
    fn terms(&self, method: Method) -> &RayTerms {
        &self.by_method.iter().find(|(m, _)| *m == method).expect("every variant method is traced").1
    }
}

#[derive(Default)]
struct RayScratch {
    profile: PathProfile,
    candidates: Vec<CrossingCandidate>,
}

impl RayScratch {
    #[allow(clippy::too_many_arguments)]
    fn trace(
        &mut self,
        world: &dyn World,
        source: (f64, f64),
        segment: &LineSegment,
        receiver: &ReceiverPoint,
        src_alt: f64,
        methods: &[Method],
        closest_point_ray: bool,
    ) -> TracedRay {
        // The closest-point ray keeps the loader's distance, exactly as production does.
        let dist = if closest_point_ray {
            segment.dist_m
        } else {
            geo::flat_dist(source.0, source.1, receiver.lat, receiver.lon)
        };
        world.rasters().build_path_profile(source.0, source.1, receiver.lat, receiver.lon, dist, &mut self.profile);
        world.crossings(source.0, source.1, receiver.lat, receiver.lon, &mut self.candidates);
        let ground_path =
            cnossos_ground_path_from_profile(&mut self.profile, src_alt, receiver.altitude_m, segment.force_hard_ground);
        let by_method: Vec<(Method, RayTerms)> = methods
            .iter()
            .map(|&m| (m, ray_terms(&self.profile, &self.candidates, src_alt, receiver.altitude_m, ground_path, m)))
            .collect();
        let validation_db = if closest_point_ray {
            validate_against_production(
                &mut self.profile,
                &self.candidates,
                src_alt,
                receiver.altitude_m,
                ground_path,
                &by_method[0].1,
            )
        } else {
            0.0
        };
        TracedRay {
            by_method,
            ground_path,
            validation_db,
        }
    }

    /// Energy of the point sum for each method, and the node count; every node traces its own
    /// ray once.
    fn point_sums(
        &mut self,
        world: &dyn World,
        segment: &LineSegment,
        receiver: &ReceiverPoint,
        spacing: NodeSpacing,
        methods: &[Method],
    ) -> (Vec<[Bands; 3]>, usize) {
        let metres_per_lon = geo::m_per_deg_lon(receiver.lat.to_radians());
        let local = |(lat, lon): (f64, f64), height: f64| -> [f64; 3] {
            [
                geo::wrapped_longitude_delta(receiver.lon, lon) * metres_per_lon,
                (lat - receiver.lat) * M_PER_DEG_LAT,
                world.rasters().elevation(lat, lon) + height,
            ]
        };
        let start = local(segment.start, segment.source_height_m);
        let end = local(segment.end, segment.source_height_m);
        let receiver_m = [0.0, 0.0, receiver.altitude_m];
        let mut totals = vec![[[0.0; NUM_BANDS]; 3]; methods.len()];
        let nodes = line_nodes(start, end, receiver_m, spacing);
        for node in &nodes {
            let lat = receiver.lat + node.position_m[1] / M_PER_DEG_LAT;
            let lon = geo::normalize_longitude(receiver.lon + node.position_m[0] / metres_per_lon);
            let src_alt = world.rasters().elevation(lat, lon) + segment.source_height_m;
            let ray = self.trace(world, (lat, lon), segment, receiver, src_alt, methods, false);
            let divergence = point_source_divergence_db(node.slant_distance_m);
            for (total, &method) in totals.iter_mut().zip(methods) {
                let terms = ray.terms(method);
                let energy = received(
                    segment,
                    &terms.barrier_or_ground_mixed_first(),
                    &terms.vegetation,
                    divergence,
                    node.slant_distance_m,
                    receiver.reflection_db + 10.0 * node.length_m.log10(),
                );
                add_energy(total, &energy);
            }
        }
        (totals, nodes.len())
    }
}
