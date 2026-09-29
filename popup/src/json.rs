//! One streamed update as a line of JSON: totals per layer and period, the loudest contributor
//! groups with their display records, and what the click read so far.

use crate::update::Update;
use physics::bands::{PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy};
use serde_json::{Map, Value, json};

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
            display.get("road_class").cloned().unwrap_or(Value::Null),
        );
        object.insert("distance_m".into(), json!(contributor.distance_m.round()));
        let mut received = Map::new();
        periods(&mut received, &contributor.energy);
        object.insert(
            "received_lden".into(),
            received.get("lden").cloned().unwrap_or(Value::Null),
        );
        object.insert("received".into(), Value::Object(received));
        object.insert("metadata".into(), display);
        contributors.push(Value::Object(object));
    }
    let mut pieces = Vec::new();
    for piece in &update.pieces {
        let ends: Vec<[f64; 2]> = piece
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
            json!({
                "slant_m": round(trace.slant_m),
                "p": trace.favourable_probability.map(round),
                "boundary_db": trace.boundary_db.map(round),
                "without_ground_db": trace.without_ground_db.map(round),
                "foliage_db": trace.foliage_db.map(round),
                "air_db": round(trace.air_db),
                "path_difference_m": trace.path_difference_m.map(round),
            })
        });
        pieces.push(json!({
            "trace": trace,
            "source_type": piece.layer.name(),
            "id": format!("{:016x}", piece.group_key),
            "ends": ends,
            "distance_m": (piece.distance_m * 10.0).round() / 10.0,
            "received": received,
            "emission": emission,
            "crossings": piece.crossings.iter().map(|(distance_m, height_m)| {
                [(distance_m * 10.0).round() / 10.0, (height_m * 10.0).round() / 10.0]
            }).collect::<Vec<_>>(),
        }));
    }
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
        "total_lden": totals.get("lden"),
        "total": totals,
        "sources": layers,
        "top_contributors": contributors,
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
