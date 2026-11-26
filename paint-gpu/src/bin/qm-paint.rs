//! Paint squares, merge their compressed bundles and publish immutable heatmap archives.
//! `qm-paint --prepared DIR --year YYYY --zoom 12|13 --out DIR (--squares X,Y[;X,Y...] | --bbox
//! SOUTH,WEST,NORTH,EAST)`: paints the z12 squares into heatmap tiles at `--zoom`, skipping squares
//! already painted, the next one prepared on the cores while up to three paint their pixels (the exact
//! evaluations on the first CUDA card when there is one) and two writers publish those before; one
//! line per square on stdout.
//! `qm-paint pack --out DIR --zoom Z --tiles DIR --build bNNN [--regions DIR]`: the map's archives
//! and manifest, reusing the zoom-8 regions' parts kept in `--regions`.
//! `qm-paint pack-region --out DIR --zoom Z --regions DIR --region X,Y`: a zoom-8 region's part of
//! the pack (its tiles from zoom 12 to 8 and its means), kept for the pack once its squares are painted.
//! `qm-paint merge --from DIR --out DIR --zoom Z`: atomic byte copies of completed squares.

use paint::exact::Batch;
use paint::hm3::painted;
use paint::levels::PaintedCells;
use paint::paint::{Corners, Grid, corners_of, paint_pixels};
use paint::square::Files;
#[path = "painter/card.rs"]
mod card;
#[path = "painter/prepared.rs"]
mod prepared;
use card::{Card, Reservation, SQUARES_ON_THE_CARD, paints_alone};
use paint_gpu::batch::GpuBatch;
use paint_gpu::device::{DeviceSquare, Gpu};
use paint_gpu::square_writer::{PaintedSquareCells, write_beside_painting};
use physics::weather::WeatherTable;
use popup::release::Release;
use prepared::PreparedSquare;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tiles::geo::{Mercator, TileId};

// glibc's per-thread arenas kept each square's freed buffers: over 32 Czech squares in one process
// the memory between squares grew from 4.5 to 11.3 GiB (14.1 GiB peak); mimalloc returns them
// (2 to 3.6 GiB between squares, 10.5 GiB peak) in the same time.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod common;
use common::option;

fn numbers(text: &str) -> Result<Vec<f64>, String> {
    text.split(',')
        .map(|part| {
            part.trim()
                .parse::<f64>()
                .map_err(|_| format!("not a number: {part}"))
        })
        .collect()
}

fn squares(arguments: &[String]) -> Result<Vec<TileId>, String> {
    if let Some(list) = option(arguments, "squares") {
        return list
            .split(';')
            .map(|pair| {
                let xy = numbers(pair)?;
                match xy[..] {
                    [x, y] => Ok(TileId {
                        x: x as u32,
                        y: y as u32,
                    }),
                    _ => Err(format!("a square is X,Y: {pair}")),
                }
            })
            .collect();
    }
    let bbox = numbers(option(arguments, "bbox").ok_or("missing --squares or --bbox")?)?;
    let [south, west, north, east] = bbox[..] else {
        return Err("--bbox is SOUTH,WEST,NORTH,EAST".into());
    };
    let (a, b) = (
        TileId::containing(Mercator::from_degrees(north, west)),
        TileId::containing(Mercator::from_degrees(south, east)),
    );
    Ok((a.y..=b.y)
        .flat_map(|y| (a.x..=b.x).map(move |x| TileId { x, y }))
        .collect())
}

fn run(arguments: &[String]) -> Result<(), String> {
    let required = |key: &str| option(arguments, key).ok_or_else(|| format!("missing --{key}"));
    if arguments.first().map(String::as_str) == Some("merge") {
        let zoom: u32 = required("zoom")?
            .parse()
            .map_err(|_| "--zoom is a number")?;
        return paint::merge::merge(
            &PathBuf::from(required("from")?),
            &PathBuf::from(required("out")?),
            zoom,
        );
    }
    if arguments.first().map(String::as_str) == Some("pack") {
        let zoom: u8 = required("zoom")?
            .parse()
            .map_err(|_| "--zoom is a number")?;
        return paint::pack::pack(
            &PathBuf::from(required("out")?),
            zoom,
            &PathBuf::from(required("tiles")?),
            required("build")?,
            option(arguments, "regions").map(PathBuf::from).as_deref(),
        );
    }
    if arguments.first().map(String::as_str) == Some("pack-region") {
        let zoom: u8 = required("zoom")?
            .parse()
            .map_err(|_| "--zoom is a number")?;
        let region = numbers(required("region")?)?;
        let [x, y] = region[..] else {
            return Err("--region is X,Y at zoom 8".into());
        };
        let squares = paint::pack::pack_region(
            &PathBuf::from(required("out")?),
            zoom,
            &PathBuf::from(required("regions")?),
            (x as u32, y as u32),
        )?;
        println!("qm-paint: region {x}/{y} kept, {squares} squares");
        return Ok(());
    }
    let year = required("year")?;
    let release = Release::open(&PathBuf::from(required("prepared")?), year)?;
    let out = PathBuf::from(required("out")?);
    let zoom: u32 = required("zoom")?
        .parse()
        .map_err(|_| "--zoom is 12 or 13")?;
    let grid = Grid::at_zoom(zoom).ok_or("--zoom is 12 or 13")?;
    let squares = squares(arguments)?;
    paint::year::bind(&out, year)?;
    let weather = WeatherTable::read(&release.weather_path)?;
    // The card, if this host has one (CUDA_VISIBLE_DEVICES chooses it); else the cores, said
    // aloud (the CUDA loader panics where the driver's libraries are missing).
    let gpu = match std::panic::catch_unwind(|| Gpu::new(0)) {
        Ok(Ok(gpu)) => Some(gpu),
        Ok(Err(error)) => {
            eprintln!("qm-paint: no card ({error}); the cores evaluate");
            None
        }
        Err(_) => {
            eprintln!("qm-paint: no CUDA libraries; the cores evaluate");
            None
        }
    };
    if let Some(gpu) = &gpu {
        eprintln!("qm-paint: exact evaluations on {}", gpu.name);
    }
    let mut todo = Vec::new();
    for (number, tile) in squares.iter().copied().enumerate() {
        if !painted(&out, zoom, (tile.x, tile.y))? {
            todo.push((number, tile));
        }
    }
    // A square is prepared on every core, or on a few beside another's pixels: those on the card
    // wait on the rest.
    let preparing_pool = (rayon::ThreadPoolBuilder::new().num_threads(PREPARING_THREADS))
        .build()
        .map_err(|e| e.to_string())?;
    let read = |&(number, tile): &(usize, TileId)| read_square(&release, &weather, number, tile);
    let weather = &weather;
    let preparing_pool = &preparing_pool;
    // Three stages: the preparer reads the next square on the host, reserves room on the card
    // ([`Card::admit`]) before uploading its device scene and taking its corners, the painters
    // paint up to [`SQUARES_ON_THE_CARD`] squares' pixels at once (the card), and two writers
    // encode and publish the squares before. Preparing two ahead slowed Dobris's busiest squares
    // by 3 % (2026-10-14). A heavy square is admitted alone before its upload and waits for an
    // empty card before the next square is read, so two heavy scenes never share card or host
    // memory.
    write_beside_painting(&out, zoom, |sender| {
        let card = Card::default();
        let (gpu, todo, squares, card) = (gpu.as_ref(), &todo, &squares, &card);
        // A square goes to its painter with its reservation on the card.
        let (prepared_sender, prepared_receiver) =
            sync_channel::<(Reservation<'_>, Result<Prepared<'_>, String>)>(0);
        // The painters own the receiver: when the last one ends, the preparer's next hand-over fails.
        let prepared_receiver = Arc::new(Mutex::new(prepared_receiver));
        let read = &read;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                for square in todo {
                    let ok = prepare_for_card(
                        card,
                        || {
                            // When the card is idle (first square, or after a heavy square
                            // finished), every core reads the square; otherwise reading stays in
                            // `preparing_pool`.
                            match card.active_squares() == 0 {
                                true => read(square),
                                false => preparing_pool.install(|| read(square)),
                            }
                        },
                        |read_square| read_square.candidates,
                        |read_square| {
                            // With only this square reserved on the card, no painter is running yet.
                            match card.active_squares() == 1 {
                                true => upload_and_corners(weather, gpu, grid, read_square),
                                false => preparing_pool.install(|| {
                                    upload_and_corners(weather, gpu, grid, read_square)
                                }),
                            }
                        },
                        |reservation, prepared| {
                            prepared_sender.send((reservation, prepared)).is_ok()
                        },
                    );
                    if !ok {
                        return;
                    }
                }
            });
            let painters: Vec<_> = (0..SQUARES_ON_THE_CARD)
                .map(|_| {
                    let (sender, prepared_receiver) = (sender.clone(), prepared_receiver.clone());
                    scope.spawn(move || -> Result<(), String> {
                        loop {
                            let received = (prepared_receiver.lock())
                                .map_err(|_| "a painter panicked".to_string())?
                                .recv();
                            let Ok((reservation, prepared)) = received else {
                                return Ok(());
                            };
                            if card.stopped() {
                                return Ok(());
                            }
                            // A panic is this square's error: the preparer may wait for the card.
                            let cells = prepared.and_then(|prepared| {
                                catch_unwind(AssertUnwindSafe(|| {
                                    finish_square(gpu, grid, squares.len(), prepared)
                                }))
                                .unwrap_or_else(|_| Err("a square's painting panicked".into()))
                            });
                            // A failed square stops the run as it leaves the card: the preparer
                            // waiting for an empty card must not hand the next square over.
                            reservation.finish(cells.is_err());
                            let sent = cells.and_then(|cells| {
                                (sender.send(cells))
                                    .map_err(|_| "a square's writer stopped".to_string())
                            });
                            if sent.is_err() {
                                card.change(|state| state.stopped = true);
                                return sent;
                            }
                        }
                    })
                })
                .collect();
            drop(prepared_receiver);
            painters.into_iter().try_for_each(|painter| {
                painter
                    .join()
                    .unwrap_or_else(|_| Err("a painter panicked".into()))
            })
        })
    })
}

/// Threads that prepare the next square while the pixels are painted.
const PREPARING_THREADS: usize = 12;

/// A square read and parsed on the host, before its device upload and corner evaluation.
struct ReadSquare<'a> {
    number: usize,
    tile: TileId,
    square: PreparedSquare<'a>,
    candidates: usize,
    read_s: f64,
    built_s: f64,
}

/// A square read, on the card and its corners taken, ready for its pixels.
struct Prepared<'a> {
    number: usize,
    tile: TileId,
    square: PreparedSquare<'a>,
    device_square: Option<DeviceSquare>,
    corners: Option<Corners>,
    candidates: usize,
    seconds: [f64; 3],
}

/// Reads and parses square `tile` on the host without touching the card.
fn read_square<'a>(
    release: &Release,
    weather: &'a WeatherTable,
    number: usize,
    tile: TileId,
) -> Result<ReadSquare<'a>, String> {
    let started = Instant::now();
    let files = Files::read(release, tile)?;
    let read_s = started.elapsed().as_secs_f64();
    let owned = PreparedSquare::from_files(files, weather, tile)?;
    let built_s = started.elapsed().as_secs_f64() - read_s;
    let candidates = owned.borrow_dependent().candidates.len();
    Ok(ReadSquare {
        number,
        tile,
        square: owned,
        candidates,
        read_s,
        built_s,
    })
}

/// Uploads a host-read square to the card (if there is one) and evaluates its corners.
fn upload_and_corners<'a>(
    weather: &WeatherTable,
    gpu: Option<&Gpu>,
    grid: Grid,
    read: ReadSquare<'a>,
) -> Result<Prepared<'a>, String> {
    let started = Instant::now();
    let ReadSquare {
        number,
        tile,
        square: owned,
        candidates,
        read_s,
        built_s,
    } = read;
    let (device_square, corners) = {
        let square = owned.borrow_dependent();
        let device_square = match gpu {
            Some(gpu) => Some(DeviceSquare::upload(gpu, square, owned.files(), weather)?),
            None => None,
        };
        let batch = gpu
            .zip(device_square.as_ref())
            .map(|(gpu, device_square)| GpuBatch::new(gpu, device_square, square));
        let corners = corners_of(square, grid, batch.as_ref().map(|b| b as &dyn Batch))?;
        (device_square, corners)
    };
    let corners_s = started.elapsed().as_secs_f64();
    Ok(Prepared {
        number,
        tile,
        square: owned,
        device_square,
        corners,
        candidates,
        seconds: [read_s, built_s, corners_s],
    })
}

/// Paints a prepared square's pixels and releases its source buffers before encoding the bundle.
fn finish_square(
    gpu: Option<&Gpu>,
    grid: Grid,
    count: usize,
    prepared: Prepared,
) -> Result<PaintedSquareCells, String> {
    let started = Instant::now();
    let Prepared {
        number,
        tile,
        square: owned,
        device_square,
        corners,
        candidates,
        seconds: [read_s, built_s, corners_s],
    } = prepared;
    let cells = match corners {
        None => PaintedCells::silence(grid.pixels * grid.pixels),
        Some(corners) => {
            let square = owned.borrow_dependent();
            let batch = gpu
                .zip(device_square.as_ref())
                .map(|(gpu, device_square)| GpuBatch::new(gpu, device_square, square));
            paint_pixels(
                square,
                grid,
                batch.as_ref().map(|b| b as &dyn Batch),
                corners,
            )
            .map_err(|error| format!("square {}/{}: {error}", tile.x, tile.y))?
        }
    };
    let painted_s = corners_s + started.elapsed().as_secs_f64();
    Ok(PaintedSquareCells {
        cells,
        tile,
        number,
        count,
        file_bytes: owned.files().bytes(),
        candidates,
        seconds: [read_s, built_s, painted_s],
    })
}

/// One preparer step: reads on the host, reserves room on `card` before uploading to the device,
/// hands the prepared square to a painter, and after a heavy square waits for the card to empty.
fn prepare_for_card<'a, R, P>(
    card: &'a Card,
    read: impl FnOnce() -> Result<R, String>,
    candidates_of: impl FnOnce(&R) -> usize,
    upload: impl FnOnce(R) -> Result<P, String>,
    hand_over: impl FnOnce(Reservation<'a>, Result<P, String>) -> bool,
) -> bool {
    if card.stopped() {
        return false;
    }
    let read_result = read();
    let candidates = read_result.as_ref().map_or(0, candidates_of);
    let heavy = paints_alone(candidates);
    // Reserve the card before allocating device memory or running corner batches.
    let Some(reservation) = card.admit(candidates) else {
        return false;
    };
    let prepared = read_result.and_then(upload);
    if !hand_over(reservation, prepared) || (heavy && !card.emptied()) {
        return false;
    }
    true
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-paint: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// A heavy square must reserve an empty card before its upload callback runs (in `6d9c8e5c`
    /// upload ran before `Card::admit`, overlapping two heavy scenes) and wait for `card.emptied()`
    /// after hand-over.
    #[test]
    fn heavy_square_waits_for_empty_card_before_upload() {
        let card = Card::default();
        let first = card.admit(100_000).unwrap();
        let (read_tx, read_rx) = sync_channel(1);
        let (handover_tx, handover_rx) = sync_channel(1);
        let upload_started = AtomicBool::new(false);
        let step_finished = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let ok = prepare_for_card(
                    &card,
                    || {
                        read_tx.send(()).unwrap();
                        Ok(card::PIPELINE_CANDIDATES_MAX + 1)
                    },
                    |&candidates| candidates,
                    |candidates| {
                        upload_started.store(true, Ordering::SeqCst);
                        // Only the heavy square itself may be reserved when uploading.
                        assert_eq!(card.active_squares(), 1);
                        Ok(candidates)
                    },
                    |reservation, prepared| {
                        assert!(prepared.is_ok());
                        handover_tx.send(reservation).is_ok()
                    },
                );
                assert!(ok);
                step_finished.store(true, Ordering::SeqCst);
            });
            read_rx.recv().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            assert!(
                !upload_started.load(Ordering::SeqCst),
                "heavy square uploaded while an earlier reservation was still active"
            );
            drop(first);
            let heavy_reservation = handover_rx.recv().unwrap();
            assert!(upload_started.load(Ordering::SeqCst));
            std::thread::sleep(Duration::from_millis(20));
            assert!(
                !step_finished.load(Ordering::SeqCst),
                "preparer returned after heavy hand-over before the card emptied"
            );
            drop(heavy_reservation);
        });
        assert!(step_finished.load(Ordering::SeqCst));
        assert_eq!(card.active_squares(), 0);
    }
}
