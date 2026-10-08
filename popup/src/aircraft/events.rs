//! What flies over the receiver: the `aircraft-events` cells around it in its tile (flights a day
//! above 50, 60 and 70 dB, of them at night, their height and type; one file, read whole).

use std::path::Path;
use tiles::aircraft_events::{AircraftEvents, BANDS, BandCell, CELLS_PER_SIDE, EventCell};
use tiles::geo::{Mercator, TileId};

/// What flies over `position` under `year_root` and the bytes read: the counts blended bilinearly
/// over the four nearest cell centres of its tile (clamped at the tile's edge), the heights by the
/// blended counts, the type of the nearest cell. A tile without a file has no flight above 50 dB
/// (the year root is complete).
pub fn events_at(year_root: &Path, position: Mercator) -> Result<(EventCell, u64), String> {
    let tile = TileId::containing(position);
    let path = tiles::tile_path(year_root, tile, tiles::Kind::AircraftEvents);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((EventCell::default(), 0));
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let events = AircraftEvents::parse(&bytes).map_err(|error| error.to_string())?;
    let side = CELLS_PER_SIDE as f64;
    // Cell-centre coordinates of the position, and the four cells around it.
    let at = |coordinate: f64, origin: u32| (coordinate - f64::from(origin)) * side - 0.5;
    let (x, y) = (at(position.x, tile.x), at(position.y, tile.y));
    let corner = |value: f64| (value.floor().max(0.0) as usize).min(CELLS_PER_SIDE - 2);
    let (column, row) = (corner(x), corner(y));
    let (u, v) = (
        (x - column as f64).clamp(0.0, 1.0),
        (y - row as f64).clamp(0.0, 1.0),
    );
    let around = [
        (column, row, (1.0 - u) * (1.0 - v)),
        (column + 1, row, u * (1.0 - v)),
        (column, row + 1, (1.0 - u) * v),
        (column + 1, row + 1, u * v),
    ]
    .map(|(column, row, weight)| (events.cell(column, row), weight));
    let nearest = |value: f64, low: usize| low + usize::from(value - low as f64 >= 0.5);
    let nearest_cell = events.cell(
        nearest(x.clamp(0.0, side - 1.0), column),
        nearest(y.clamp(0.0, side - 1.0), row),
    );
    let blend = |part: &dyn Fn(&EventCell) -> f64| -> f64 {
        around
            .iter()
            .map(|(cell, weight)| weight * part(cell))
            .sum()
    };
    let bands: [BandCell; BANDS] = std::array::from_fn(|band| {
        let per_day = blend(&|cell| f64::from(cell.bands[band].per_day));
        let height_sum = blend(&|cell| {
            f64::from(cell.bands[band].per_day) * f64::from(cell.bands[band].height_m)
        });
        BandCell {
            per_day: per_day as f32,
            night_per_day: blend(&|cell| f64::from(cell.bands[band].night_per_day)) as f32,
            height_m: if per_day > 0.0 {
                (height_sum / per_day).round() as i16
            } else {
                0
            },
            designator: nearest_cell.bands[band].designator.or_else(|| {
                around
                    .iter()
                    .filter(|(cell, _)| cell.bands[band].per_day > 0.0)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .and_then(|(cell, _)| cell.bands[band].designator)
            }),
        }
    });
    let cell = EventCell {
        bands,
        helicopters_per_day: blend(&|cell| f64::from(cell.helicopters_per_day)) as f32,
    };
    Ok((cell, bytes.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Between four cell centres the counts blend bilinearly and the heights by the blended
    /// counts; the type is the nearest cell's.
    #[test]
    fn counts_blend_between_cell_centres() {
        let root = std::env::temp_dir().join(format!("qm-events-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tile = TileId { x: 2212, y: 1387 };
        let mut cells = vec![EventCell::default(); CELLS_PER_SIDE * CELLS_PER_SIDE];
        let band = |per_day: f32, height_m: i16, designator: &[u8; 4]| BandCell {
            per_day,
            night_per_day: per_day / 10.0,
            height_m,
            designator: Some(*designator),
        };
        cells[5 * CELLS_PER_SIDE + 5].bands[0] = band(100.0, 600, b"A320");
        cells[5 * CELLS_PER_SIDE + 6].bands[0] = band(300.0, 200, b"B738");
        let path = tiles::tile_path(&root, tile, tiles::Kind::AircraftEvents);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, tiles::aircraft_events::encode(&cells)).unwrap();
        // A quarter of the way from cell (5, 5)'s centre towards (6, 5)'s.
        let side = CELLS_PER_SIDE as f64;
        let position = Mercator {
            x: f64::from(tile.x) + (5.5 + 0.25) / side,
            y: f64::from(tile.y) + 5.5 / side,
        };
        let (cell, _) = events_at(&root, position).unwrap();
        let read = cell.bands[0];
        assert!((read.per_day - 150.0).abs() < 1e-3, "{read:?}");
        assert!((read.night_per_day - 15.0).abs() < 1e-3);
        assert_eq!(read.height_m, 400);
        assert_eq!(read.designator, Some(*b"A320"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
