//! A building crossing a square edge chooses its receiver independently of that edge.

use crate::exact::Point;
use crate::square::Square;
use popup::candidates::GROUND_REACH_M;
use popup::obstacles::{FacadeReceiver, Footprint};

pub(crate) fn choose(
    square: &Square,
    building: &Footprint,
    receivers: &[FacadeReceiver],
) -> Result<Point, String> {
    // The first canonical receiver gives both squares the same weather for the ranking.
    let (lat, lon) = square.frame.to_mercator(receivers[0].position).to_degrees();
    let candidates: Vec<_> = square
        .candidates
        .iter()
        .filter(|candidate| {
            receivers
                .iter()
                .any(|receiver| candidate.distance_from(receiver.position) <= GROUND_REACH_M)
        })
        .collect();
    let facade = popup::building::loudest_facade(
        building,
        receivers,
        &candidates,
        &square.attributes,
        &square.ground,
        &square.obstacles,
        square.weather.place(lat, lon),
    )?;
    Point::on_facade(square, facade.position, building.id)
}

/// Only background interpolation is clamped; the façade's rays and aircraft use its true point.
pub(crate) fn background_pixel(square: &Square, n: usize, position: [f64; 2]) -> usize {
    let pixel = [
        (0.5 + position[0] / square.frame.east_m_per_unit) * n as f64,
        (0.5 - position[1] / square.frame.north_m_per_unit) * n as f64,
    ]
    .map(|value| value.floor().clamp(0.0, (n - 1) as f64) as usize);
    pixel[1] * n + pixel[0]
}
