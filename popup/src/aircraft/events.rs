//! Flights a day above 50, 60 and 70 dB at the receiver, interpolated from the aircraft-event grid.

use std::path::Path;
use tiles::aircraft_events::{AircraftEvents, BANDS, BandCell, CELLS_PER_SIDE, EventCell};
use tiles::geo::{Mercator, TILES_PER_AXIS, TileId};

/// Blend the four surrounding counts bilinearly and weight heights by the flights in each count.
/// Return the estimate, number of tile payloads read and their bytes. In a complete year an absent
/// tile means no flights crossed a threshold there.
pub fn events_at(year_root: &Path, position: Mercator) -> Result<(EventCell, usize, u64), String> {
    let side = CELLS_PER_SIDE as u32;
    let world = TILES_PER_AXIS * side;
    let x = position.x * f64::from(side) - 0.5;
    let y = (position.y * f64::from(side) - 0.5).clamp(0.0, f64::from(world - 1));
    let (west, north) = (x.floor() as i32, y.floor() as u32);
    let (u, v) = (x - x.floor(), y - y.floor());
    let weights = [(1.0 - u) * (1.0 - v), u * (1.0 - v), (1.0 - u) * v, u * v];
    let corners = [
        (west, north),
        (west + 1, north),
        (west, north + 1),
        (west + 1, north + 1),
    ]
    .map(|(column, row)| {
        let column = column.rem_euclid(world as i32) as u32;
        let row = row.min(world - 1);
        (
            TileId {
                x: column / side,
                y: row / side,
            },
            (column % side) as usize,
            (row % side) as usize,
        )
    });
    let mut tiles = Vec::with_capacity(4);
    for ((tile, _, _), weight) in corners.into_iter().zip(weights) {
        if weight > 0.0 && !tiles.contains(&tile) {
            tiles.push(tile);
        }
    }
    let mut around = weights.map(|weight| (EventCell::default(), weight));
    let payloads = tiles::square::read_held(year_root, tiles::Kind::AircraftEvents, tiles)?;
    let reads = payloads.len();
    let mut bytes_read = 0;
    for (tile, bytes) in payloads {
        let events = AircraftEvents::parse(&bytes).map_err(|error| error.to_string())?;
        bytes_read += bytes.len() as u64;
        for (i, (source_tile, column, row)) in corners.into_iter().enumerate() {
            if source_tile == tile {
                around[i].0 = events.cell(column, row);
            }
        }
    }
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
            designator: around
                .iter()
                .map(|(cell, weight)| {
                    (
                        cell.bands[band],
                        weight * f64::from(cell.bands[band].per_day),
                    )
                })
                .filter(|(_, flights)| *flights > 0.0)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .and_then(|(band, _)| band.designator),
        }
    });
    let cell = EventCell {
        bands,
        helicopters_per_day: blend(&|cell| f64::from(cell.helicopters_per_day)) as f32,
    };
    Ok((cell, reads, bytes_read))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The farther cell contributes more flights here, including their height and example type.
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
        tiles::square::write_tiles(
            &root,
            tiles::Kind::AircraftEvents,
            &[(tile, tiles::aircraft_events::encode(&cells))],
        )
        .unwrap();
        // Three eighths of the way from cell (5, 5)'s centre towards (6, 5)'s.
        let side = CELLS_PER_SIDE as f64;
        let position = Mercator {
            x: f64::from(tile.x) + (5.5 + 0.375) / side,
            y: f64::from(tile.y) + 5.5 / side,
        };
        let (cell, reads, _) = events_at(&root, position).unwrap();
        assert_eq!(reads, 1);
        let read = cell.bands[0];
        assert!((read.per_day - 175.0).abs() < 1e-3, "{read:?}");
        assert!((read.night_per_day - 17.5).abs() < 1e-3);
        assert_eq!(read.height_m, 343);
        assert_eq!(read.designator, Some(*b"B738"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_tile_corner_blends_all_four_samples_including_an_absent_tile() {
        let root = std::env::temp_dir().join(format!("qm-events-corner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let last = tiles::geo::TILES_PER_AXIS - 1;
        let side = CELLS_PER_SIDE;
        // Across the date line and a square edge. The south-east tile has no flights.
        let tiles = [
            (last, 7, side * side - 1, 8.0, 100, b"A320"),
            (0, 7, side * (side - 1), 4.0, 200, b"B738"),
            (last, 8, side - 1, 4.0, 400, b"B738"),
        ]
        .map(|(x, y, index, per_day, height, designator)| {
            let mut cells = vec![EventCell::default(); side * side];
            cells[index].bands[0] = BandCell {
                per_day,
                night_per_day: per_day / 2.0,
                height_m: height,
                designator: Some(*designator),
            };
            cells[index].helicopters_per_day = per_day / 4.0;
            (TileId { x, y }, tiles::aircraft_events::encode(&cells))
        });
        tiles::square::write_tiles(&root, tiles::Kind::AircraftEvents, &tiles).unwrap();
        let (cell, reads, _) = events_at(&root, Mercator { x: 0.0, y: 8.0 }).unwrap();
        assert_eq!(reads, 3);
        let band = cell.bands[0];
        assert_eq!(band.per_day, 4.0);
        assert_eq!(band.night_per_day, 2.0);
        assert_eq!(cell.helicopters_per_day, 1.0);
        assert_eq!(band.height_m, 200);
        assert_eq!(band.designator, Some(*b"A320"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
