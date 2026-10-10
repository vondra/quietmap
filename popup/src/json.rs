//! One streamed update as a line of JSON: totals per layer and period, the loudest contributor
//! groups with their display records, and what the click read so far.

use crate::update::Update;
use physics::bands::{PERIODS, lden_energy};
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
    level(lden_energy(periods))
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

/// The aircraft layer's kinds as their shares of its Lden energy (to 0.1 %, so the largest stays
/// the largest), those of at least 0.5 %.
fn aircraft_kinds(kinds: &[f64; 5]) -> Value {
    const NAMES: [&str; 5] = [
        "airliners",
        "regional_business_jets",
        "propeller",
        "helicopters",
        "ground",
    ];
    let total: f64 = kinds.iter().sum();
    let mut object = Map::new();
    if total > 0.0 {
        for (name, part) in NAMES.iter().zip(kinds) {
            let share = part / total;
            if share >= 0.005 {
                object.insert((*name).into(), json!((share * 1000.0).round() / 1000.0));
            }
        }
    }
    Value::Object(object)
}

/// A loudness to three significant digits (0.432, 4.32, 43.2): the list ranks by it, the visitor
/// reads two.
fn sone(value: f64) -> f64 {
    let digits = (2 - value.max(0.001).log10().floor() as i32).max(0);
    let scale = 10f64.powi(digits);
    (value * scale).round() / scale
}

/// A share of the click's loudness to a tenth of a percent.
fn share(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// What flies over the receiver: per band (maximum level at or above) the flights a day and at
/// night, their mean height above the ground there and type (none without flights); the
/// helicopters a day above the lowest band.
fn aircraft_events(cell: &tiles::aircraft_events::EventCell) -> Value {
    let per_day = |value: f32| (f64::from(value) * 1000.0).round() / 1000.0;
    let bands = &cell.bands;
    json!({
        "above_db": tiles::aircraft_events::EVENT_BANDS_DB,
        "per_day": bands.map(|band| per_day(band.per_day)),
        "night": bands.map(|band| per_day(band.night_per_day)),
        "height_m": bands.map(|band| (band.per_day > 0.0).then_some(band.height_m)),
        "type": bands.map(|band| {
            band.designator
                .map(|designator| String::from_utf8_lossy(&designator).trim().to_string())
        }),
        "helicopters_per_day": per_day(cell.helicopters_per_day),
    })
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
        // The aircraft layer is one row of the list: its own Nden ranks it there.
        if let Some(own) = layer.nden_sone {
            object.insert("nden_sone".into(), json!(sone(own)));
        }
        if let Some(part) = layer.share {
            object.insert("share".into(), json!(share(part)));
        }
        // What the list leaves out of a ground layer: its last row.
        if let Some(energy) = layer.unlisted {
            let mut unlisted = Map::new();
            periods(&mut unlisted, &energy);
            object.insert("unlisted".into(), Value::Object(unlisted));
            object.insert("unlisted_sources".into(), json!(layer.unlisted_sources));
        }
        if let (Layer::Aircraft, Some(kinds)) = (layer.layer, update.aircraft_kinds) {
            object.insert("kinds".into(), aircraft_kinds(&kinds));
        }
        if let (Layer::Aircraft, Some(events)) = (layer.layer, &update.aircraft_events) {
            object.insert("events".into(), aircraft_events(events));
        }
        // Unrounded Lden for the benchmark's error measurement (fast against exact).
        let weighted = lden_energy(&layer.energy);
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
        let display = (update.display_record)(contributor.display, contributor.layer)?;
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
        if let Some(own) = contributor.nden_sone {
            object.insert("nden_sone".into(), json!(sone(own)));
        }
        if let Some(part) = contributor.share {
            object.insert("share".into(), json!(share(part)));
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
        // within the reach in the final update (what the shared budget leaves it, maybe nothing),
        // its loudest pieces before.
        let degrees = |metres: &[f64; 2]| {
            let (lat, lon) = update.frame.to_mercator(*metres).to_degrees();
            json!([(lat * 1e6).round() / 1e6, (lon * 1e6).round() / 1e6])
        };
        let geometry: Vec<Value> = if update.partial {
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
            .take(if piece.candidate.line { 2 } else { 1 })
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
        let metadata =
            (update.display_record)(candidate.display, candidate.layer).unwrap_or(Value::Null);
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
            "source_lden": lden(&piece.source_energy),
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
                "track": flight.track.iter().map(|line| {
                    line.iter().map(|[lat, lon]| {
                        [(lat * 1e5).round() / 1e5, (lon * 1e5).round() / 1e5]
                    }).collect::<Vec<_>>()
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
        // The place's weather (final update): the percent of each period the weather bends sound
        // down along each of 16 bearings (the direction the sound travels, clockwise from north),
        // and the air's absorption per octave band (dB/km).
        "weather": update.weather.map(|weather| json!({
            "favourable_percent": weather.favourable.by_sector.map(|row| row.map(|p| (p * 100.0).round() as u8)),
            "alpha_db_per_km": weather.alpha_db_per_km.map(|alpha| (alpha * 100.0).round() / 100.0),
        })),
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
            let mean = loudness.mean_sone;
            json!({"mean_sone": {"day": sone(mean[0]), "evening": sone(mean[1]), "night": sone(mean[2])},
                "nden_sone": sone(loudness.nden_sone)})
        }),
        "rest_nden_sone": update.rest_nden_sone.map(sone),
        "rest_share": update.rest_share.map(share),
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
