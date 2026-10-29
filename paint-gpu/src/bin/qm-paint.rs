//! Paint squares, merge their compressed bundles and publish immutable heatmap archives.
//! `qm-paint --prepared DIR --year YYYY --zoom 12|13 --out DIR (--squares X,Y[;X,Y...] | --bbox
//! SOUTH,WEST,NORTH,EAST)`: paints the z12 squares into heatmap tiles at `--zoom`, skipping squares
//! already painted, the next one prepared on the cores while two paint their pixels (the exact
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
use paint::square::{Files, Square};
use paint_gpu::batch::GpuBatch;
use paint_gpu::device::{DeviceSquare, Gpu};
use paint_gpu::square_writer::{PaintedSquareCells, write_beside_painting};
use physics::weather::WeatherTable;
use popup::release::Release;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;
use tiles::geo::{Mercator, TileId};

// glibc's per-thread arenas kept each square's freed buffers: over 32 Czech squares in one process
// the memory between squares grew from 4.5 to 11.3 GiB (14.1 GiB peak); mimalloc returns them
// (2 to 3.6 GiB between squares, 10.5 GiB peak) in the same time.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn value<'a>(arguments: &'a [String], key: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == format!("--{key}"))
        .map(|pair| pair[1].as_str())
}

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
    if let Some(list) = value(arguments, "squares") {
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
    let bbox = numbers(value(arguments, "bbox").ok_or("missing --squares or --bbox")?)?;
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
    let required = |key: &str| value(arguments, key).ok_or_else(|| format!("missing --{key}"));
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
            value(arguments, "regions").map(PathBuf::from).as_deref(),
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
    let release = Release::open(&PathBuf::from(required("prepared")?), required("year")?)?;
    let weather = WeatherTable::read(&release.weather_path)?;
    let out = PathBuf::from(required("out")?);
    let zoom: u32 = required("zoom")?
        .parse()
        .map_err(|_| "--zoom is 12 or 13")?;
    let grid = Grid::at_zoom(zoom).ok_or("--zoom is 12 or 13")?;
    let squares = squares(arguments)?;
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
    let prepare = |&(number, tile): &(usize, TileId)| {
        prepare_square(&release, &weather, gpu.as_ref(), grid, number, tile)
    };
    let preparing_pool = &preparing_pool;
    // Three stages: the preparer reads the next square, uploads it and takes its corners (on its
    // own cores), the painters paint [`SQUARES_ON_THE_CARD`] squares' pixels at once (the card),
    // two writers encode and publish the squares before. The preparer hands a square to the
    // first free painter; preparing two ahead slowed Dobris's busiest squares by 3 % (2026-10-14).
    // A heavy square paints alone: two of those do not fit in memory.
    write_beside_painting(&out, zoom, |sender| {
        let (prepared_sender, prepared_receiver) = sync_channel::<Result<Prepared, String>>(0);
        // The painters own the receiver: when the last one ends, the preparer's next hand-over fails.
        let prepared_receiver = Arc::new(Mutex::new(prepared_receiver));
        let card = Card::default();
        let (weather, gpu, todo, squares, card) = (&weather, gpu.as_ref(), &todo, &squares, &card);
        let prepare = &prepare;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                // The first square and the one after a heavy square wait for no card: every core.
                let mut card_idle = true;
                for square in todo {
                    if card.stopped() {
                        return;
                    }
                    let prepared = match card_idle {
                        true => prepare(square),
                        false => preparing_pool.install(|| prepare(square)),
                    };
                    let heavy = (prepared.as_ref())
                        .is_ok_and(|prepared| prepared.candidates > PIPELINE_CANDIDATES_MAX);
                    if heavy && !card.emptied() {
                        return;
                    }
                    card.change(|state| state.squares += 1);
                    if prepared_sender.send(prepared).is_err() || heavy && !card.emptied() {
                        return;
                    }
                    card_idle = heavy;
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
                            let Ok(prepared) = received else {
                                return Ok(());
                            };
                            if card.stopped() {
                                card.change(|state| state.squares -= 1);
                                return Ok(());
                            }
                            // A panic is this square's error: the preparer may wait for the card.
                            let cells = prepared.and_then(|prepared| {
                                catch_unwind(AssertUnwindSafe(|| {
                                    finish_square(weather, gpu, grid, squares.len(), prepared)
                                }))
                                .unwrap_or_else(|_| Err("a square's painting panicked".into()))
                            });
                            // A failed square stops the run as it leaves the card: the preparer
                            // waiting for an empty card must not hand the next square over.
                            card.change(|state| {
                                state.squares -= 1;
                                state.stopped |= cells.is_err();
                            });
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

/// The squares handed to the painters and not yet painted, and whether a painter stopped on an
/// error: the preparer waits for an empty card before and after a heavy square.
#[derive(Default)]
struct Card {
    state: Mutex<CardState>,
    changed: Condvar,
}

#[derive(Default)]
struct CardState {
    squares: usize,
    stopped: bool,
}

impl Card {
    fn change(&self, change: impl FnOnce(&mut CardState)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.changed.notify_all();
    }

    fn stopped(&self) -> bool {
        (self.state.lock().unwrap_or_else(PoisonError::into_inner)).stopped
    }

    /// Waits until no square is on the card: false when a painter stopped first.
    fn emptied(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let state = (self
            .changed
            .wait_while(state, |state| state.squares > 0 && !state.stopped))
        .unwrap_or_else(PoisonError::into_inner);
        !state.stopped
    }
}

/// Squares whose pixels are painted at once: one square's passes leave the card idle between them
/// (two painter processes on one card: light squares 1.4x, 30-300 KB 1.17x, Dobris 1.12x,
/// 2026-10-14); two processes also kept two copies of the kernels' card memory.
const SQUARES_ON_THE_CARD: usize = 2;

/// A square's sources at most this many candidates paint while the next one is prepared (Dobris's
/// about 130,000 do; Prague's centre, 470,000 and 30-45 GB, paints alone).
const PIPELINE_CANDIDATES_MAX: usize = 250_000;

/// Threads that prepare the next square while the pixels are painted.
const PREPARING_THREADS: usize = 12;

/// A square read, on the card and its corners taken, ready for its pixels.
struct Prepared {
    number: usize,
    tile: TileId,
    files: Files,
    device_square: Option<DeviceSquare>,
    corners: Option<Corners>,
    candidates: usize,
    seconds: [f64; 3],
}

/// Reads square `tile`, uploads it to the card (if there is one) and takes its corners.
fn prepare_square(
    release: &Release,
    weather: &WeatherTable,
    gpu: Option<&Gpu>,
    grid: Grid,
    number: usize,
    tile: TileId,
) -> Result<Prepared, String> {
    let started = Instant::now();
    let files = Files::read(release, tile)?;
    let read_s = started.elapsed().as_secs_f64();
    let (device_square, corners, candidates, built_s) = {
        let square = Square::new(weather, tile, &files)?;
        let built_s = started.elapsed().as_secs_f64() - read_s;
        let device_square = match gpu {
            Some(gpu) => Some(DeviceSquare::upload(gpu, &square, &files, weather)?),
            None => None,
        };
        let batch = gpu
            .zip(device_square.as_ref())
            .map(|(gpu, device_square)| GpuBatch::new(gpu, device_square, &square));
        let corners = corners_of(&square, grid, batch.as_ref().map(|b| b as &dyn Batch))?;
        (device_square, corners, square.candidates.len(), built_s)
    };
    let corners_s = started.elapsed().as_secs_f64() - read_s - built_s;
    Ok(Prepared {
        number,
        tile,
        files,
        device_square,
        corners,
        candidates,
        seconds: [read_s, built_s, corners_s],
    })
}

/// Paints a prepared square's pixels and releases its source buffers before encoding the bundle.
fn finish_square(
    weather: &WeatherTable,
    gpu: Option<&Gpu>,
    grid: Grid,
    count: usize,
    prepared: Prepared,
) -> Result<PaintedSquareCells, String> {
    let started = Instant::now();
    let Prepared {
        number,
        tile,
        files,
        device_square,
        corners,
        candidates,
        seconds: [read_s, built_s, corners_s],
    } = prepared;
    let cells = match corners {
        None => PaintedCells::silence(grid.pixels * grid.pixels),
        Some(corners) => {
            let square = Square::new(weather, tile, &files)?;
            let batch = gpu
                .zip(device_square.as_ref())
                .map(|(gpu, device_square)| GpuBatch::new(gpu, device_square, &square));
            paint_pixels(
                &square,
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
        file_bytes: files.bytes(),
        candidates,
        seconds: [read_s, built_s, painted_s],
    })
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-paint: {error}");
        std::process::exit(1);
    }
}
