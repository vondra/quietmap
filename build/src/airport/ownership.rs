//! Aeroway ownership from the mapped aerodrome covering the greatest length of a complete runway.

use super::lines::{Airport, distance_m};
use crate::aircraft::flat::interpolate;
use crate::dev4::{Dev4, Square, column, degrees_to_z30, text, z30_corner_degrees};
use crate::sources::cells::{PreparedRing, ring_cell};
use arrow_array::{BinaryArray, Float32Array, Int32Array, Int64Array, StringArray, UInt8Array};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const AERODROME: u8 = 5;
// Retain dev4's search radius when no identified footprint covers the geometry.
const AERODROME_MIN_RADIUS_M: f64 = 6_000.0;
const AERODROME_RADIUS_FACTOR: f64 = 1.5;

#[derive(Debug, Clone)]
pub(super) struct Aerodrome {
    pub osm_id: i64,
    pub(super) airport: Airport,
    pub(super) centre: [f64; 2],
    pub(super) area_m2: f64,
    pub(super) radius_m: f64,
    pub(super) footprint: Option<Arc<PreparedRing>>,
}

/// A repeated identifier cannot locate an official count when its mapped aerodromes' existing
/// search areas are disjoint. Keep observations, but do not send a supplement to a remote namesake.
pub(super) fn ambiguous_airport_keys(aerodromes: &[Aerodrome]) -> HashSet<String> {
    let mut by_key: HashMap<&str, Vec<&Aerodrome>> = HashMap::new();
    for aerodrome in aerodromes {
        by_key
            .entry(&aerodrome.airport.key)
            .or_default()
            .push(aerodrome);
    }
    by_key
        .into_iter()
        .filter_map(|(key, matches)| {
            matches
                .iter()
                .enumerate()
                .any(|(index, a)| {
                    matches[index + 1..]
                        .iter()
                        .any(|b| distance_m(a.centre, b.centre) > a.radius_m + b.radius_m)
                })
                .then(|| key.to_string())
        })
        .collect()
}

/// The named aerodromes of one square.
pub(super) fn aerodromes(dev4: &Dev4, square: Square) -> Result<Vec<Aerodrome>, String> {
    let Some(table) = dev4.table(square, "airport_areas.arrow")? else {
        return Ok(Vec::new());
    };
    let context = |error: String| format!("airport_areas.arrow of {square:?}: {error}");
    let mut found = Vec::new();
    for batch in &table.batches {
        let kind = column::<UInt8Array>(batch, "aeroway_type").map_err(context)?;
        let osm_id = column::<Int64Array>(batch, "osm_id").map_err(context)?;
        let (gx, gy) = (
            column::<Int32Array>(batch, "centroid_gx").map_err(context)?,
            column::<Int32Array>(batch, "centroid_gy").map_err(context)?,
        );
        let area = column::<Float32Array>(batch, "area_m2").map_err(context)?;
        let geometry = column::<BinaryArray>(batch, "geom").map_err(context)?;
        let labels = ["icao", "iata", "name"]
            .map(|name| column::<StringArray>(batch, name).map_err(context));
        let [icao, iata, name] = labels;
        let (icao, iata, name) = (icao?, iata?, name?);
        for row in 0..batch.num_rows() {
            if kind.value(row) != AERODROME {
                continue;
            }
            let label = |values: &StringArray| text(values, row).trim().to_string();
            let name = label(name);
            let Some(key) = [label(icao), label(iata), name.clone()]
                .into_iter()
                .find(|key| !key.is_empty())
            else {
                continue;
            };
            let (lat, lon) = z30_corner_degrees(gx.value(row), gy.value(row));
            let area_m2 = crate::dev4::positive(area, row).unwrap_or(0.0);
            found.push(Aerodrome {
                osm_id: osm_id.value(row),
                airport: Airport { key, name },
                centre: [lat, lon],
                area_m2,
                footprint: PreparedRing::new(&ring_cell(geometry, row)).map(Arc::new),
                radius_m: AERODROME_MIN_RADIUS_M
                    .max(AERODROME_RADIUS_FACTOR * (area_m2 / std::f64::consts::PI).sqrt()),
            });
        }
    }
    Ok(found)
}

/// Prefer the identified aerodrome footprint covering the greatest length of the mapped way. The input's at-most
/// 250 m pieces sample their midpoints. A point aerodrome cannot steal an enclosed runway;
/// disconnected pieces of the same way cannot acquire different airports. Equal coverage prefers
/// the larger mapped aerodrome, then the first sorted OSM id.
/// Only a way with no containing footprint falls back
/// to the aerodrome with the lowest length-weighted mean distance, then an unnamed geographic strip.
pub(super) fn airport_of(pieces: &[[[f64; 2]; 2]], aerodromes: &[Aerodrome]) -> Airport {
    let mut samples: Vec<_> = pieces
        .iter()
        .map(|&[a, b]| {
            let (lat, lon) = interpolate(a[0] as f32, a[1] as f32, b[0] as f32, b[1] as f32, 0.5);
            ([f64::from(lat), f64::from(lon)], distance_m(a, b))
        })
        .collect();
    // Summation order must not move an unnamed strip across a geographic-key boundary.
    samples.sort_by(|(a, x), (b, y)| {
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(x.total_cmp(y))
    });
    // Sub-metre prepared pieces can collapse to a point at the flight geometry's f32 precision.
    // A point still has an owner; use equal point weights when no piece retains any length.
    if samples.iter().all(|(_, length)| *length == 0.0) {
        for (_, length) in &mut samples {
            *length = 1.0;
        }
    }
    let mut covered = None;
    let mut nearest = None;
    let total: f64 = samples.iter().map(|(_, length)| length).sum();
    for aerodrome in aerodromes {
        let length: f64 = samples
            .iter()
            .filter(|(point, _)| {
                aerodrome
                    .footprint
                    .as_ref()
                    .is_some_and(|ring| ring.contains(degrees_to_z30(point[0], point[1])))
            })
            .map(|(_, length)| length)
            .sum();
        if length > 0.0
            && covered.is_none_or(|(owner, best): (&Aerodrome, f64)| {
                length > best || (length == best && aerodrome.area_m2 > owner.area_m2)
            })
        {
            covered = Some((aerodrome, length));
        }
        let distance = samples
            .iter()
            .map(|(point, length)| distance_m(*point, aerodrome.centre) * length)
            .sum::<f64>()
            / total;
        if distance <= aerodrome.radius_m && nearest.is_none_or(|(_, best)| distance < best) {
            nearest = Some((aerodrome, distance));
        }
    }
    covered.or(nearest).map_or_else(
        || {
            let latitude = samples
                .iter()
                .map(|(point, weight)| point[0] * weight)
                .sum::<f64>()
                / total;
            let longitude_sine = samples
                .iter()
                .map(|(point, weight)| point[1].to_radians().sin() * weight)
                .sum::<f64>();
            let longitude_cosine = samples
                .iter()
                .map(|(point, weight)| point[1].to_radians().cos() * weight)
                .sum::<f64>();
            let longitude = (longitude_sine.atan2(longitude_cosine).to_degrees() + 180.0)
                .rem_euclid(360.0)
                - 180.0;
            let middle = [latitude, longitude];
            Airport {
                key: format!(
                    "airstrip {:.2},{:.2}",
                    (middle[0] * 100.0).floor() / 100.0,
                    (middle[1] * 100.0).floor() / 100.0
                ),
                name: String::new(),
            }
        },
        |(aerodrome, _)| aerodrome.airport.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unnamed_geometry_keeps_its_key_when_reordered_or_reversed() {
        let pieces = [
            [[50.0, 179.990], [50.0, 179.995]],
            [[50.0, 179.995], [50.0, -179.999]],
        ];
        let mut reversed = pieces.map(|[a, b]| [b, a]);
        reversed.reverse();
        let original = airport_of(&pieces, &[]);
        assert_eq!(original, airport_of(&reversed, &[]));
        assert_eq!(original.key, "airstrip 50.00,179.99");
    }

    #[test]
    fn a_shared_identifier_cannot_allocate_counts_across_disjoint_aerodromes() {
        let first = Aerodrome {
            osm_id: 1,
            airport: Airport {
                key: "SHARED".into(),
                name: String::new(),
            },
            centre: [50.0, 14.0],
            area_m2: 0.0,
            radius_m: AERODROME_MIN_RADIUS_M,
            footprint: None,
        };
        let mut other = first.clone();
        other.osm_id = 2;
        other.centre = [50.01, 14.0];
        assert!(ambiguous_airport_keys(&[first.clone(), other.clone()]).is_empty());
        other.centre = [60.0, -150.0];
        assert_eq!(
            ambiguous_airport_keys(&[first, other]),
            HashSet::from(["SHARED".into()])
        );
    }

    #[test]
    fn a_whole_runway_belongs_to_its_footprint_despite_a_closer_neighbour() {
        let airport = |osm_id, key: &str, centre, bounds: Option<[[f64; 2]; 4]>| Aerodrome {
            osm_id,
            airport: Airport {
                key: key.into(),
                name: key.into(),
            },
            centre,
            area_m2: if bounds.is_some() { 1_000_000.0 } else { 0.0 },
            radius_m: AERODROME_MIN_RADIUS_M,
            footprint: bounds
                .and_then(|ring| PreparedRing::new(&ring.map(|p| degrees_to_z30(p[0], p[1]))))
                .map(Arc::new),
        };
        let aerodromes = [
            airport(1, "NEAR", [50.0, 14.0], None),
            airport(
                2,
                "OWNER",
                [50.01, 14.02],
                Some([
                    [49.999, 13.999],
                    [49.999, 14.03],
                    [50.002, 14.03],
                    [50.002, 13.999],
                ]),
            ),
        ];
        let pieces = [
            [[50.0, 14.0], [50.0, 14.003]],
            [[50.0, 14.003], [50.0, 14.006]],
        ];
        assert_eq!(airport_of(&pieces, &aerodromes).key, "OWNER");
        assert_eq!(airport_of(&pieces, &aerodromes[..1]).key, "NEAR");
        let mut reversed = pieces;
        reversed.reverse();
        assert_eq!(airport_of(&reversed, &aerodromes).key, "OWNER");
        let point = [[[50.0, 14.0]; 2]];
        assert_eq!(airport_of(&point, &aerodromes).key, "OWNER");
        assert_eq!(airport_of(&point, &aerodromes[..1]).key, "NEAR");

        let mut smaller = aerodromes[1].clone();
        smaller.osm_id = 0;
        smaller.airport.key = "INNER".into();
        smaller.area_m2 /= 2.0;
        smaller.footprint = PreparedRing::new(
            &[
                [49.999, 13.999],
                [49.999, 14.008],
                [50.001, 14.008],
                [50.001, 13.999],
            ]
            .map(|p| degrees_to_z30(p[0], p[1])),
        )
        .map(Arc::new);
        let nested = [smaller, aerodromes[1].clone()];
        assert_eq!(airport_of(&pieces, &nested).key, "OWNER");
    }
}
