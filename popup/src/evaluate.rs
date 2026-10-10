//! The full physics of one source at the receiver: a line piece through the point-sum quadrature,
//! each node on its own ray, or a point on one ray; received A-weighted energy per period and band,
//! including the receiver reflection, and how it got there (its path account).

use crate::candidates::Candidate;
use crate::candidates::SourceAttribute;
use crate::obstacles::Scene;
use crate::scene::Ground;
use physics::bands::{BANDS, PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy};
use physics::bound::POINT_DIVERGENCE_OFFSET_DB;
use physics::line::{LinePieceGeometry, LineQuadratureNode, SkylineArc, line_quadrature_nodes};
use physics::profile::Profile;
use physics::ray::{Crossing, RayEnds, RayScratch, RayTerms, ray_terms};
use physics::weather::PlaceWeather;

/// What every source of one click shares.
pub struct Receiver<'s, 'a> {
    pub ground: &'s Ground<'a>,
    pub obstacles: &'s Scene<'a>,
    /// Where the receiver stands (click metres): the click, or a building's façade receiver.
    pub position: [f64; 2],
    pub altitude_m: f64,
    pub weather: PlaceWeather,
    pub reflection_db: f64,
    /// The building the receiver stands in (a painted point inside a footprint): its walls do not
    /// screen it. 0 for none: a click is answered outside every building.
    pub own_footprint: u64,
}

/// Per-thread buffers.
#[derive(Default)]
pub struct Scratch {
    nodes: Vec<LineQuadratureNode>,
    profile: Profile,
    crossings: Vec<Crossing>,
    ray: RayScratch,
}

impl Scratch {
    /// The ground under the last ray [`trace`] followed.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
}

impl Receiver<'_, '_> {
    /// Direction of travel from `point` to the receiver, radians anticlockwise from east.
    fn azimuth(&self, point: [f64; 2]) -> f64 {
        (self.position[1] - point[1]).atan2(self.position[0] - point[0])
    }
}

/// The ray ends of a source at this receiver.
fn ray_ends(receiver: &Receiver, source: &SourceAttribute) -> RayEnds {
    RayEnds {
        source_height_m: source.height_m,
        receiver_altitude_m: receiver.altitude_m,
        source_ground_factor: (source.ground_percent != tiles::sources::GROUND_FROM_TERRAIN)
            .then(|| f64::from(source.ground_percent) / 100.0),
        platform_half_width_m: source.platform_half_width_m,
        own_footprint: source.footprint_id,
    }
}

/// The terms of the ray from `point` of `source` to the receiver, every obstacle on it (traces and
/// comparisons).
pub fn trace(
    receiver: &Receiver,
    point: [f64; 2],
    source: &SourceAttribute,
    scratch: &mut Scratch,
) -> Result<RayTerms, String> {
    ray(receiver, point, true, &ray_ends(receiver, source), scratch)
}

/// Received A-weighted energy per period and octave band.
pub type Bands = [[f64; BANDS]; PERIODS];

/// What one source delivers: per period and band, and how its sound got there; and by what its
/// rays bend over in calm air ([`physics::ray::Edge`]: nothing, a building or wall, terrain) their
/// Lden-weighted energy after the air, after the screening and after the boundary, calm.
pub struct Received {
    pub bands: Bands,
    pub path: Path,
    pub edges: [[f64; 3]; 3],
}

/// How a source's sound reaches the receiver, its rays' A-weighted energy per period summed after
/// each term (the façades' reflection in all): over distance alone, with the air's absorption, with
/// the screening of each meteorological state (homogeneous, favourable), and with its ground too:
/// the boundary, whose states the weather mixes into what is received. Each term's dB is the ratio
/// of two sums, so the terms add up from the free field to the level.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Path {
    pub free: [f64; PERIODS],
    pub air: [f64; PERIODS],
    pub screened: [[f64; PERIODS]; 2],
    pub boundary: [[f64; PERIODS]; 2],
}

impl Path {
    /// The path `weight` times, added (a sampled piece stands for its share of the estimate).
    pub fn add(&mut self, other: &Path, weight: f64) {
        for period in 0..PERIODS {
            self.free[period] += weight * other.free[period];
            self.air[period] += weight * other.air[period];
            for state in 0..2 {
                self.screened[state][period] += weight * other.screened[state][period];
                self.boundary[state][period] += weight * other.boundary[state][period];
            }
        }
    }
}

/// The per-period sums of band energies.
pub fn period_sums(bands: &Bands) -> [f64; PERIODS] {
    bands.map(|period| period.iter().sum())
}

/// Received A-weighted energy per period of one candidate with its attribute.
pub fn received_energy(
    receiver: &Receiver,
    candidate: &Candidate,
    source: &SourceAttribute,
    scratch: &mut Scratch,
) -> Result<[f64; PERIODS], String> {
    received_bands(receiver, candidate, source, scratch)
        .map(|received| period_sums(&received.bands))
}

/// Adds `weight` times what one ray with its `terms` delivers of `emission` to `received`.
fn add_ray(
    received: &mut Received,
    emission: &[[f64; BANDS]; PERIODS],
    terms: &RayTerms,
    weight: f64,
) {
    let air = terms.air_db.map(|db| energy(-db));
    let screened = [0, 1].map(|state| {
        std::array::from_fn::<f64, BANDS, _>(|band| {
            air[band] * energy(-terms.boundaries[state].without_ground_db[band])
        })
    });
    let transfer = &terms.transfer;
    let path = &mut received.path;
    let edge = &mut received.edges[terms.calm_edge as usize];
    for (period, powers) in emission.iter().enumerate() {
        let lden = PERIOD_HOURS[period] / 24.0 * energy(PERIOD_PENALTY_DB[period]);
        for (band, power) in powers.iter().enumerate() {
            let power = weight * power;
            edge[0] += lden * power * air[band];
            edge[1] += lden * power * screened[0][band];
            edge[2] += lden * power * transfer.states[0][band];
            received.bands[period][band] += power * transfer.periods[period][band];
            path.free[period] += power;
            path.air[period] += power * air[band];
            for (state, (screened, boundary)) in screened.iter().zip(&transfer.states).enumerate() {
                path.screened[state][period] += power * screened[band];
                path.boundary[state][period] += power * boundary[band];
            }
        }
    }
}

/// One ray of a source at the receiver: the point it leaves from (click metres), the in-plane
/// angle it stands for on a line piece (0 for a point), its weight (the divergence, for a line the
/// angle times the line's, and the receiver reflection), its terms, and the ground and the walls it
/// was computed over (none where the skyline shows no wall can reach its line of sight).
pub struct SourceRay<'s> {
    pub from_m: [f64; 2],
    pub angle_rad: f64,
    pub weight: f64,
    pub terms: RayTerms,
    pub profile: &'s Profile,
    pub crossings: &'s [Crossing],
}

/// Every ray of one candidate at the receiver: a point's one, a line piece's quadrature nodes.
pub fn source_rays(
    receiver: &Receiver,
    candidate: &Candidate,
    source: &SourceAttribute,
    scratch: &mut Scratch,
    visit: &mut dyn FnMut(&SourceRay),
) -> Result<(), String> {
    let reflection = energy(receiver.reflection_db);
    let ends = ray_ends(receiver, source);
    let [a, b] = candidate.ends_m;
    if !candidate.line {
        let terms = ray(receiver, a, true, &ends, scratch)?;
        let distance = candidate
            .distance_m
            .max(source.exclusion_radius_m)
            .hypot(receiver.altitude_m - (candidate.ground_m[0] + source.height_m))
            .max(1.0);
        let divergence = 1.0 / (distance * distance * energy(POINT_DIVERGENCE_OFFSET_DB));
        visit(&SourceRay {
            from_m: a,
            angle_rad: 0.0,
            weight: divergence * reflection,
            terms,
            profile: &scratch.profile,
            crossings: &scratch.crossings,
        });
        return Ok(());
    }
    let altitude = |end: usize| candidate.ground_m[end] + source.height_m - receiver.altitude_m;
    let [x, y] = receiver.position;
    let Some(geometry) = LinePieceGeometry::new(
        [a[0] - x, a[1] - y, altitude(0)],
        [b[0] - x, b[1] - y, altitude(1)],
    ) else {
        return Ok(());
    };
    let obstacles = receiver.obstacles;
    let mut skyline = |lo: f64, hi: f64, radius: f64, visit: &mut dyn FnMut(SkylineArc)| {
        obstacles.skyline_arcs(
            receiver.position,
            lo,
            hi,
            radius,
            source.height_m.max(0.0),
            &mut |arc| {
                visit(SkylineArc {
                    lo_rad: arc.lo_rad,
                    hi_rad: arc.hi_rad,
                    nearest_m: arc.nearest_m,
                })
            },
        );
    };
    let mut nodes = std::mem::take(&mut scratch.nodes);
    line_quadrature_nodes(&geometry, &mut skyline, &mut nodes);
    let divergence = geometry.divergence_factor();
    for node in &nodes {
        let fraction = node.along_m / geometry.length_m();
        let point = [
            a[0] + fraction * (b[0] - a[0]),
            a[1] + fraction * (b[1] - a[1]),
        ];
        let terms = ray(receiver, point, node.obstacles_on_ray, &ends, scratch)?;
        visit(&SourceRay {
            from_m: point,
            angle_rad: node.weight_rad,
            weight: node.weight_rad * divergence * reflection,
            terms,
            profile: &scratch.profile,
            crossings: &scratch.crossings,
        });
    }
    scratch.nodes = nodes;
    Ok(())
}

/// Received A-weighted energy per period and band of one candidate with its attribute.
pub fn received_bands(
    receiver: &Receiver,
    candidate: &Candidate,
    source: &SourceAttribute,
    scratch: &mut Scratch,
) -> Result<Received, String> {
    let mut received = Received {
        bands: [[0.0; BANDS]; PERIODS],
        path: Path::default(),
        edges: [[0.0; 3]; 3],
    };
    source_rays(receiver, candidate, source, scratch, &mut |ray| {
        add_ray(&mut received, &source.energy, &ray.terms, ray.weight)
    })?;
    Ok(received)
}

/// Drops the walls of the building the receiver stands in.
fn without_own_walls(receiver: &Receiver, crossings: &mut Vec<Crossing>) {
    if receiver.own_footprint != 0 {
        crossings.retain(|crossing| crossing.footprint_id != receiver.own_footprint);
    }
}

/// The terms of one ray from `point` to the receiver.
fn ray(
    receiver: &Receiver,
    point: [f64; 2],
    obstacles_on_ray: bool,
    ends: &RayEnds,
    scratch: &mut Scratch,
) -> Result<RayTerms, String> {
    receiver
        .ground
        .fill_profile(point, receiver.position, &mut scratch.profile)?;
    scratch.crossings.clear();
    if obstacles_on_ray {
        receiver
            .obstacles
            .crossings(point, receiver.position, &mut scratch.crossings)?;
        without_own_walls(receiver, &mut scratch.crossings);
    }
    let p = std::array::from_fn(|period| {
        receiver
            .weather
            .favourable
            .at(period, receiver.azimuth(point))
    });
    Ok(ray_terms(
        &scratch.profile,
        &scratch.crossings,
        ends,
        (p, &receiver.weather.alpha_db_per_km),
        &mut scratch.ray,
    ))
}
