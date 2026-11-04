//! Match measured points and line support with explicit distance and travel-direction semantics.

use super::{input::Road, observation::Observation, observation_target::DistanceRule};

pub fn longitude_delta(value: f64) -> f64 {
    if value > 180.0 {
        value - 360.0
    } else if value < -180.0 {
        value + 360.0
    } else {
        value
    }
}

pub(crate) fn flat_distance(point: [f64; 2], other: [f64; 2]) -> f64 {
    let cosine = ((point[1] + other[1]) / 2.0 * std::f64::consts::PI / 180.0).cos();
    (longitude_delta(other[0] - point[0]) * 111_320.0 * cosine)
        .hypot((other[1] - point[1]) * 110_540.0)
}

pub(crate) fn great_circle(point: [f64; 2], other: [f64; 2]) -> f64 {
    let radians = |v: f64| v * std::f64::consts::PI / 180.0;
    let dlat = radians(other[1] - point[1]);
    let dlon = radians(other[0] - point[0]);
    let a = (dlat / 2.0).sin().powi(2)
        + radians(point[1]).cos() * radians(other[1]).cos() * (dlon / 2.0).sin().powi(2);
    // IUGG mean Earth radius and arithmetic of the frozen point-counter matcher.
    2.0 * 6_371_008.8 * a.sqrt().min(1.0).asin()
}

pub(crate) fn segment_distance(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> f64 {
    // Preserve the original line matcher's metric approximation for exact source parity.
    let cosine = (point[1] * std::f64::consts::PI / 180.0).cos();
    let px = longitude_delta(point[0] - start[0]) * 111_320.0 * cosine;
    let py = (point[1] - start[1]) * 110_540.0;
    let ex = longitude_delta(end[0] - start[0]) * 111_320.0 * cosine;
    let ey = (end[1] - start[1]) * 110_540.0;
    let length = ex * ex + ey * ey;
    if length < 1e-6 {
        return flat_distance(point, start);
    }
    let fraction = ((px * ex + py * ey) / length).clamp(0.0, 1.0);
    // A shared survey endpoint must have the same distance from either incident line.
    if fraction == 1.0 {
        return (longitude_delta(point[0] - end[0]) * 111_320.0 * cosine)
            .hypot((point[1] - end[1]) * 110_540.0);
    }
    (px - fraction * ex).hypot(py - fraction * ey)
}

pub fn road_points(road: &Road) -> [[f64; 2]; 3] {
    // Keep the producer's operation order: rounding at a shared survey endpoint can break a tie.
    let degrees = |(x, y): (f64, f64)| {
        let radius = crate::osm::geometry::WEB_MERCATOR_RADIUS_M;
        [
            (x / radius) * 180.0 / std::f64::consts::PI,
            (2.0 * (y / radius).exp().atan() - std::f64::consts::PI / 2.0) * 180.0
                / std::f64::consts::PI,
        ]
    };
    let [start_lon, start_lat] = degrees(road.start);
    let [end_lon, end_lat] = degrees(road.end);
    let normalized = |value: f64| ((value + 180.0) % 360.0 + 360.0) % 360.0 - 180.0;
    let middle = normalized(start_lon + normalized(end_lon - start_lon) / 2.0);
    let unwrap = |value: f64| {
        if value - middle > 180.0 {
            value - 360.0
        } else if value - middle < -180.0 {
            value + 360.0
        } else {
            value
        }
    };
    [
        [unwrap(start_lon), start_lat],
        [middle, (start_lat + end_lat) / 2.0],
        [unwrap(end_lon), end_lat],
    ]
}

fn follows(
    row: &Observation,
    road: &Road,
    mapped: &[[f64; 2]; 3],
    start: [f64; 2],
    end: [f64; 2],
) -> bool {
    if row.direction == 0 && row.target.minimum_alignment_cosine == 0.0 {
        return true;
    }
    let scale = 111_320.0 * (mapped[1][1] * std::f64::consts::PI / 180.0).cos();
    let x = longitude_delta(mapped[2][0] - mapped[0][0]) * scale;
    let y = (mapped[2][1] - mapped[0][1]) * 110_540.0;
    let ex = longitude_delta(end[0] - start[0]) * scale;
    let ey = (end[1] - start[1]) * 110_540.0;
    let lengths = x.hypot(y) * ex.hypot(ey);
    if lengths == 0.0 {
        return row.direction == 0;
    }
    let cosine = (x * ex + y * ey) / lengths;
    if row.direction == 0 {
        return cosine.abs() >= row.target.minimum_alignment_cosine;
    }
    let sign = if (road.direction == 2) != (row.direction == 2) {
        -1.0
    } else {
        1.0
    };
    sign * cosine >= row.target.minimum_alignment_cosine.max(f64::MIN_POSITIVE)
}

pub fn distance(row: &Observation, road: &Road, points: &[[f64; 2]; 3]) -> f64 {
    distance_with_heading(row, road, points, true)
}

fn distance_with_heading(
    row: &Observation,
    road: &Road,
    points: &[[f64; 2]; 3],
    respect_heading: bool,
) -> f64 {
    if respect_heading && row.direction != 0 && road.direction == 0 {
        return f64::INFINITY;
    }
    if row.geometry.is_empty() {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    if row.target.distance_rule == DistanceRule::NearestPiece {
        let mut nearest_follows = false;
        for part in &row.geometry {
            if part.len() == 1 {
                let distance = flat_distance(points[1], part[0]);
                if distance < best {
                    best = distance;
                    nearest_follows =
                        !respect_heading || follows(row, road, points, part[0], part[0]);
                }
            }
            for pair in part.windows(2) {
                let distance = segment_distance(points[1], pair[0], pair[1]);
                if distance < best {
                    best = distance;
                    nearest_follows =
                        !respect_heading || follows(row, road, points, pair[0], pair[1]);
                } else if distance == best
                    && (!respect_heading || follows(row, road, points, pair[0], pair[1]))
                {
                    nearest_follows = true;
                }
            }
        }
        return if nearest_follows { best } else { f64::INFINITY };
    }
    for part in &row.geometry {
        if part.len() == 1 {
            let distance = match row.target.distance_rule {
                DistanceRule::PointToPiece => segment_distance(part[0], points[0], points[2]),
                DistanceRule::GreatCircle => great_circle(points[1], part[0]),
                _ => flat_distance(points[1], part[0]),
            };
            best = best.min(distance);
            continue;
        }
        let at = |point| {
            part.windows(2)
                .filter(|pair| !respect_heading || follows(row, road, points, pair[0], pair[1]))
                .map(|pair| segment_distance(point, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min)
        };
        let distance = if row.target.distance_rule == DistanceRule::WholePiece {
            points.iter().map(|p| at(*p)).fold(0.0, f64::max)
        } else {
            at(points[1])
        };
        best = best.min(distance);
    }
    best
}

pub fn distance_without_heading(row: &Observation, road: &Road, points: &[[f64; 2]; 3]) -> f64 {
    distance_with_heading(row, road, points, false)
}

/// End-to-end support breaks a midpoint tie without choosing a count by serialization order.
pub(super) fn endpoint_support(row: &Observation, points: &[[f64; 2]; 3]) -> f64 {
    [points[0], points[2]]
        .into_iter()
        .map(|point| {
            row.geometry
                .iter()
                .flat_map(|part| part.windows(2))
                .map(|pair| segment_distance(point, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min)
        })
        .sum()
}
