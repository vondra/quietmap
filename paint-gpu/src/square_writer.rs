//! Encode and publish the painted squares beside painting the next ones, two at a time.

use paint::hm3::write;
use paint::paint::Cells;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;
use tiles::geo::TileId;

/// Painted squares that may wait for a writer: the painter goes on while a square's encoding
/// outlasts the next one's pixels (a square's cells, 7 MB at zoom 13). Light squares 3-7 % faster
/// than with a rendezvous, the others the same (2026-10-14).
const SQUARES_WAITING: usize = 2;

/// Squares encoded at once. A square's encoding lasts as long as its slowest tile (one stream on one
/// core at Brotli quality 11), and a light square has few distinct tiles: one writer, busy all the
/// time on the light classes, held their painters back while most cores waited.
const WRITERS: usize = 2;

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

/// [`WRITERS`] writers (dev1's surface runner had one); their queue of [`SQUARES_WAITING`] bounds
/// residency to the squares being written, the waiting ones and the painters' next cells.
/// Completion is reported only after the atomic bundle is durable.
pub fn write_beside_painting(
    out: &Path,
    zoom: u32,
    paint: impl FnOnce(&SyncSender<PaintedSquareCells>) -> Result<(), String>,
) -> Result<(), String> {
    let (sender, receiver) = sync_channel::<PaintedSquareCells>(SQUARES_WAITING);
    let queue = &Mutex::new(Some(receiver));
    // The scope's closure owns the sender: a panic while painting drops it as it unwinds, so the
    // writers end and the scope's wait for them returns.
    std::thread::scope(move |scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|_| scope.spawn(move || write_queued(queue, out, zoom)))
            .collect();
        let painted = paint(&sender);
        drop(sender);
        // Every writer joined, the first error kept.
        let mut written = Ok(());
        for writer in writers {
            let joined = writer.join();
            written =
                written.and(joined.unwrap_or_else(|_| Err("a square's writer panicked".into())));
        }
        written?;
        painted
    })
}

/// One writer: the queue's squares until it closes. Its first error (a panic too) closes the
/// queue once no other writer waits on it, so the painters' hand-overs fail and the run stops.
fn write_queued(
    queue: &Mutex<Option<Receiver<PaintedSquareCells>>>,
    out: &Path,
    zoom: u32,
) -> Result<(), String> {
    let lock = || queue.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        // One writer waits on the queue at a time; the others write.
        let received = lock().as_ref().map(Receiver::recv);
        let Some(Ok(square)) = received else {
            return Ok(());
        };
        let written = catch_unwind(AssertUnwindSafe(|| square.write(out, zoom)))
            .unwrap_or_else(|_| Err("a square's writer panicked".into()));
        if written.is_err() {
            lock().take();
            return written;
        }
    }
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
        // A writer's error refuses the squares a painter goes on handing over: bounded by time, so
        // any schedule of the writers lets the failing one close the queue first.
        let (mut refused, started) = (false, std::time::Instant::now());
        let result = write_beside_painting(directory.path(), 12, |sender| {
            let mut x = 2;
            while x < 4000 && started.elapsed() < std::time::Duration::from_secs(20) {
                let cells = if x == 2 { Vec::new() } else { cells.clone() };
                if sender.send(square(x, cells)).is_err() {
                    refused = true;
                    return Err("a square's writer stopped".into());
                }
                x += 1;
            }
            Ok(())
        });
        assert!(result.unwrap_err().contains("not a zoom-12 square's cells"));
        assert!(refused);
        // A painter's panic comes out of the writing, not a wait for the writers.
        let unwound = std::panic::catch_unwind(AssertUnwindSafe(|| {
            write_beside_painting(directory.path(), 12, |sender| -> Result<(), String> {
                sender.send(square(4000, cells.clone())).unwrap();
                panic!("a painter panicked")
            })
        }));
        assert!(unwound.is_err());
    }
}
