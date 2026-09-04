//! The `tiles=x,y,side` window a `--stream` line may attach to one cell, and the aircraft
//! painters' stream-line reader that parses it.
//!
//! One window definition serves every stream painter — surface, GPU airborne, CPU
//! cruise/airborne — so a short release check can paint the same bounded area on all seven
//! layers instead of one layer's window against another layer's whole cell.
//!
//! The window narrows only the tiles a cell WRITES. The cell still loads its whole
//! `grid_disk(1)` source ring and screens every receiver against it, so a windowed tile is
//! byte-identical to the same tile painted as part of the whole cell — the property that
//! lets a windowed check compare against a whole-cell reference.

use std::str::FromStr;

use anyhow::{bail, Context, Result};
use h3o::{CellIndex, Resolution};

use crate::region_runner::region_tiles;

/// A square Web-Mercator tile window used by short release checks. The streamed cell still
/// owns the source read set; this only narrows its writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileWindow {
    pub x: u32,
    pub y: u32,
    pub side: u32,
}

impl TileWindow {
    /// The tiles of `tiles` inside this window. An empty intersection is an error: the
    /// caller addressed a cell that owns nothing it asked for, and painting zero tiles
    /// would report `done` for an area nobody painted.
    pub fn select(self, tiles: Vec<(u32, u32)>) -> Result<Vec<(u32, u32)>> {
        let x_end = self
            .x
            .checked_add(self.side)
            .context("tile-window x range overflows")?;
        let y_end = self
            .y
            .checked_add(self.side)
            .context("tile-window y range overflows")?;
        let selected: Vec<_> = tiles
            .into_iter()
            .filter(|(x, y)| *x >= self.x && *x < x_end && *y >= self.y && *y < y_end)
            .collect();
        if selected.is_empty() {
            bail!(
                "tile-window {},{},{} selects no tiles owned by this cell",
                self.x,
                self.y,
                self.side
            );
        }
        Ok(selected)
    }
}

impl FromStr for TileWindow {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let coordinates: Vec<_> = value.split(',').collect();
        if coordinates.len() != 3 {
            bail!("tile-window must be x,y,side");
        }
        let parse = |field: &str, name: &str| {
            field
                .parse::<u32>()
                .with_context(|| format!("tile-window {name} is not an unsigned integer"))
        };
        let window = Self {
            x: parse(coordinates[0], "x")?,
            y: parse(coordinates[1], "y")?,
            side: parse(coordinates[2], "side")?,
        };
        if window.side == 0 {
            bail!("tile-window side must be positive");
        }
        Ok(window)
    }
}

/// The R4 cell a stream line names. Every stream painter validates here: the tile store
/// owns R4 cells only and [`region_tiles`] panics on anything else, so a mistyped cell must
/// become one refused line rather than a process that dies with the queue behind it.
pub fn parse_r4_cell_hex(hex: &str) -> Result<u64> {
    let region_r4 =
        u64::from_str_radix(hex, 16).with_context(|| format!("{hex:?} is not hexadecimal"))?;
    let cell = CellIndex::try_from(region_r4).context("invalid H3 cell")?;
    if cell.resolution() != Resolution::Four {
        bail!(
            "resolution {} is not the R4 the tile store owns",
            u8::from(cell.resolution())
        );
    }
    Ok(region_r4)
}

/// The tiles one streamed cell writes: everything the cell owns, INTERSECTED with `window`
/// when the line carried one. Every stream painter resolves its write set through this one
/// rule, so a windowed cell and a whole-cell run can never disagree about which tiles are its
/// own.
///
/// A window is a square in tile space, not a cell selector, so it usually overlaps several
/// cells and each of them paints only its own share — that is how the release check's 4x4
/// square is painted exactly once by the two cells that own it. An empty intersection is a
/// refusal, never a whole-cell paint: the caller named a cell that owns nothing it asked for,
/// and painting the whole cell would silently deliver a hundredfold of the work it budgeted.
pub fn streamed_cell_tiles(
    region_r4: u64,
    zoom: u8,
    window: Option<TileWindow>,
) -> Result<Vec<(u32, u32)>> {
    let owned = region_tiles(region_r4, zoom);
    match window {
        None => Ok(owned),
        Some(window) => window
            .select(owned)
            .context("select the requested tile window"),
    }
}

/// One aircraft `--stream` stdin line: the R4 cell to paint and the optional window that
/// narrows its writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamedAircraftCell {
    pub region_r4: u64,
    pub tile_window: Option<TileWindow>,
}

/// Parse one aircraft `--stream` line: `<r4hex>`, optionally followed by `tiles=x,y,side` —
/// the same token the surface painter reads. Blank and `#` comment lines carry no work
/// (`Ok(None)`). Without the token the cell paints every tile it owns, which is what a world
/// task sends (`scripts/world/worker.py` in the ops repository writes bare cell ids).
///
/// The window is INTERSECTED with the cell's own tiles ([`streamed_cell_tiles`]); a cell whose
/// share of the square is empty fails by name. A malformed window is an error too, never a
/// silently whole-cell paint: the caller asked for a bounded area and would otherwise get every
/// tile the cell owns — a hundredfold of the work it budgeted, over tiles it never named.
pub fn parse_aircraft_stream_line(line: &str) -> Result<Option<StreamedAircraftCell>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let mut tokens = line.split_whitespace();
    let hex = tokens.next().expect("a non-empty line has a first token");
    let mut tile_window = None;
    for token in tokens {
        let value = token
            .strip_prefix("tiles=")
            .with_context(|| format!("only tiles=x,y,side is understood, not {token:?}"))?;
        let window: TileWindow = value.parse().context("invalid tiles=")?;
        if tile_window.replace(window).is_some() {
            bail!("tiles= may be given only once");
        }
    }
    Ok(Some(StreamedAircraftCell {
        region_r4: parse_r4_cell_hex(hex)?,
        tile_window,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dobris and its western neighbour: the two cells that own the 4x4 z13 window the
    /// release check paints.
    const DOBRIS: u64 = 0x841e309ffffffff;
    const NEIGHBOUR: u64 = 0x841e301ffffffff;
    const ZOOM: u8 = 13;

    fn cell(line: &str) -> StreamedAircraftCell {
        parse_aircraft_stream_line(line)
            .unwrap_or_else(|error| panic!("{line:?} did not parse: {error:#}"))
            .unwrap_or_else(|| panic!("{line:?} carried no work"))
    }

    fn refusal(line: &str) -> String {
        format!(
            "{:#}",
            parse_aircraft_stream_line(line).expect_err(&format!("{line:?} was accepted"))
        )
    }

    #[test]
    fn a_bare_cell_line_paints_every_tile_the_cell_owns() {
        assert_eq!(
            cell("841e309ffffffff"),
            StreamedAircraftCell {
                region_r4: DOBRIS,
                tile_window: None,
            }
        );
        assert_eq!(
            streamed_cell_tiles(DOBRIS, ZOOM, None).unwrap(),
            region_tiles(DOBRIS, ZOOM)
        );
    }

    #[test]
    fn blank_and_comment_lines_carry_no_work() {
        assert_eq!(parse_aircraft_stream_line("").unwrap(), None);
        assert_eq!(parse_aircraft_stream_line("   ").unwrap(), None);
        assert_eq!(
            parse_aircraft_stream_line("# a cell list header").unwrap(),
            None
        );
    }

    /// One bug class: a line the painter cannot turn into the work it names must be refused,
    /// never degraded into a whole-cell paint or a panic inside `region_tiles`.
    #[test]
    fn an_unusable_line_is_refused_rather_than_painted_whole() {
        assert!(refusal("zzz").contains("hexadecimal"));
        assert!(refusal("0").contains("invalid H3 cell"));
        let finer = CellIndex::try_from(DOBRIS)
            .unwrap()
            .center_child(Resolution::Five)
            .unwrap();
        assert!(refusal(&format!("{finer:x}")).contains("resolution"));
        assert!(refusal("841e309ffffffff layers=road").contains("only tiles="));
        assert!(refusal("841e309ffffffff tiles=4414,2786").contains("must be x,y,side"));
        assert!(refusal("841e309ffffffff tiles=4414,2786,0").contains("positive"));
        assert!(refusal("841e309ffffffff tiles=4414,2786,x").contains("unsigned integer"));
        assert!(refusal("841e309ffffffff tiles=4414,2786,4 tiles=1,2,3").contains("only once"));
        // A window that overlaps no tile of this cell fails by name; it must never fall back
        // to painting the whole cell.
        let empty = streamed_cell_tiles(
            DOBRIS,
            ZOOM,
            Some(TileWindow {
                x: 0,
                y: 0,
                side: 4,
            }),
        )
        .expect_err("a window this cell owns nothing in must be refused");
        assert!(format!("{empty:#}").contains("selects no tiles owned by this cell"));
    }

    /// The window the release check paints: 13 of its 16 tiles belong to Dobris and the
    /// three southern ones to the western neighbour, so the two cells together write the
    /// whole 4x4 square exactly once.
    #[test]
    fn a_tile_window_selects_each_cells_own_share_of_the_square() {
        let parsed = cell("841e309ffffffff tiles=4414,2786,4");
        let window = TileWindow {
            x: 4414,
            y: 2786,
            side: 4,
        };
        assert_eq!(parsed.tile_window, Some(window));
        let dobris = streamed_cell_tiles(DOBRIS, ZOOM, Some(window)).unwrap();
        let neighbour = streamed_cell_tiles(NEIGHBOUR, ZOOM, Some(window)).unwrap();
        assert_eq!(dobris.len(), 13);
        assert_eq!(neighbour, vec![(4415, 2789), (4416, 2789), (4417, 2789)]);
        let mut square: Vec<_> = dobris.into_iter().chain(neighbour).collect();
        square.sort_unstable();
        square.dedup();
        assert_eq!(square.len(), 16);
        assert_eq!(square.first(), Some(&(4414, 2786)));
        assert_eq!(square.last(), Some(&(4417, 2789)));
    }
}
