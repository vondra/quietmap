//! `qm-paint --prepared DIR --year YYYY --zoom 12|13 --out DIR (--squares X,Y[;X,Y...] | --bbox
//! SOUTH,WEST,NORTH,EAST)`: paints the z12 squares one after another (each on every core, its exact
//! evaluations on the first CUDA card when there is one) into heatmap tiles at `--zoom`, skipping
//! squares already painted; one line per square on stdout.
//! `qm-paint pack --out DIR --zoom Z --tiles DIR --build bNNN`: the map's archives and manifest.

use paint::exact::Batch;
use paint::exact::LAYERS;
use paint::hm3::{painted, write};
use paint::paint::{Corners, Grid, SILENCE, corners_of, paint_pixels};
use paint::square::{Files, Square};
use paint_gpu::batch::GpuBatch;
use paint_gpu::device::{DeviceSquare, Gpu};
use physics::weather::WeatherTable;
use popup::release::Release;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use tiles::geo::{Mercator, TileId};

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
    let todo: Vec<(usize, TileId)> = (squares.iter().copied().enumerate())
        .filter(|(_, tile)| !painted(&out, zoom, (tile.x, tile.y)))
        .collect();
    // A square is prepared on every core, or on a few beside another's pixels: those on the card
    // wait on the rest.
    let preparing_pool = (rayon::ThreadPoolBuilder::new().num_threads(PREPARING_THREADS))
        .build()
        .map_err(|e| e.to_string())?;
    let prepare = |&(number, tile): &(usize, TileId)| {
        prepare_square(&release, &weather, gpu.as_ref(), grid, number, tile)
    };
    let prepare_beside =
        |following: &(usize, TileId)| preparing_pool.install(|| prepare(following));
    let finish = |prepared: Prepared| {
        finish_square(
            &weather,
            gpu.as_ref(),
            grid,
            (&out, zoom),
            squares.len(),
            prepared,
        )
    };
    // While the card paints one square's pixels, the cores read the next and take its corners,
    // unless the square on the card is heavy: two of those do not fit in memory.
    let mut next = todo.first().map(prepare).transpose()?;
    for k in 0..todo.len() {
        let current = next.take().ok_or("a square not prepared")?;
        match todo.get(k + 1) {
            Some(following) if current.candidates <= PIPELINE_CANDIDATES_MAX => {
                let (finished, prepared) = std::thread::scope(|scope| {
                    let preparing = scope.spawn(|| prepare_beside(following));
                    let finished = finish(current);
                    let prepared = (preparing.join())
                        .unwrap_or_else(|_| Err("a square's preparation panicked".into()));
                    (finished, prepared)
                });
                finished?;
                next = Some(prepared?);
            }
            following => {
                finish(current)?;
                next = following.map(prepare).transpose()?;
            }
        }
    }
    Ok(())
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
            .map(|(gpu, device_square)| GpuBatch {
                gpu,
                device_square,
                square: &square,
            });
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

/// Paints a prepared square's pixels and writes its tiles; one line on stdout.
fn finish_square(
    weather: &WeatherTable,
    gpu: Option<&Gpu>,
    grid: Grid,
    (out, zoom): (&PathBuf, u32),
    count: usize,
    prepared: Prepared,
) -> Result<(), String> {
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
                .map(|(gpu, device_square)| GpuBatch {
                    gpu,
                    device_square,
                    square: &square,
                });
            paint_pixels(
                &square,
                grid,
                batch.as_ref().map(|b| b as &dyn Batch),
                corners,
            )?
        }
    };
    write(out, zoom, (tile.x, tile.y), &cells)?;
    let painted_s = corners_s + started.elapsed().as_secs_f64();
    writeln!(
        std::io::stdout().lock(),
        "qm-paint: square {}/{} ({} of {count}): {:.0} MB, {candidates} sources, read {read_s:.1} s, built {built_s:.1} s, painted {painted_s:.1} s",
        tile.x,
        tile.y,
        number + 1,
        files.bytes() as f64 / 1e6,
    )
    .map_err(|e| e.to_string())
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
