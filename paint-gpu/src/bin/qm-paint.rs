//! Paint squares, merge their compressed bundles and publish immutable heatmap archives.
//! `qm-paint --prepared DIR --year YYYY --zoom 12|13 --out DIR (--squares X,Y[;X,Y...] | --bbox
//! SOUTH,WEST,NORTH,EAST)`: paints the z12 squares one after another (each on every core, its exact
//! evaluations on the first CUDA card when there is one) into heatmap tiles at `--zoom`, skipping
//! squares already painted; one line per square on stdout.
//! `qm-paint pack --out DIR --zoom Z --tiles DIR --build bNNN`: the map's archives and manifest.
//! `qm-paint merge --from DIR --out DIR --zoom Z`: atomic byte copies of completed squares.

use paint::exact::Batch;
use paint::exact::LAYERS;
use paint::hm3::painted;
use paint::paint::{Corners, Grid, SILENCE, corners_of, paint_pixels};
use paint::square::{Files, Square};
use paint_gpu::batch::GpuBatch;
use paint_gpu::device::{DeviceSquare, Gpu};
use paint_gpu::square_writer::{PaintedSquareCells, write_beside_painting};
use physics::weather::WeatherTable;
use popup::release::Release;
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;
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
        );
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
    // Three stages, a thread each: the preparer reads the next square, uploads it and takes its
    // corners (on its own cores) while this thread paints a square's pixels (the card) and the
    // writer encodes and publishes the squares before. The preparer hands its square over when
    // the card is free; preparing two ahead slowed Dobris's busiest squares by 3 % (2026-10-14).
    // A heavy square paints alone: two of those do not fit in memory.
    write_beside_painting(&out, zoom, |sender| {
        std::thread::scope(|scope| {
            let (prepared_sender, prepared_receiver) = sync_channel(0);
            let (heavy_done, heavy_painted) = sync_channel::<()>(0);
            scope.spawn(move || {
                // The first square and the one after a heavy square wait for no card: every core.
                let mut card_idle = true;
                for square in &todo {
                    let prepared = match card_idle {
                        true => prepare(square),
                        false => preparing_pool.install(|| prepare(square)),
                    };
                    let heavy = prepared
                        .as_ref()
                        .is_ok_and(|prepared| prepared.candidates > PIPELINE_CANDIDATES_MAX);
                    if prepared_sender.send(prepared).is_err()
                        || (heavy && heavy_painted.recv().is_err())
                    {
                        return;
                    }
                    card_idle = heavy;
                }
            });
            for prepared in prepared_receiver {
                let prepared: Prepared = prepared?;
                let heavy = prepared.candidates > PIPELINE_CANDIDATES_MAX;
                let cells = finish_square(&weather, gpu.as_ref(), grid, squares.len(), prepared)?;
                if heavy {
                    // The preparer waits on it; it may have stopped on an error of its own.
                    let _ = heavy_done.send(());
                }
                sender
                    .send(cells)
                    .map_err(|_| "a square's writer stopped".to_string())?;
            }
            Ok(())
        })
    })
}

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
        None => vec![[SILENCE; LAYERS + 1]; grid.pixels * grid.pixels],
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
            )?
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
    // A square's large buffers, freed, serve the next square from the heap instead of going back to
    // the system and being faulted in again: the Sahara's quiet squares in about half the time
    // (2026-10-11).
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 30);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 1 << 30);
        libc::mallopt(libc::M_TOP_PAD, 1 << 30);
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-paint: {error}");
        std::process::exit(1);
    }
}
