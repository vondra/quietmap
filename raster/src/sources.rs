//! The sources layers: every road piece in the colour of its vehicles a day, solid where a count
//! gives them, half where a model makes them (the buildings' trips, Thailand's highway table,
//! Amsterdam's traffic model), faint where a fixed estimate per class stands; every railway piece
//! (horns aside) in the colour of its trains a day, solid where timetables give all of them, half
//! where they give some, faint where none. Fainter lines are drawn first, the known ones on top.

use crate::outlines::{draw_line, near};
use crate::{Layer, MapTile, ramp};
use serde_json::Value;
use std::collections::HashMap;
use tiles::geo::{STEPS_PER_TILE, TileId};
use tiles::sources::{
    AMSTERDAM_MODEL_SOURCE_ID, BUILDING_TRAFFIC_SOURCE_ID, GUESSED_SPLIT_SOURCES,
    GUESSED_TRAIN_SOURCES, Layer as SourceLayer, NETWORK_ESTIMATE_SOURCE_IDS,
    SERVICE_TREE_SOURCE_ID, Sources, TABLE_TRAIN_SOURCES, THAI_HIGHWAYS_SOURCE_ID, display_fields,
};

/// Vehicles a day (log10) and their colours: yellow 100, orange 5,000, red 15,000, dark red 40,000.
const TRAFFIC_STOPS: [(f64, [f64; 3]); 6] = [
    (2.0, [254.0, 217.0, 118.0]),
    (3.0, [254.0, 178.0, 76.0]),
    (3.7, [253.0, 141.0, 60.0]),
    (4.18, [240.0, 59.0, 32.0]),
    (4.6, [189.0, 0.0, 38.0]),
    (5.0, [103.0, 0.0, 13.0]),
];
/// Trains a day (log10) and their colours: light blue 1, blue 10, dark blue 50, navy 150, purple 300.
const TRAIN_STOPS: [(f64, [f64; 3]); 5] = [
    (0.0, [158.0, 202.0, 225.0]),
    (1.0, [107.0, 174.0, 214.0]),
    (1.7, [33.0, 113.0, 181.0]),
    (2.18, [8.0, 48.0, 107.0]),
    (2.48, [63.0, 0.0, 125.0]),
];
/// The busiest roads' and railways' line (pixels).
pub const WIDEST_LINE_PX: f64 = 3.5;
/// How a number is known, as opacity: counted or timetabled, modelled, a fixed estimate or guess.
const COUNTED_ALPHA: u8 = 255;
const MODELLED_ALPHA: u8 = 150;
const FIXED_ALPHA: u8 = 80;

/// Where a named display field stands in a layer's array.
fn field_index(layer: SourceLayer, name: &str) -> usize {
    display_fields(layer)
        .iter()
        .position(|field| *field == name)
        .expect("a display field")
}

/// The value of a named display field as a number (absent or null: 0).
fn number(layer: SourceLayer, fields: &[Value], name: &str) -> f64 {
    fields
        .get(field_index(layer, name))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// A piece's colour, line width and vehicles or trains a day from its display fields, or `None`
/// when it carries nothing.
fn style(layer: SourceLayer, display: &str) -> Option<([u8; 4], f64, f64)> {
    let fields = serde_json::from_str::<Value>(display).ok()?;
    let fields = fields.as_array()?;
    let field = |name: &str| number(layer, fields, name);
    let (value, stops, alpha): (f64, &[(f64, [f64; 3])], u8) = match layer {
        SourceLayer::Road => {
            let vehicles = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
                .map(field)
                .iter()
                .sum();
            let source = field("source_id") as u16;
            let alpha = if GUESSED_SPLIT_SOURCES.contains(&source)
                || NETWORK_ESTIMATE_SOURCE_IDS.contains(&source)
                || source == SERVICE_TREE_SOURCE_ID
            {
                FIXED_ALPHA
            } else if [
                BUILDING_TRAFFIC_SOURCE_ID,
                THAI_HIGHWAYS_SOURCE_ID,
                AMSTERDAM_MODEL_SOURCE_ID,
            ]
            .contains(&source)
            {
                MODELLED_ALPHA
            } else {
                COUNTED_ALPHA
            };
            (vehicles, &TRAFFIC_STOPS, alpha)
        }
        _ => {
            // A horn's fields count soundings at a crossing, not trains.
            if fields
                .get(field_index(layer, "rail_type"))
                .and_then(Value::as_str)
                == Some("horn")
            {
                return None;
            }
            // Passenger and freight trains a day, each known (a timetable) or not (a guess, a table).
            let kinds = ["passenger", "freight"].map(|kind| {
                let trains: f64 = ["day", "evening", "night"]
                    .map(|period| field(&format!("trains_{kind}_{period}")))
                    .iter()
                    .sum();
                let source = field(&format!("{kind}_source_id")) as u16;
                (
                    trains,
                    !GUESSED_TRAIN_SOURCES.contains(&source)
                        && !TABLE_TRAIN_SOURCES.contains(&source),
                )
            });
            let running: Vec<bool> = kinds
                .iter()
                .filter(|(trains, _)| *trains > 0.0)
                .map(|(_, known)| *known)
                .collect();
            let alpha = if running.iter().all(|known| *known) {
                COUNTED_ALPHA
            } else if running.iter().any(|known| *known) {
                MODELLED_ALPHA
            } else {
                FIXED_ALPHA
            };
            (
                kinds.iter().map(|(trains, _)| trains).sum(),
                &TRAIN_STOPS,
                alpha,
            )
        }
    };
    if value <= 0.0 {
        return None;
    }
    let [r, g, b] = ramp(stops, value.log10()).map(|channel| channel.round() as u8);
    // Wider as busier: a tenth of the top stop's value and more is the widest, below a hundredth 1.5.
    let share = value.log10() - stops[stops.len() - 1].0;
    let width = if share >= -1.0 {
        WIDEST_LINE_PX
    } else if share >= -2.0 {
        2.5
    } else {
        1.5
    };
    Some(([r, g, b, alpha], width, value))
}

/// The map tile's pixels from the sources files of `tiles`: the roads (`Layer::Traffic`) or the
/// railways (`Layer::Trains`); points are not drawn.
pub fn render(
    layer: Layer,
    map_tile: MapTile,
    tiles: &[TileId],
    files: &[Option<Vec<u8>>],
) -> Result<Vec<[u8; 4]>, String> {
    let wanted = if layer == Layer::Traffic {
        SourceLayer::Road
    } else {
        SourceLayer::Railway
    };
    let mut lines = Vec::new();
    for (&tile, file) in tiles.iter().zip(files) {
        let Some(bytes) = file else { continue };
        let sources = Sources::parse(bytes).map_err(|error| error.to_string())?;
        let centre = map_tile.centre_of(tile);
        let to_pixel = |local: [i16; 2]| {
            [
                (centre.x + f64::from(local[0]) / STEPS_PER_TILE - map_tile.west) / map_tile.pixel,
                (centre.y + f64::from(local[1]) / STEPS_PER_TILE - map_tile.north) / map_tile.pixel,
            ]
        };
        let mut styles: HashMap<u32, Option<([u8; 4], f64, f64)>> = HashMap::new();
        for index in 0..sources.piece_count() {
            let piece = sources.piece(index).map_err(|error| error.to_string())?;
            let ends = piece.ends.map(to_pixel);
            if !piece.is_line() || !near(&ends, 4.0) {
                continue;
            }
            if sources
                .layer(piece.attribute)
                .map_err(|error| error.to_string())?
                != wanted
            {
                continue;
            }
            let style = match styles.get(&piece.attribute) {
                Some(style) => *style,
                None => {
                    let display = sources
                        .display(piece.attribute)
                        .map_err(|error| error.to_string())?;
                    *styles
                        .entry(piece.attribute)
                        .or_insert(style(wanted, display))
                }
            };
            if let Some((colour, width, value)) = style {
                lines.push((colour, width, value, ends));
            }
        }
    }
    // The better known on top, and of two as well known the busier.
    lines.sort_by(|a, b| a.0[3].cmp(&b.0[3]).then(a.2.total_cmp(&b.2)));
    let mut pixels = vec![[0; 4]; crate::PIXELS * crate::PIXELS];
    for (colour, width, _, ends) in &lines {
        draw_line(ends, *width, *colour, &mut pixels);
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A display array of `layer` with the named fields set, the rest empty strings.
    fn display(layer: SourceLayer, set: &[(&str, Value)]) -> String {
        let mut fields = vec![Value::from(""); display_fields(layer).len()];
        for (name, value) in set {
            fields[field_index(layer, name)] = value.clone();
        }
        Value::from(fields).to_string()
    }

    fn road(vehicles: f64, source: u16) -> Option<([u8; 4], f64)> {
        let fields = [
            ("aadt_light", Value::from(vehicles)),
            ("source_id", Value::from(source)),
        ];
        style(SourceLayer::Road, &display(SourceLayer::Road, &fields))
            .map(|(colour, width, _)| (colour, width))
    }

    fn rail(kind: &str, passenger: (f64, u16), freight: (f64, u16)) -> Option<([u8; 4], f64)> {
        let fields = [
            ("rail_type", Value::from(kind)),
            ("trains_passenger_day", Value::from(passenger.0)),
            ("passenger_source_id", Value::from(passenger.1)),
            ("trains_freight_night", Value::from(freight.0)),
            ("freight_source_id", Value::from(freight.1)),
        ];
        style(
            SourceLayer::Railway,
            &display(SourceLayer::Railway, &fields),
        )
        .map(|(colour, width, _)| (colour, width))
    }

    /// A road's colour follows its vehicles a day and its opacity how they are known: counted,
    /// modelled (the buildings, Amsterdam's model) or a fixed estimate (a class's prior, the service
    /// tree, a country default, a national network's class values). A source carrying none is not
    /// drawn.
    #[test]
    fn roads_take_the_colour_of_their_traffic_and_the_opacity_of_its_source() {
        // 10,000 vehicles: 0.625 of the way from orange to red, a tenth of the top stop: the widest.
        assert_eq!(
            road(10_000.0, 9003),
            Some(([245, 90, 42, COUNTED_ALPHA], WIDEST_LINE_PX))
        );
        assert_eq!(
            road(100.0, 9003),
            Some(([254, 217, 118, COUNTED_ALPHA], 1.5))
        );
        assert_eq!(
            road(100_000.0, 9003),
            Some(([103, 0, 13, COUNTED_ALPHA], WIDEST_LINE_PX))
        );
        for (source, alpha) in [
            (30, MODELLED_ALPHA),
            (31, MODELLED_ALPHA),
            (1103, MODELLED_ALPHA),
            (0, FIXED_ALPHA),
            (11, FIXED_ALPHA),
            (9865, FIXED_ALPHA),
            (1025, FIXED_ALPHA),
            (21, COUNTED_ALPHA),
        ] {
            assert_eq!(
                road(10_000.0, source).unwrap().0[3],
                alpha,
                "source {source}"
            );
        }
        assert!(road(0.0, 9003).is_none());
        assert!(style(SourceLayer::Road, "not json").is_none());
    }

    /// A railway is as solid as the share of its trains a timetable gives: all, some or none (a
    /// guess, China's or India's table, Czechia's residual); a crossing's horn is no train.
    #[test]
    fn railways_are_as_solid_as_their_timetabled_trains() {
        // 50 trains (more than a tenth of the top stop's 302: the widest): Czechia's timetable for
        // the passengers, a guess for the freight.
        assert_eq!(
            rail("rail", (40.0, 110), (10.0, 0)),
            Some(([33, 113, 181, MODELLED_ALPHA], WIDEST_LINE_PX))
        );
        assert_eq!(rail("rail", (5.0, 110), (0.0, 0)).unwrap().1, 2.5);
        assert_eq!(
            rail("rail", (40.0, 110), (0.0, 0)).unwrap().0[3],
            COUNTED_ALPHA
        );
        assert_eq!(
            rail("rail", (0.0, 110), (50.0, 0)).unwrap().0[3],
            FIXED_ALPHA
        );
        assert_eq!(
            rail("rail", (500.0, 2021), (0.0, 0)).unwrap().0[3],
            FIXED_ALPHA
        );
        assert_eq!(
            rail("rail", (2.0, 9863), (1.0, 9863)).unwrap().0[3],
            FIXED_ALPHA
        );
        assert!(rail("horn", (60.0, 112), (0.0, 0)).is_none());
        assert!(rail("rail", (0.0, 110), (0.0, 0)).is_none());
    }
}
