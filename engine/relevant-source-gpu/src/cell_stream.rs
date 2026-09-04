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

use anyhow::{bail, Context, Result};
use std::io::{BufRead, Write};
use std::path::Path;

use tile_painter::region_runner::stream_cell_started_line;
use tile_painter::stream_tile_window::{parse_r4_cell_hex, TileWindow};

use crate::surface_layers::{LAYER_COUNT, LAYER_NAMES};

/// One cell to paint: the R4 cell and the layers it wants, as indices into
/// [`LAYER_NAMES`] in output order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamedCell {
    pub region_r4: u64,
    pub layers: Vec<usize>,
    pub tile_window: Option<TileWindow>,
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
    Rejected {
        cell: String,
        message: String,
    },
    /// The stream itself broke — a read error or a byte that is not UTF-8.
    /// No cell can be named, so this is never a `fail` line; it is a plain
    /// diagnostic and a non-zero exit status, because ending the stream
    /// quietly would report success for every cell still queued behind it.
    Unreadable {
        message: String,
    },
}

fn rejected(cell: &str, message: impl Into<String>) -> CellRequest {
    CellRequest::Rejected {
        cell: cell.to_owned(),
        message: message.into(),
    }
}

/// Parse one stdin line: `<r4hex>`, followed optionally by `layers=<csv>` and
/// `tiles=x,y,side`. Blank lines and `#` comments carry no work at all, so
/// they are neither painted nor failed. A tile window belongs to its one cell;
/// this lets a release gate paint a bounded multi-cell window without starting
/// a second process and paying CUDA's cold initialisation twice.
///
/// An unknown layer name is refused rather than ignored: silently painting the
/// names it did recognise would report `done` for a cell whose missing layer
/// the orchestrator then collects as an absent tile.
pub fn parse_cell_line(line: &str) -> Option<CellRequest> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut tokens = line.split_whitespace();
    let hex = tokens.next().expect("a non-empty line has a first token");
    let mut requested = None;
    let mut tile_window = None;
    for token in tokens {
        if let Some(value) = token.strip_prefix("layers=") {
            if requested
                .replace(value.split(',').collect::<Vec<_>>())
                .is_some()
            {
                return Some(rejected(hex, "layers= may be given only once"));
            }
        } else if let Some(value) = token.strip_prefix("tiles=") {
            if tile_window.is_some() {
                return Some(rejected(hex, "tiles= may be given only once"));
            }
            match value.parse() {
                Ok(window) => tile_window = Some(window),
                Err(error) => return Some(rejected(hex, format!("invalid tiles=: {error}"))),
            }
        } else {
            return Some(rejected(
                hex,
                "only layers=<csv> and tiles=x,y,side are understood; nothing else",
            ));
        }
    }
    let region_r4 = match parse_r4_cell_hex(hex) {
        Ok(region_r4) => region_r4,
        Err(error) => return Some(rejected(hex, format!("{error:#}"))),
    };
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
    Some(CellRequest::Cell(StreamedCell {
        region_r4,
        layers,
        tile_window,
    }))
}

/// The cells as they arrive. The reader is consumed line by line and never
/// drained ahead, so a world run that feeds cells over hours paints the first
/// one immediately. A read error yields one [`CellRequest::Unreadable`] and
/// then ends the stream: a broken pipe must not look like a clean EOF, or
/// every cell still queued behind it would be reported as painted.
pub fn cell_requests<R: BufRead>(reader: R) -> impl Iterator<Item = CellRequest> {
    let mut lines = reader.lines();
    let mut broken = false;
    std::iter::from_fn(move || loop {
        if broken {
            return None;
        }
        match lines.next()? {
            Ok(line) => {
                if let Some(request) = parse_cell_line(&line) {
                    return Some(request);
                }
            }
            Err(error) => {
                broken = true;
                return Some(CellRequest::Unreadable {
                    message: format!("stdin is unreadable: {error}"),
                });
            }
        }
    })
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

/// The one `<year>/h3r4` child of a prepared root. Two years side by side is a
/// tree this painter cannot choose from: the caller must name the year.
pub fn prepared_dataset_year(prepared_directory: &Path) -> Result<String> {
    let mut years: Vec<String> = std::fs::read_dir(prepared_directory)
        .with_context(|| format!("read prepared root {}", prepared_directory.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("h3r4").is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_digit()))
        .collect();
    years.sort_unstable();
    match years.as_slice() {
        [year] => Ok(year.clone()),
        [] => bail!(
            "prepared root {} holds no <year>/h3r4 tree",
            prepared_directory.display()
        ),
        many => bail!(
            "prepared root {} holds {} dataset years ({}); set DATA_YEAR",
            prepared_directory.display(),
            many.len(),
            many.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use h3o::{CellIndex, Resolution};

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
        assert_eq!(cell.tile_window, None);
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
        assert!(rejection("841e309ffffffff layer=road").contains("nothing else"));
        assert!(rejection("841e309ffffffff layers=road extra").contains("nothing else"));
        assert!(rejection("841e309ffffffff layers=road layers=rail").contains("only once"));
        assert!(rejection("841e309ffffffff tiles=4414,2786,0").contains("positive"));
        assert!(rejection("841e309ffffffff tiles=4414,2786,4 tiles=1,2,3").contains("only once"));
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

    /// One bug class: a broken pipe must not read as a clean end of work, or
    /// the cells behind it are reported painted when nothing painted them.
    #[test]
    fn an_unreadable_stream_is_reported_and_ends_the_stream() {
        let requests: Vec<CellRequest> =
            cell_requests(b"841e309ffffffff\n\xff\xfe\n".as_slice()).collect();
        assert_eq!(requests.len(), 2);
        assert!(matches!(requests[0], CellRequest::Cell(_)));
        match &requests[1] {
            CellRequest::Unreadable { message } => assert!(message.contains("unreadable")),
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
                tile_window: None,
            })
        );
        assert_eq!(
            requests[1],
            CellRequest::Cell(StreamedCell {
                region_r4: 0x843e191ffffffff,
                layers: vec![0, 1],
                tile_window: None,
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

    /// The window itself — its syntax and which tiles it selects — is
    /// `tile_painter::stream_tile_window`'s, shared with the aircraft painters so all seven
    /// layers can paint one area. This painter only has to attach it to its cell.
    #[test]
    fn a_tiles_token_attaches_the_shared_window_to_its_cell() {
        let window = TileWindow {
            x: 4414,
            y: 2786,
            side: 4,
        };
        assert_eq!(
            cell("841e309ffffffff layers=road,rail tiles=4414,2786,4").tile_window,
            Some(window)
        );
        assert_eq!(
            cell("841e301ffffffff tiles=4414,2786,4 layers=road").tile_window,
            Some(window)
        );
    }

    /// One bug class: the dataset year comes from the prepared tree itself,
    /// never from a literal a dataset transition would leave behind.
    #[test]
    fn the_prepared_tree_names_its_own_dataset_year() {
        let root_with_years = |years: &[&str]| {
            let root = tempfile::tempdir().unwrap();
            for year in years {
                std::fs::create_dir_all(root.path().join(year).join("h3r4")).unwrap();
            }
            root
        };
        let one = root_with_years(&["2026"]);
        assert_eq!(prepared_dataset_year(one.path()).unwrap(), "2026");
        let two = root_with_years(&["2026", "2027"]);
        let ambiguous = prepared_dataset_year(two.path()).unwrap_err().to_string();
        assert!(ambiguous.contains("2026, 2027") && ambiguous.contains("set DATA_YEAR"));
        let none = root_with_years(&[]);
        assert!(prepared_dataset_year(none.path()).is_err());
        // A stray four-digit directory without an h3r4 tree is not a dataset year.
        std::fs::create_dir_all(one.path().join("2027")).unwrap();
        assert_eq!(prepared_dataset_year(one.path()).unwrap(), "2026");
    }
}
