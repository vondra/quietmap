//! Bench only: replays a square's card launches dumped by the painter (`QM_DUMP_LAUNCHES`) and
//! times each pair kernel; `compare` holds two replays' results against each other.
//! `qm-ray-bench --prepared DIR --year YYYY --square X,Y --launches DIR [--repeat N] [--out DIR]`
//! `qm-ray-bench compare A_DIR B_DIR`

use paint::square::{Files, Square};
use paint_gpu::device::{DeviceSquare, Gpu, KERNEL_SECONDS, TIME_KERNELS};
use physics::weather::WeatherTable;
use popup::release::Release;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tiles::geo::TileId;

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn value<'a>(arguments: &'a [String], key: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == format!("--{key}"))
        .map(|pair| pair[1].as_str())
}

struct Launch {
    reached: usize,
    receivers: Vec<[f64; 2]>,
    own: Vec<u64>,
    candidates: Vec<u32>,
    at: Vec<u32>,
    weights: Vec<f32>,
}

fn read_launch(path: &Path) -> Result<Launch, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if &bytes[..8] != b"qmlaunch" {
        return Err(format!("{}: not a launch", path.display()));
    }
    let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let (count, reached) = (word(8) as usize, word(16) as usize);
    let mut at = 24;
    let mut take = |size: usize| {
        let slice = &bytes[at..at + size * count];
        at += size * count;
        slice
    };
    let xy = take(16);
    let receivers = (xy.chunks_exact(16))
        .map(|c| {
            [
                f64::from_le_bytes(c[..8].try_into().unwrap()),
                f64::from_le_bytes(c[8..].try_into().unwrap()),
            ]
        })
        .collect();
    let own = (take(8).chunks_exact(8))
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let candidates = (take(4).chunks_exact(4))
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let slots = (take(4).chunks_exact(4))
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let weights = (take(4).chunks_exact(4))
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok(Launch {
        reached,
        receivers,
        own,
        candidates,
        at: slots,
        weights,
    })
}

fn floats(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((bytes.chunks_exact(4))
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

/// Two replays' sums and bands: the largest level differences (dB) where both exceed a floor, and
/// how many values differ by more than 0.01 and 0.1 dB.
fn compare(a: &Path, b: &Path) -> Result<(), String> {
    let mut names: Vec<_> = std::fs::read_dir(a)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .collect();
    names.sort();
    for name in names {
        let (x, y) = (floats(&a.join(&name))?, floats(&b.join(&name))?);
        if x.len() != y.len() {
            return Err(format!("{name:?}: {} against {} values", x.len(), y.len()));
        }
        let (mut worst, mut over_001, mut over_01, mut one_zero) = (0.0f64, 0, 0, 0);
        for (&p, &q) in x.iter().zip(&y) {
            let (p, q) = (f64::from(p), f64::from(q));
            if p <= 1e-30 && q <= 1e-30 {
                continue;
            }
            if p <= 1e-30 || q <= 1e-30 {
                one_zero += 1;
                continue;
            }
            let db = (10.0 * (p / q).log10()).abs();
            worst = worst.max(db);
            over_001 += usize::from(db > 0.01);
            over_01 += usize::from(db > 0.1);
        }
        println!(
            "{name:?}: {} values, worst {worst:.4} dB, over 0.01 dB {over_001}, over 0.1 dB {over_01}, one side zero {one_zero}",
            x.len()
        );
    }
    Ok(())
}

fn run(arguments: &[String]) -> Result<(), String> {
    if arguments.first().map(String::as_str) == Some("compare") {
        return compare(Path::new(&arguments[1]), Path::new(&arguments[2]));
    }
    let required = |key: &str| value(arguments, key).ok_or_else(|| format!("missing --{key}"));
    let release = Release::open(&PathBuf::from(required("prepared")?), required("year")?)?;
    let weather = WeatherTable::read(&release.weather_path)?;
    let xy: Vec<u32> = (required("square")?.split(','))
        .map(|v| v.parse().map_err(|_| "--square X,Y".to_string()))
        .collect::<Result<_, _>>()?;
    let tile = TileId { x: xy[0], y: xy[1] };
    let repeat: usize = value(arguments, "repeat").map_or(Ok(1), str::parse).map_err(|_| "--repeat N")?;
    let out = value(arguments, "out").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).map_err(|error| error.to_string())?;
    }
    let started = Instant::now();
    let files = Files::read(&release, tile)?;
    let square = Square::new(&weather, tile, &files)?;
    let gpu = Gpu::new(0)?;
    let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather)?;
    eprintln!("qm-ray-bench: {} ready in {:.1} s", gpu.name, started.elapsed().as_secs_f64());
    TIME_KERNELS.store(true, std::sync::atomic::Ordering::Relaxed);
    let launches = PathBuf::from(required("launches")?);
    let mut names: Vec<_> = std::fs::read_dir(&launches)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .collect();
    names.sort();
    let mut best_total = [0.0f64; 4];
    let (mut pairs_total, mut failed_total) = (0usize, 0usize);
    for name in names {
        let launch = read_launch(&launches.join(&name))?;
        let mut best = [f64::INFINITY; 4];
        for round in 0..repeat {
            *KERNEL_SECONDS.lock().unwrap() = [0.0; 4];
            let mut bands = gpu.bands(launch.reached)?;
            let (sums, failed) = gpu.evaluate_pairs(
                &device_square,
                (&launch.receivers, &launch.own),
                (&launch.candidates, &launch.at, &launch.weights),
                &mut bands,
            )?;
            let seconds = *KERNEL_SECONDS.lock().unwrap();
            for (b, s) in best.iter_mut().zip(seconds) {
                *b = b.min(s);
            }
            if round == 0 {
                pairs_total += launch.candidates.len();
                failed_total += failed.iter().filter(|&&f| f != 0).count();
                if let Some(out) = &out {
                    let flat = gpu.download(&bands)?;
                    let mut bytes = Vec::new();
                    sums.iter().flatten().for_each(|v| bytes.extend(v.to_le_bytes()));
                    failed.iter().for_each(|&v| bytes.extend((v as f32).to_le_bytes()));
                    flat.iter().for_each(|v| bytes.extend(v.to_le_bytes()));
                    std::fs::write(out.join(&name), bytes).map_err(|error| error.to_string())?;
                }
            }
        }
        for (t, b) in best_total.iter_mut().zip(best) {
            *t += b;
        }
        println!(
            "{name:?}: {} pairs, count {:.3} s, list {:.3} s, rays {:.3} s, bands {:.3} s",
            launch.candidates.len(),
            best[0],
            best[1],
            best[2],
            best[3]
        );
    }
    println!(
        "total: {pairs_total} pairs ({failed_total} failed), count {:.3} s, list {:.3} s, rays {:.3} s, bands {:.3} s",
        best_total[0], best_total[1], best_total[2], best_total[3]
    );
    Ok(())
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-ray-bench: {error}");
        std::process::exit(1);
    }
}
