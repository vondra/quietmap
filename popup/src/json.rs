//! One streamed update as a line of JSON: totals per layer and period, the loudest contributor
//! groups with their display records, and what the click read so far.

use crate::update::Update;
use physics::bands::{PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy};
use serde_json::{Map, Value, json};
use tiles::sources::Layer;

/// A level rounded to 0.1 dB, `null` for silence.
fn level(energy_value: f64) -> Value {
    if energy_value > 0.0 {
        json!((100.0 * energy_value.log10()).round() / 10.0)
    } else {
        Value::Null
    }
}

fn lden(periods: &[f64; PERIODS]) -> Value {
    level(
        (0..PERIODS)
            .map(|p| PERIOD_HOURS[p] * periods[p] * energy(PERIOD_PENALTY_DB[p]))
            .sum::<f64>()
            / 24.0,
    )
}

fn periods(object: &mut Map<String, Value>, energies: &[f64; PERIODS]) {
    for (name, value) in ["ld", "le", "ln"].iter().zip(energies) {
        object.insert((*name).into(), level(*value));
    }
    object.insert("lden".into(), lden(energies));
}

/// The contributor's label: its name, else its reference or address, else its class or type.
fn label(display: &Value) -> String {
    [
        "name",
        "ref",
        "address",
        "road_class",
        "rail_type",
        "building_type",
        "source_type",
    ]
    .iter()
    .filter_map(|key| display.get(*key).and_then(Value::as_str))
    .find(|text| !text.is_empty())
    .unwrap_or("")
    .to_string()
}

pub fn update_line(update: &Update, sequence: usize) -> Result<String, String> {
    let mut total = [0.0; PERIODS];
    let mut layers = Vec::new();
    for layer in &update.layers {
        for (sum, value) in total.iter_mut().zip(layer.energy) {
            *sum += value;
        }
        let mut object = Map::new();
        object.insert("source_type".into(), json!(layer.layer.name()));
        periods(&mut object, &layer.energy);
        // The aircraft layer is one row of the list: its loud moments rank it there.
        if let (Layer::Aircraft, Some(loud)) = (layer.layer, update.aircraft_loud) {
            object.insert("loud_lden".into(), lden(&loud));
        }
        // Unrounded Lden for the benchmark's error measurement (fast against exact).
        let weighted: f64 = (0..PERIODS)
            .map(|p| PERIOD_HOURS[p] * layer.energy[p] * energy(PERIOD_PENALTY_DB[p]))
            .sum::<f64>()
            / 24.0;
        if weighted > 0.0 {
            object.insert(
                "lden_precise".into(),
                json!((10_000.0 * weighted.log10()).round() / 1000.0),
            );
        }
        let omitted = lden(&[0, 1, 2].map(|p| layer.energy[p] + layer.omitted_bound[p]));
        object.insert("lden_upper".into(), omitted);
        object.insert("evaluated".into(), json!(layer.evaluated));
        object.insert("candidates".into(), json!(layer.candidates));
        layers.push(Value::Object(object));
    }
    let mut contributors = Vec::new();
    for contributor in &update.contributors {
        let display: Value = serde_json::from_str(&(update.display_json)(
            contributor.display,
            contributor.layer,
        )?)
        .map_err(|error| error.to_string())?;
        let mut object = Map::new();
        object.insert(
            "id".into(),
            json!(format!("{:016x}", contributor.group_key)),
        );
        object.insert("source_type".into(), json!(contributor.layer.name()));
        object.insert("name".into(), json!(label(&display)));
        object.insert(
            "subtype".into(),
            display
                .get("road_class")
                .or_else(|| display.get("subtype"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        object.insert("distance_m".into(), json!(contributor.distance_m.round()));
        let mut received = Map::new();
        periods(&mut received, &contributor.energy);
        object.insert(
            "received_lden".into(),
            received.get("lden").cloned().unwrap_or(Value::Null),
        );
        object.insert("received".into(), Value::Object(received));
        if let Some(loud) = contributor.loud {
            object.insert("loud_lden".into(), lden(&loud));
        }
        object.insert("metadata".into(), display);
        if let Some(heard) = contributor.heard {
            let rate = |value: f64| (value * 100.0).round() / 100.0;
            object.insert(
                "heard".into(),
                json!({
                    "per_hour": {"day": rate(heard.per_hour[0]), "evening": rate(heard.per_hour[1]),
                        "night": rate(heard.per_hour[2])},
                    "steady": heard.steady,
                }),
            );
        }
        // What the map draws of it as lines of [lat, lon], one point for a point source: all of it
        // within the reach in the final update, its loudest pieces before.
        let degrees = |metres: &[f64; 2]| {
            let (lat, lon) = update.frame.to_mercator(*metres).to_degrees();
            json!([(lat * 1e6).round() / 1e6, (lon * 1e6).round() / 1e6])
        };
        let geometry: Vec<Value> = if contributor.lines.is_empty() {
            let mut pieces = contributor.pieces.clone();
            crate::selection::loudest_pieces(&mut pieces);
            pieces
                .iter()
                .map(|(ends, _)| {
                    if ends[0] == ends[1] {
                        json!([degrees(&ends[0])])
                    } else {
                        json!([degrees(&ends[0]), degrees(&ends[1])])
                    }
                })
                .collect()
        } else {
            contributor
                .lines
                .iter()
                .map(|line| Value::Array(line.iter().map(degrees).collect()))
                .collect()
        };
        object.insert("geometry".into(), Value::Array(geometry));
        contributors.push(Value::Object(object));
    }
    let mut pieces = Vec::new();
    for piece in &update.pieces {
        let ends: Vec<[f64; 2]> = piece
            .candidate
            .ends_m
            .iter()
            .map(|&end| {
                let (lat, lon) = update.frame.to_mercator(end).to_degrees();
                [(lat * 1e7).round() / 1e7, (lon * 1e7).round() / 1e7]
            })
            .collect();
        let mut received = Map::new();
        periods(&mut received, &piece.energy);
        let mut emission = Map::new();
        periods(&mut emission, &piece.emission);
        let round = |value: f64| (value * 100.0).round() / 100.0;
        let trace = piece.trace.as_ref().map(|trace| {
            let metres = |value: f64| (value * 10.0).round() / 10.0;
            json!({
                "profile": trace.profile.iter().map(|[distance, altitude, ground]| {
                    json!([metres(*distance), metres(*altitude), round(*ground)])
                }).collect::<Vec<_>>(),
                "source_altitude_m": metres(trace.source_altitude_m),
                "receiver_altitude_m": metres(trace.receiver_altitude_m),
                "slant_m": round(trace.slant_m),
                "p": trace.favourable_probability.map(round),
                "boundary_db": trace.boundary_db.map(round),
                "without_ground_db": trace.without_ground_db.map(round),
                "air_db": round(trace.air_db),
                "path_difference_m": trace.path_difference_m.map(round),
                "ray": trace.ray_m.map(|end| {
                    let (lat, lon) = update.frame.to_mercator(end).to_degrees();
                    [(lat * 1e6).round() / 1e6, (lon * 1e6).round() / 1e6]
                }),
            })
        });
        let candidate = &piece.candidate;
        let metadata: Value = (update.display_json)(candidate.display, candidate.layer)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        pieces.push(json!({
            "metadata": metadata,
            "trace": trace,
            "source_type": candidate.layer.name(),
            "id": format!("{:016x}", candidate.group_key),
            "ends": ends,
            "distance_m": (candidate.distance_m * 10.0).round() / 10.0,
            // Every ray the piece was summed over: [lat, lon] it leaves from, the in-plane angle
            // it stands for (rad, 0 for a point), the Lden it delivers and its terms (dB: ground
            // and screening calm and bent, screening alone calm and bent, air; slant m).
            "rays": piece.rays.iter().map(|ray| {
                let (lat, lon) = update.frame.to_mercator(ray.from_m).to_degrees();
                let tenth = |value: f64| (value * 10.0).round() / 10.0;
                let terms = ray.terms.map(|t| json!([
                    tenth(t.boundary_db[0]), tenth(t.boundary_db[1]),
                    tenth(t.without_ground_db[0]), tenth(t.without_ground_db[1]),
                    tenth(t.air_db), t.slant_m.round(),
                ]));
                json!([(lat * 1e6).round() / 1e6, (lon * 1e6).round() / 1e6,
                    (ray.angle_rad * 1e6).round() / 1e6, lden(&ray.energy), terms])
            }).collect::<Vec<_>>(),
            "received": received,
            "emission": emission,
            "crossings": piece.crossings.iter().map(|(distance_m, height_m, footprint)| {
                json!([
                    (distance_m * 10.0).round() / 10.0,
                    (height_m * 10.0).round() / 10.0,
                    format!("{footprint:016x}"),
                ])
            }).collect::<Vec<_>>(),
            "footprint": format!("{:016x}", piece.footprint_id),
        }));
    }
    let period_names = ["day", "evening", "night"];
    let flights: Vec<Value> = update
        .flights
        .iter()
        .map(|flight| {
            json!({
                "icao": format!("{:06x}", flight.icao),
                "callsign": flight.callsign,
                "type": flight.type_designator,
                "start_unix": flight.start_unix,
                "period": period_names.get(usize::from(flight.period)).copied().unwrap_or(""),
                "sel_db": (flight.sel_db * 10.0).round() / 10.0,
                "lmax_db": (flight.lmax_db * 10.0).round() / 10.0,
                "closest_m": flight.closest_m.round(),
                "altitude_m": flight.altitude_m.round(),
                "track": flight.track.iter().map(|piece| {
                    piece.map(|[lat, lon, altitude]| {
                        [(lat * 1e5).round() / 1e5, (lon * 1e5).round() / 1e5, altitude.round()]
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut totals = Map::new();
    periods(&mut totals, &total);
    let statistics = &update.statistics;
    let line = json!({
        "seq": sequence,
        "partial": update.partial,
        "center": [update.lat, update.lon],
        "elevation_m": ((update.receiver_altitude_m - crate::answer::RECEIVER_HEIGHT_M) * 10.0).round() / 10.0,
        "building": update.building.map(|click| {
            let facade = click.facade.map(|facade| {
                let (lat, lon) = update.frame.to_mercator(facade.position).to_degrees();
                json!({
                    "receiver": [(lat * 1e7).round() / 1e7, (lon * 1e7).round() / 1e7],
                    "bearing_deg": facade.outward_bearing_deg.round(),
                    "index": facade.index,
                })
            });
            json!({
                "id": format!("{:016x}", click.footprint_id),
                "height_m": (click.height_m * 10.0).round() / 10.0,
                "facade_receivers": click.receivers,
                "facade": facade,
            })
        }),
        "reflection_db": update.reflection_db,
        "total_lden": totals.get("lden"),
        "total": totals,
        "sources": layers,
        "percentiles": update.percentiles.map(|p| {
            let periods = |levels: [f64; PERIODS]| {
                let round = |value: f64| value.is_finite().then(|| (value * 10.0).round() / 10.0);
                json!({"day": round(levels[0]), "evening": round(levels[1]), "night": round(levels[2])})
            };
            json!({"l5": periods(p.l5), "l10": periods(p.l10), "l50": periods(p.l50), "l90": periods(p.l90)})
        }),
        "loudness": update.loudness.as_ref().map(|loudness| {
            // Two significant digits: 0.43, 4.3, 43.
            let round = |sone: f64| {
                let digits = (1 - sone.max(0.01).log10().floor() as i32).max(0);
                let scale = 10f64.powi(digits);
                (sone * scale).round() / scale
            };
            let n5 = loudness.n5_sone;
            json!({"n5_sone": {"day": round(n5[0]), "evening": round(n5[1]), "night": round(n5[2])},
                "n5_den_sone": round(loudness.n5_den_sone)})
        }),
        "top_contributors": contributors,
        "top_flights": flights,
        "stats": {
            "rings": statistics.rings_read,
            "files": statistics.files,
            "bytes": statistics.bytes,
            "read_ms": (statistics.read_seconds * 1000.0).round(),
            "candidate_ms": (statistics.candidate_seconds * 1000.0).round(),
            "evaluate_ms": (statistics.evaluate_seconds * 1000.0).round(),
            "elapsed_ms": (statistics.elapsed_seconds * 1000.0).round(),
        },
    });
    let mut line = line;
    if !pieces.is_empty() {
        line["pieces"] = Value::Array(pieces);
    }
    Ok(line.to_string())
}
