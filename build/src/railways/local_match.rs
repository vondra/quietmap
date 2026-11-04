//! Apply nearest stop or source line observations and ordered corridor priors to prepared railway pieces.
use super::{
    matcher::{Claims, interval},
    model::RailModel,
    movements::{Location, Movement},
    network, prior_rules, write,
};
use arrow_array::{Array, StringArray};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

type Cells = HashMap<(i32, i32), Vec<usize>>;
fn cell(point: [f64; 2]) -> (i32, i32) {
    (
        (point[0] * 100.0).floor() as i32,
        (point[1] * 100.0).floor() as i32,
    )
}

fn observations(movements: &[Movement]) -> Vec<Movement> {
    let mut rows = Vec::<Movement>::new();
    let mut duplicates = HashMap::new();
    for movement in movements {
        match &movement.location {
            Location::Station { stop } => {
                let key = format!(
                    "{:.4}/{:.4}/{:?}",
                    stop.latitude, stop.longitude, movement.rail_types
                );
                if let Some(&index) = duplicates.get(&key) {
                    super::station_pairs::add_counts(&mut rows[index], movement);
                } else {
                    duplicates.insert(key, rows.len());
                    rows.push(movement.clone());
                }
            }
            Location::Line { .. } | Location::Region { .. } => rows.push(movement.clone()),
            _ => (),
        }
    }
    rows
}

fn grids(rows: &[Movement]) -> Vec<Cells> {
    let mut groups = Vec::<Cells>::new();
    let mut kinds = HashMap::new();
    for (index, movement) in rows.iter().enumerate() {
        let key = movement
            .attributes
            .get("kind")
            .map(String::as_str)
            .unwrap_or("");
        let group = *kinds.entry(key).or_insert_with(|| {
            groups.push(Cells::new());
            groups.len() - 1
        });
        let mut cells = HashSet::new();
        match &movement.location {
            Location::Station { stop } => {
                cells.insert(cell([stop.latitude, stop.longitude]));
            }
            Location::Line { coordinates, .. } => {
                for pair in coordinates.windows(2) {
                    let steps = ((pair[1][0] - pair[0][0])
                        .abs()
                        .max((pair[1][1] - pair[0][1]).abs())
                        / 0.005)
                        .ceil()
                        .max(1.0) as usize;
                    for step in 0..=steps {
                        let fraction = step as f64 / steps as f64;
                        cells.insert(cell([
                            pair[0][0] + (pair[1][0] - pair[0][0]) * fraction,
                            pair[0][1] + (pair[1][1] - pair[0][1]) * fraction,
                        ]));
                    }
                }
            }
            _ => (),
        }
        for cell in cells {
            groups[group].entry(cell).or_default().push(index);
        }
    }
    groups
}

fn nearest<'a>(
    rows: &'a [Movement],
    grids: &[Cells],
    kind: u8,
    a: [f64; 2],
    b: [f64; 2],
    radius: f64,
) -> Option<&'a Movement> {
    let midpoint = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let (y, x) = cell(midpoint);

    let dy = (radius / 1111.949).ceil().max(1.0) as i32;
    let dx = (radius / (1111.949 * (midpoint[0] * std::f64::consts::PI / 180.0).cos().max(0.05)))
        .ceil()
        .max(1.0) as i32;
    let mut seen = HashSet::new();
    let mut nearest = None;
    let mut best = radius;
    for grid in grids {
        for oy in -dy..=dy {
            for ox in -dx..=dx {
                for &index in grid.get(&(y + oy, x + ox)).into_iter().flatten() {
                    if !seen.insert(index) {
                        continue;
                    }
                    let movement = &rows[index];
                    if !movement.rail_types.contains(&kind) {
                        continue;
                    }
                    let (distance, limit) = match &movement.location {
                        Location::Station { stop } => (
                            network::project([stop.latitude, stop.longitude], a, b).0,
                            500.0,
                        ),
                        Location::Line {
                            coordinates,
                            radius_m,
                        } => (network::distance_to_line(midpoint, coordinates), *radius_m),
                        _ => continue,
                    };
                    if distance < best && distance < limit {
                        best = distance;
                        nearest = Some(movement);
                    }
                }
            }
        }
    }
    nearest
}

pub fn apply(
    model: &RailModel,
    geography: &Path,
    movements: &[Movement],
    squares: &HashSet<[u16; 2]>,
    output: &mut Claims,
) -> Result<(), String> {
    let observations = observations(movements);
    if observations.is_empty() {
        return Ok(());
    }
    let grids = grids(&observations);
    let radius = observations
        .iter()
        .filter_map(|movement| match movement.location {
            Location::Station { .. } => Some(500.0),
            Location::Line { radius_m, .. } => Some(radius_m),
            _ => None,
        })
        .fold(0.0, f64::max);
    for square in squares {
        let directory = geography.join(format!("z9/{}/{}", square[0], square[1]));
        let Some(rails) = crate::dev4::read_table(&directory.join("railways.arrow"))? else {
            continue;
        };
        let pieces = super::topology::load_square_pieces(&directory)?;
        for batch in &rails.batches {
            for row in 0..batch.num_rows() {
                if write::col_u8(batch, "service")?.value(row) != 0 {
                    continue;
                }
                let kind = write::col_u8(batch, "rail_type")?.value(row);
                let country = write::col_u16(batch, "country_iso")?
                    .value(row)
                    .to_le_bytes();
                if !model.local_source_applies(
                    observations[0].source_id,
                    &observations[0].country,
                    country,
                ) {
                    continue;
                }
                let point = |side| -> Result<[f64; 2], String> {
                    let (lon, lat) = crate::osm::geometry::grid_cell_lonlat(
                        write::col_i32(batch, &format!("{side}_gx"))?.value(row),
                        write::col_i32(batch, &format!("{side}_gy"))?.value(row),
                    );
                    Ok([lat, lon])
                };
                let a = point("start")?;
                let b = point("end")?;
                let midpoint = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                let region = observations
                    .iter()
                    .find(|movement| match movement.location {
                        Location::Region { bounds } => {
                            movement.rail_types.contains(&kind)
                                && (bounds[0]..=bounds[2]).contains(&midpoint[0])
                                && (bounds[1]..=bounds[3]).contains(&midpoint[1])
                        }
                        _ => false,
                    });
                let Some(observation) =
                    region.or_else(|| nearest(&observations, &grids, kind, a, b, radius))
                else {
                    continue;
                };
                if !model.local_source_applies(observation.source_id, &observation.country, country)
                {
                    continue;
                }
                let mut movement = observation.clone();
                if matches!(
                    movement.location,
                    Location::Line { .. } | Location::Region { .. }
                ) {
                    let rules = model
                        .spatial_priors
                        .get(&movement.source_id)
                        .ok_or_else(|| {
                            format!("rail model lacks priors for {}", movement.source_id)
                        })?;
                    let names = batch
                        .column_by_name("name")
                        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
                        .ok_or("rail geography lacks name")?;
                    let sample = prior_rules::Observation {
                        point: midpoint,
                        rail_type: kind,
                        usage: write::col_u8(batch, "usage")?.value(row),
                        name: if names.is_null(row) {
                            ""
                        } else {
                            names.value(row)
                        },
                        attributes: &movement.attributes,
                    };
                    let Some([passenger, freight]) = prior_rules::classify(rules, &sample) else {
                        continue;
                    };
                    movement.passenger = (passenger > 0.0).then_some(passenger);
                    movement.passenger_status = if passenger > 0.0 { 2 } else { 0 };
                    movement.freight = (freight > 0.0).then_some(freight);
                    movement.freight_status = if freight > 0.0 { 2 } else { 0 };
                }
                let way = write::col_i64(batch, "osm_id")?.value(row);
                let segment = write::col_i16(batch, "segment_idx")?.value(row);
                let piece = pieces
                    .get(&(way, segment))
                    .ok_or("local railway observation lacks source piece")?;
                output.entry(*square).or_default().push((
                    interval(&movement, way, segment, piece.from_m, piece.to_m, 0),
                    0,
                ));
            }
        }
    }
    Ok(())
}
