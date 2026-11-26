//! A building crossing a square edge chooses its receiver independently of that edge.

use crate::exact::Point;
use crate::square::Square;
use popup::candidates::GROUND_REACH_M;
use popup::obstacles::{FacadeReceiver, Footprint};
use std::sync::Arc;

/// Distance from the receiver set's centre within which every receiver is guaranteed within
/// [`GROUND_REACH_M`]. Stays signed (negative when `radius > GROUND_REACH_M`) so a midpoint source
/// farther than [`GROUND_REACH_M`] from every receiver is never falsely admitted.
fn inner_reach_m(radius: f64) -> f64 {
    GROUND_REACH_M - radius - 1e-9
}

pub(crate) fn choose(
    square: &Square,
    building: &Footprint,
    receivers: &[FacadeReceiver],
    excluded_sources: Option<Arc<[u32]>>,
) -> Result<Point, String> {
    // The first canonical receiver gives both squares the same weather for the ranking.
    let (lat, lon) = square.frame.to_mercator(receivers[0].position).to_degrees();
    let excluded = excluded_sources.as_deref();
    let (mut low, mut high) = (receivers[0].position, receivers[0].position);
    for receiver in &receivers[1..] {
        low[0] = low[0].min(receiver.position[0]);
        low[1] = low[1].min(receiver.position[1]);
        high[0] = high[0].max(receiver.position[0]);
        high[1] = high[1].max(receiver.position[1]);
    }
    let centre = [0.5 * (low[0] + high[0]), 0.5 * (low[1] + high[1])];
    let radius = receivers
        .iter()
        .map(|r| (r.position[0] - centre[0]).hypot(r.position[1] - centre[1]))
        .fold(0.0f64, f64::max);
    let inner_reach = inner_reach_m(radius);
    let outer_reach = GROUND_REACH_M + radius + 1e-9;
    let mut near = Vec::new();
    square.index.within(low, high, GROUND_REACH_M, &mut near);
    near.sort_unstable();
    let candidates: Vec<_> = near
        .into_iter()
        .filter(|index| excluded.is_none_or(|list| list.binary_search(index).is_err()))
        .map(|index| &square.candidates[index as usize])
        .filter(|candidate| {
            let d = candidate.distance_from(centre);
            d <= inner_reach
                || (d <= outer_reach
                    && receivers.iter().any(|receiver| {
                        candidate.distance_from(receiver.position) <= GROUND_REACH_M
                    }))
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
    Point::on_facade(square, facade.position, building.id, excluded_sources)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_receiver_span_keeps_negative_inner_reach() {
        // Two exposed ends 30 km apart (radius 15 km > 12 km reach): a source at their midpoint
        // (distance 0) is 15 km from every receiver and must not pass `d <= inner_reach`.
        let inner = inner_reach_m(15_000.0);
        assert!(inner < 0.0, "expected negative inner reach, got {inner}");
        assert!(0.0 > inner);
        assert!(inner_reach_m(100.0) > 11_899.0);
    }
}
