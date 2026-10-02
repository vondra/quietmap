//! The full physics of one source at the receiver: a line piece through the point-sum quadrature,
//! each node on its own ray, or a point on one ray; received A-weighted energy per period and band,
//! including the receiver reflection.

use crate::candidates::Candidate;
use crate::candidates::SourceAttribute;
use crate::obstacles::Scene;
use crate::scene::Ground;
use physics::bands::{BANDS, PERIODS, energy};
use physics::bound::POINT_DIVERGENCE_OFFSET_DB;
use physics::line::{LinePieceGeometry, LineQuadratureNode, SkylineArc, line_quadrature_nodes};
use physics::profile::Profile;
use physics::ray::{Crossing, RayEnds, RayScratch, RayTerms, Transfer, ray_terms, ray_transfer};
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

/// The terms of the ray from `point` of `source` to the receiver (traces and comparisons).
pub fn trace(
    receiver: &Receiver,
    point: [f64; 2],
    source: &SourceAttribute,
    scratch: &mut Scratch,
) -> Result<RayTerms, String> {
    let ends = ray_ends(receiver, source);
    receiver
        .ground
        .fill_profile(point, receiver.position, &mut scratch.profile)?;
    receiver
        .obstacles
        .crossings(point, receiver.position, &mut scratch.crossings)?;
    let p = std::array::from_fn(|period| {
        receiver
            .weather
            .favourable
            .at(period, receiver.azimuth(point))
    });
    Ok(ray_terms(
        &scratch.profile,
        &scratch.crossings,
        &ends,
        (p, &receiver.weather.alpha_db_per_km),
        &mut scratch.ray,
    ))
}

/// Received A-weighted energy per period and octave band.
pub type Bands = [[f64; BANDS]; PERIODS];

/// What one source delivers: per period and band, and per meteorological state (homogeneous,
/// favourable) and period summed over the bands, for the time the source is heard.
pub struct Received {
    pub bands: Bands,
    pub states: [[f64; PERIODS]; 2],
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

/// Adds `weight` times one ray's transfer of `emission` to `received`.
fn add_ray(
    received: &mut Received,
    emission: &[[f64; BANDS]; PERIODS],
    transfer: &Transfer,
    weight: f64,
) {
    for period in 0..PERIODS {
        for (band, power) in emission[period].iter().enumerate() {
            let power = weight * power;
            received.bands[period][band] += power * transfer.periods[period][band];
            for (state, total) in received.states.iter_mut().enumerate() {
                total[period] += power * transfer.states[state][band];
            }
        }
    }
}

/// One ray of a source at the receiver: the point it leaves from (click metres), the in-plane
/// angle it stands for on a line piece (0 for a point), its weight (the divergence, for a line the
/// angle times the line's, and the receiver reflection) and its transfer.
pub struct SourceRay {
    pub from_m: [f64; 2],
    pub angle_rad: f64,
    pub weight: f64,
    pub transfer: Transfer,
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
        let transfer = ray(receiver, a, true, &ends, scratch)?;
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
            transfer,
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
        let transfer = ray(receiver, point, node.obstacles_on_ray, &ends, scratch)?;
        visit(&SourceRay {
            from_m: point,
            angle_rad: node.weight_rad,
            weight: node.weight_rad * divergence * reflection,
            transfer,
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
        states: [[0.0; PERIODS]; 2],
    };
    source_rays(receiver, candidate, source, scratch, &mut |ray| {
        add_ray(&mut received, &source.energy, &ray.transfer, ray.weight)
    })?;
    Ok(received)
}

/// The transfer of one ray from `point` to the receiver.
fn ray(
    receiver: &Receiver,
    point: [f64; 2],
    obstacles_on_ray: bool,
    ends: &RayEnds,
    scratch: &mut Scratch,
) -> Result<Transfer, String> {
    receiver
        .ground
        .fill_profile(point, receiver.position, &mut scratch.profile)?;
    scratch.crossings.clear();
    if obstacles_on_ray {
        receiver
            .obstacles
            .crossings(point, receiver.position, &mut scratch.crossings)?;
    }
    let p = std::array::from_fn(|period| {
        receiver
            .weather
            .favourable
            .at(period, receiver.azimuth(point))
    });
    Ok(ray_transfer(
        &scratch.profile,
        &scratch.crossings,
        ends,
        (p, &receiver.weather.alpha_db_per_km),
        &mut scratch.ray,
    ))
}
