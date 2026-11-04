//! Compare aircraft boxes and event counts with the diagnostic flight reference.
use crate::{boxes, cli::*, year};
use physics::weather::WeatherTable;
use std::path::Path;

/// An events table as JSON: per band its threshold, flights a day by period, mean height (m) and
/// most frequent type; helicopters a day in the lowest band.
fn events_json(events: &boxes::events::EventCounts) -> serde_json::Value {
    let bands: Vec<serde_json::Value> = boxes::events::EVENT_BANDS_DB
        .iter()
        .enumerate()
        .map(|(band, above_db)| {
            serde_json::json!({
                "above_db": above_db,
                "per_day": events.per_day[band],
                "height_m": events.mean_height_m(band),
                "type": events
                    .top_designator(band)
                    .map(|designator| String::from_utf8_lossy(&designator).trim().to_string()),
            })
        })
        .collect();
    serde_json::json!({"bands": bands, "helicopters_per_day": events.helicopters})
}

pub(crate) fn run(
    command: &str,
    options: &Arguments,
    inputs: &year::YearInputs,
    out: &Path,
) -> Result<(), String> {
    match command {
        "aircraft-check" => {
            let window = inputs.manifest.window();
            let points: Vec<serde_json::Value> = serde_json::from_str(
                &std::fs::read_to_string(options.get("points")?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let only: Vec<&str> = options
                .optional("only")
                .map_or(Vec::new(), |names| names.split(',').collect());
            let terrain = options.terrain(inputs, out)?;
            let chosen: Vec<(&str, boxes::check::CheckPoint)> = points
                .iter()
                .map(|point| {
                    let (lat, lon) = (
                        point["lat"].as_f64().unwrap_or(0.0),
                        point["lon"].as_f64().unwrap_or(0.0),
                    );
                    let ground_m = boxes::check::ground_at(terrain, (lat, lon));
                    let name = point["name"].as_str().unwrap_or("");
                    (name, boxes::check::CheckPoint { lat, lon, ground_m })
                })
                .filter(|(name, _)| only.is_empty() || only.contains(name))
                .collect();
            let diagnosed = match options.optional("diagnose") {
                Some(name) => Some(
                    chosen
                        .iter()
                        .position(|(chosen, _)| *chosen == name)
                        .ok_or_else(|| format!("--diagnose {name}: no such point"))?,
                ),
                None => None,
            };
            let weather = WeatherTable::read(inputs.required(crate::year::Kind::Weather)?)?;
            let compared = boxes::check::compare(
                (inputs.required(crate::year::Kind::Segments)?, &weather),
                &window,
                (out, box_rules(options)?[0].level_step_db),
                terrain,
                (
                    &chosen.iter().map(|(_, point)| *point).collect::<Vec<_>>(),
                    diagnosed,
                ),
            )?;
            for ((name, _), report) in chosen.iter().zip(compared) {
                println!(
                    "{}",
                    serde_json::json!({
                        "point": name,
                        "exact_leq": report.exact,
                        "boxed_leq": report.boxed,
                        "beyond_reach_leq": report.beyond,
                        "boxed_as_read_leq": report.boxed_as_read,
                        "megabytes_fine_as_read": report.megabytes,
                        "near_ground": {
                            "exact_leq": report.near_ground[0],
                            "boxed_leq": report.near_ground[1],
                        },
                        "by_distance": report.by_distance,
                        "aloft": {
                            "exact_leq": report.aloft[0],
                            "boxed_leq": report.aloft[1],
                        },
                        "exact_events": events_json(&report.events),
                        "written_events": report.events_written.bands.map(|band| {
                            serde_json::json!([band.per_day, band.night_per_day, band.height_m])
                        }),
                        "exact_top": report
                            .exact_top
                            .iter()
                            .map(|(flight, sel)| serde_json::json!([format!("{flight:016x}"), sel]))
                            .collect::<Vec<_>>(),
                        "diagnosis": report
                            .diagnosis
                            .iter()
                            .map(|found| serde_json::json!({
                                "centroid": found.centroid,
                                "distance_m": found.distance_m,
                                "zoom": found.zoom,
                                "clearance_m": found.clearance_m,
                                "flights_per_day": found.flights_per_day,
                                "axis_deg": found.axis_deg,
                                "gradient": found.gradient,
                                "gradient_spread": found.gradient_spread,
                                "piece_length_m": found.piece_length_m,
                                "exact_db": found.exact_db,
                                "boxed_db": found.boxed_db,
                                "exact_altitude_m": found.exact_altitude_m,
                                "exact_scaled_distance_m": found.exact_scaled_distance_m,
                                "boxed_scaled_distance_m": found.boxed_scaled_distance_m,
                                "exact_offset_m": found.exact_offset_m,
                                "exact_length_m": found.exact_length_m,
                            }))
                            .collect::<Vec<_>>(),
                        "lists": report
                            .lists
                            .iter()
                            .map(|list| serde_json::json!({
                                "pieces": list.pieces,
                                "search_is_exhaustive": list.search_is_exhaustive,
                                "recall": list.recall,
                                "tolerant_recall": list.tolerant_recall,
                                "listed": list
                                    .listed
                                    .iter()
                                    .map(|(flight, sel, gap)| {
                                        serde_json::json!([format!("{flight:016x}"), sel, gap])
                                    })
                                    .collect::<Vec<_>>(),
                            }))
                            .collect::<Vec<_>>(),
                    })
                );
            }
            Ok(())
        }
        other => Err(format!("unknown aircraft command {other}")),
    }
}
