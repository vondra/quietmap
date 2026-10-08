//! What flies over the receiver: the `aircraft-events` cell holding it (flights a day above 50, 60
//! and 70 dB, of them at night, their height and type; the builder's z17 cell, read whole).

use std::path::Path;
use tiles::aircraft_events::{AircraftEvents, CELLS_PER_SIDE, EventCell};
use tiles::geo::{Mercator, TileId};

/// The cell holding `position` under `year_root` and the bytes read; a tile without a file has no
/// flight above 50 dB (the year root is complete).
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
    let index = |at: f64, origin: u32| {
        (((at - f64::from(origin)) * CELLS_PER_SIDE as f64).floor() as usize)
            .min(CELLS_PER_SIDE - 1)
    };
    let cell = events.cell(index(position.x, tile.x), index(position.y, tile.y));
    Ok((cell, bytes.len() as u64))
}
