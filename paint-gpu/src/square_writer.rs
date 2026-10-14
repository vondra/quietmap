//! Encode and publish one square beside painting the next, with no queued cell buffers.

use paint::hm3::write;
use paint::paint::Cells;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Instant;
use tiles::geo::TileId;

/// Painted squares that may wait for the writer: the painter goes on while a square's encoding
/// outlasts the next one's pixels (a square's cells, 7 MB at zoom 13). Light squares 3-7 % faster
/// than with a rendezvous, the others the same (msas2, 2026-10-14).
const SQUARES_WAITING: usize = 2;

pub struct PaintedSquareCells {
    pub cells: Cells,
    pub tile: TileId,
    pub number: usize,
    pub count: usize,
    pub file_bytes: u64,
    pub candidates: usize,
    pub seconds: [f64; 3],
}

impl PaintedSquareCells {
    fn write(self, out: &Path, zoom: u32) -> Result<(), String> {
        let started = Instant::now();
        write(out, zoom, (self.tile.x, self.tile.y), &self.cells)?;
        let written_s = started.elapsed().as_secs_f64();
        let [read_s, built_s, painted_s] = self.seconds;
        writeln!(
            std::io::stdout().lock(),
            "qm-paint: square {}/{} ({} of {}): {:.0} MB, {} sources, read {read_s:.1} s, built {built_s:.1} s, painted {painted_s:.1} s, written {written_s:.1} s",
            self.tile.x,
            self.tile.y,
            self.number + 1,
            self.count,
            self.file_bytes as f64 / 1e6,
            self.candidates,
        )
        .map_err(|error| error.to_string())
    }
}

/// One writer, as in dev1's surface runner; its queue of [`SQUARES_WAITING`] bounds residency to
/// its square, the waiting ones and the painter's next cells. Completion is reported only after
/// the atomic bundle is durable.
pub fn write_beside_painting(
    out: &Path,
    zoom: u32,
    paint: impl FnOnce(&SyncSender<PaintedSquareCells>) -> Result<(), String>,
) -> Result<(), String> {
    std::thread::scope(|scope| {
        let (sender, receiver) = sync_channel::<PaintedSquareCells>(SQUARES_WAITING);
        let writing = scope.spawn(move || -> Result<(), String> {
            for square in receiver {
                square.write(out, zoom)?;
            }
            Ok(())
        });
        let painted = paint(&sender);
        drop(sender);
        writing
            .join()
            .unwrap_or_else(|_| Err("a square's writer panicked".into()))?;
        painted
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use paint::exact::LAYERS;
    use paint::hm3::{TILE_PX, painted, read};

    #[test]
    fn writer_errors_fail_the_run_and_only_complete_bundles_can_resume() {
        let directory = tempfile::tempdir().unwrap();
        let cells = vec![[254; LAYERS + 1]; TILE_PX * TILE_PX];
        let square = |x, cells| PaintedSquareCells {
            cells,
            tile: TileId { x, y: 0 },
            number: x as usize,
            count: 2,
            file_bytes: 0,
            candidates: 0,
            seconds: [0.0; 3],
        };
        let result = write_beside_painting(directory.path(), 12, |sender| {
            sender.send(square(0, cells.clone())).unwrap();
            sender.send(square(1, Vec::new())).unwrap();
            Ok(())
        });
        assert!(result.unwrap_err().contains("not a zoom-12 square's cells"));
        assert!(painted(directory.path(), 12, (0, 0)).unwrap());
        assert!(!painted(directory.path(), 12, (1, 0)).unwrap());
        assert_eq!(read(directory.path(), 12, (0, 0)).unwrap(), cells);
        let result = write_beside_painting(directory.path(), 12, |sender| {
            sender.send(square(1, cells.clone())).unwrap();
            Err("painting stopped".into())
        });
        assert_eq!(result.unwrap_err(), "painting stopped");
        assert_eq!(read(directory.path(), 12, (1, 0)).unwrap(), cells);
    }
}
