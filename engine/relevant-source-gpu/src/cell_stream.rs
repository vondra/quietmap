//! The `--stream` wire of the surface painter: one cell per stdin line in, one
//! `start` / `done` / `fail` line per cell out on stderr.
//!
//! STDERR is the protocol's stream and stdout carries nothing at all, so a
//! supervisor reads one stream and sees the engine's own library output
//! interleaved with the lifecycle lines in the order it happened.
//!
//! The orchestrator parses those three prefixes and nothing else, so every
//! line here is one line: a `fail` message is collapsed to single spaces
//! before it is written.

use std::io::{BufRead, Write};

use h3o::{CellIndex, Resolution};
use tile_painter::region_runner::{split_stream_line, stream_cell_started_line};

use crate::surface_layers::{LAYER_COUNT, LAYER_NAMES};

/// One cell to paint: the R4 cell and the layers it wants, as indices into
/// [`LAYER_NAMES`] in output order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamedCell {
    pub region_r4: u64,
    pub layers: Vec<usize>,
}

impl StreamedCell {
    pub fn label(&self) -> String {
        format!("{:x}", self.region_r4)
    }
}

/// One stdin line that carries work: a cell to paint, or a line this painter
/// refuses. A refused line is reported as `fail` and counted, never silently
/// skipped — a dropped cell would leave the orchestrator waiting forever for
/// tiles nobody paints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellRequest {
    Cell(StreamedCell),
    Rejected { cell: String, message: String },
}

fn rejected(cell: &str, message: impl Into<String>) -> CellRequest {
    CellRequest::Rejected {
        cell: cell.to_owned(),
        message: message.into(),
    }
}

/// Parse one stdin line: `<r4hex>` or `<r4hex> layers=<csv>`. Blank lines and
/// `#` comments carry no work at all, so they are neither painted nor failed.
///
/// An unknown layer name is refused rather than ignored: silently painting the
/// names it did recognise would report `done` for a cell whose missing layer
/// the orchestrator then collects as an absent tile.
pub fn parse_cell_line(line: &str) -> Option<CellRequest> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (hex, requested) = split_stream_line(line);
    let Ok(region_r4) = u64::from_str_radix(hex, 16) else {
        return Some(rejected(hex, "not a hexadecimal H3 cell"));
    };
    match CellIndex::try_from(region_r4) {
        Ok(cell) if cell.resolution() == Resolution::Four => {}
        Ok(cell) => {
            return Some(rejected(
                hex,
                format!(
                    "resolution {} is not the R4 the tile store owns",
                    u8::from(cell.resolution())
                ),
            ))
        }
        Err(error) => return Some(rejected(hex, format!("invalid H3 cell: {error}"))),
    }
    let layers = match requested {
        None => (0..LAYER_COUNT).collect(),
        Some(names) => {
            let mut layers: Vec<usize> = Vec::with_capacity(names.len());
            for name in names {
                let Some(layer) = LAYER_NAMES.iter().position(|known| *known == name) else {
                    return Some(rejected(
                        hex,
                        format!("layers= names {name}, which this painter does not paint"),
                    ));
                };
                if !layers.contains(&layer) {
                    layers.push(layer);
                }
            }
            layers.sort_unstable();
            layers
        }
    };
    if layers.is_empty() {
        return Some(rejected(
            hex,
            "layers= selected none of this painter's layers",
        ));
    }
    Some(CellRequest::Cell(StreamedCell { region_r4, layers }))
}

/// The cells as they arrive. The reader is consumed line by line and never
/// drained ahead, so a world run that feeds cells over hours paints the first
/// one immediately. A read error on the pipe ends the stream like EOF: the
/// orchestrator's own supervision owns a broken pipe, the painter just stops.
pub fn cell_requests<R: BufRead>(reader: R) -> impl Iterator<Item = CellRequest> {
    reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| parse_cell_line(&line))
}

/// `start <cell> <unix_ms>`: this cell's painting begins now. Every write here
/// is flushed because stderr is a pipe the orchestrator reads live.
pub fn report_cell_started(region_r4: u64) {
    write_protocol_line(&stream_cell_started_line(region_r4, unix_milliseconds()));
}

/// `done <cell> <statistics>`: every tile of the cell is written and closed.
pub fn report_cell_done(region_r4: u64, statistics: &str) {
    write_protocol_line(&format!("done {region_r4:x} {statistics}"));
}

/// `fail <cell> <message>`: this cell produced no complete output. The stream
/// continues with the next cell; the process exit status carries the count.
pub fn report_cell_failed(cell: &str, message: &str) {
    write_protocol_line(&format!("fail {cell} {}", single_line(message)));
}

fn unix_milliseconds() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn write_protocol_line(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
}

/// An error chain rendered as one line: the protocol is parsed by prefix, so a
/// newline inside a message would read as a line the orchestrator cannot place.
fn single_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOBRIS: u64 = 0x841e309ffffffff;

    fn cell(line: &str) -> StreamedCell {
        match parse_cell_line(line) {
            Some(CellRequest::Cell(cell)) => cell,
            other => panic!("{line:?} did not parse as a cell: {other:?}"),
        }
    }

    fn rejection(line: &str) -> String {
        match parse_cell_line(line) {
            Some(CellRequest::Rejected { message, .. }) => message,
            other => panic!("{line:?} was not rejected: {other:?}"),
        }
    }

    #[test]
    fn a_bare_cell_paints_every_layer_in_output_order() {
        let cell = cell("841e309ffffffff");
        assert_eq!(cell.region_r4, DOBRIS);
        assert_eq!(cell.layers, vec![0, 1, 2, 3, 4]);
        assert_eq!(cell.label(), "841e309ffffffff");
    }

    #[test]
    fn a_layers_request_paints_that_subset_in_output_order() {
        assert_eq!(cell("841e309ffffffff layers=rail,road").layers, vec![0, 1]);
        assert_eq!(
            cell("841e309ffffffff layers=aircraft-ground").layers,
            vec![4]
        );
        assert_eq!(cell("841e309ffffffff layers=road,road").layers, vec![0]);
    }

    #[test]
    fn blank_and_comment_lines_carry_no_work() {
        assert!(parse_cell_line("").is_none());
        assert!(parse_cell_line("   ").is_none());
        assert!(parse_cell_line("# a cell list header").is_none());
    }

    /// One bug class: a line the painter cannot turn into work must fail loudly
    /// with the cell it names, never be skipped into a silent missing tile.
    #[test]
    fn unpaintable_lines_are_rejected_with_their_cell() {
        assert!(rejection("zzz").contains("hexadecimal"));
        assert!(rejection("841e309ffffffff layers=noise").contains("does not paint"));
        let finer = CellIndex::try_from(DOBRIS)
            .unwrap()
            .center_child(Resolution::Five)
            .unwrap();
        assert!(rejection(&format!("{finer:x}")).contains("resolution"));
        assert!(rejection("0").contains("invalid H3 cell"));
        match parse_cell_line("zzz") {
            Some(CellRequest::Rejected { cell, .. }) => assert_eq!(cell, "zzz"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn cells_are_read_one_line_at_a_time_in_arrival_order() {
        let stream = "841e309ffffffff\n\n# comment\n843e191ffffffff layers=road,rail\nzzz\n";
        let requests: Vec<CellRequest> = cell_requests(stream.as_bytes()).collect();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0],
            CellRequest::Cell(StreamedCell {
                region_r4: DOBRIS,
                layers: vec![0, 1, 2, 3, 4],
            })
        );
        assert_eq!(
            requests[1],
            CellRequest::Cell(StreamedCell {
                region_r4: 0x843e191ffffffff,
                layers: vec![0, 1],
            })
        );
        assert!(matches!(requests[2], CellRequest::Rejected { .. }));
    }

    #[test]
    fn a_failure_message_is_collapsed_onto_one_protocol_line() {
        assert_eq!(
            single_line("load\n  the arrows: no such file"),
            "load the arrows: no such file"
        );
    }
}
