//! `qm-paint-gpu check --prepared DIR --year YYYY --square X,Y [--pairs N] [--seed S] [--device D]
//! [--group G] [--near M] [--reach M] [--kind point|line]`:
//! the GPU's energy of random (receiver, source) pairs in one square against the CPU's (the
//! popup's `received_bands` at the painter's outdoor point), half the sources within `--near`
//! metres (300), half anywhere within `--reach` (the ground reach), only points or only lines with
//! `--kind`; prints the agreement, the failures, the worst pairs and both times.
//!
//! `qm-paint-gpu popup ... --square X,Y --out DIR [--pixels N] [--seed S]`: the popup's loudness
//! (Nden per layer and in total) at N random outdoor pixels of the square (300), written as a
//! sparse reference under DIR (every other cell not assessed), to score a map or an etalon against
//! with `compare`.
//!
//! `qm-paint-gpu etalon ... --square X,Y --out DIR [--device D] [--sample-step K]`: the square's
//! etalon (every outdoor pixel's loudness with every loud source exact, `paint::etalon`; with K one
//! drawn in each K x K cell, the rest not assessed) written as z13 tiles under DIR.
//!
//! `qm-paint-gpu compare ... --painted DIR --reference DIR --squares X,Y;...`: a painted map against
//! a reference (an etalon, or the popup's sparse answers) cell by cell where the reference assesses
//! one, per layer and the total by the owner's z13 contract in loudness level (rungs over 0.5, 1, 3
//! and 6 phon, presence, bias, the quiet band; a breached limit marked `!`), and the worst cell.

use paint::exact::{LAYERS, PairEnergy, Point, whole_energy};
use paint::paint::{NO_LEVEL, SILENCE, cells_of};
use paint::square::{Files, Square};
use paint_gpu::device::{DeviceSquare, Gpu};
use physics::bands::{BANDS, PERIODS, lden_energy};
use physics::weather::WeatherTable;
use popup::candidates::GROUND_REACH_M;
use popup::evaluate::{Bands, Scratch};
use popup::release::Release;
use popup::selection::SplitMix;
use rayon::prelude::*;
use std::path::PathBuf;
use std::time::Instant;
use tiles::geo::{Mercator, TileId};

fn option<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == format!("--{name}"))
        .map(|pair| pair[1].as_str())
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-paint-gpu: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: &[String]) -> Result<(), String> {
    let command = arguments.first().map(String::as_str);
    if !matches!(command, Some("check" | "popup" | "etalon" | "compare")) {
        return Err("usage: qm-paint-gpu check|popup|etalon|compare --prepared DIR --year YYYY ... (see the file's header)".into());
    }
    let required = |name: &str| option(arguments, name).ok_or(format!("--{name} is required"));
    let parse = |name: &str, default: u64| -> Result<u64, String> {
        option(arguments, name).map_or(Ok(default), |value| {
            value.parse().map_err(|_| format!("--{name}: not a number"))
        })
    };
    let release = Release::open(&PathBuf::from(required("prepared")?), required("year")?)?;
    if command == Some("compare") {
        return compare_maps(
            &PathBuf::from(required("painted")?),
            &PathBuf::from(required("reference")?),
            required("squares")?,
        );
    }
    if command == Some("popup") {
        let (x, y) = required("square")?
            .split_once(',')
            .ok_or("--square is X,Y")?;
        let tile = TileId {
            x: x.parse().map_err(|_| "--square x")?,
            y: y.parse().map_err(|_| "--square y")?,
        };
        return popup_reference(
            &release,
            tile,
            parse("pixels", 300)? as usize,
            &mut SplitMix(parse("seed", 1)?),
            &PathBuf::from(required("out")?),
        );
    }
    let weather = WeatherTable::read(&release.weather_path)?;
    let (x, y) = required("square")?
        .split_once(',')
        .ok_or("--square is X,Y")?;
    let tile = TileId {
        x: x.parse().map_err(|_| "--square x")?,
        y: y.parse().map_err(|_| "--square y")?,
    };
    let pairs = parse("pairs", 20_000)? as usize;
    let mut random = SplitMix(parse("seed", 1)?);
    let started = Instant::now();
    let files = Files::read(&release, tile)?;
    let square = Square::new(&weather, tile, &files)?;
    if command == Some("etalon") {
        let out = PathBuf::from(required("out")?);
        let sample_step = match parse("sample-step", 1)? {
            0 => return Err("--sample-step: at least 1".into()),
            step => step as usize,
        };
        let started = Instant::now();
        let gpu = Gpu::new(parse("device", 0)? as usize)?;
        let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather)?;
        let batch = paint_gpu::batch::GpuBatch {
            gpu: &gpu,
            device_square: &device_square,
            square: &square,
        };
        let grid = paint::paint::Grid::at_zoom(13).ok_or("no grid at zoom 13")?;
        let cells = paint::etalon::etalon(&square, grid, Some(&batch), sample_step)?;
        paint::hm3::write(&out, 13, (tile.x, tile.y), &cells)?;
        println!(
            "etalon {}/{}: {} sources, sample step {sample_step}, {:.1} s on {}",
            tile.x,
            tile.y,
            square.candidates.len(),
            started.elapsed().as_secs_f64(),
            gpu.name
        );
        return Ok(());
    }
    eprintln!(
        "square {}/{}: {} candidates, read and built in {:.1} s",
        tile.x,
        tile.y,
        square.candidates.len(),
        started.elapsed().as_secs_f64()
    );
    let started = Instant::now();
    let gpu = Gpu::new(parse("device", 0)? as usize)?;
    let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather)?;
    eprintln!(
        "{}: kernels compiled, {:.0} MB uploaded in {:.1} s",
        gpu.name,
        device_square.bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    // The pairs: outdoor receivers anywhere in the square, sources near or anywhere in reach.
    let corner = |dx: f64, dy: f64| {
        square.frame.to_metres(Mercator {
            x: f64::from(tile.x) + dx,
            y: f64::from(tile.y) + dy,
        })
    };
    let (a, b) = (corner(0.0, 0.0), corner(1.0, 1.0));
    let mut receivers = Vec::with_capacity(pairs);
    let mut chosen = Vec::with_capacity(pairs);
    let mut near = Vec::new();
    // With --group G, G neighbouring receivers (rows of 8, 6 m apart, z13's pixels) share one
    // source, as a warp of the painter would take them.
    let group = parse("group", 1)? as usize;
    let near_m = parse("near", 300)? as f64;
    let reach_m = parse("reach", GROUND_REACH_M as u64)? as f64;
    // --kind line, point or deck (a bridge's pieces): candidates of that kind only.
    let kind = option(arguments, "kind");
    let of_kind = |line: bool, on_deck: bool| match kind {
        Some("line") => line,
        Some("point") => !line,
        Some("deck") => on_deck,
        _ => true,
    };
    let mut base = [0.0, 0.0];
    while receivers.len() < pairs {
        let member = receivers.len() % group;
        if member == 0 {
            base = [
                a[0] + random.unit() * (b[0] - a[0]),
                a[1] + random.unit() * (b[1] - a[1]),
            ];
        }
        let position = [
            base[0] + 6.0 * (member % 8) as f64,
            base[1] - 6.0 * (member / 8) as f64,
        ];
        let indoor = square.obstacles.enclosing_building_id(position)?.is_some();
        if member > 0 {
            // A group stays whole: an indoor member stands at the group's first receiver.
            let first = receivers.len() - member;
            receivers.push(if indoor { receivers[first] } else { position });
            chosen.push(chosen[first]);
            continue;
        }
        if indoor {
            continue;
        }
        let index = if (receivers.len() / group).is_multiple_of(2) {
            square.index.within(position, position, near_m, &mut near);
            near.retain(|&index| {
                let candidate = &square.candidates[index as usize];
                candidate.distance_from(position) <= near_m
                    && of_kind(candidate.line, candidate.deck_m.is_some())
            });
            if near.is_empty() {
                continue;
            }
            near[(random.next_u64() % near.len() as u64) as usize]
        } else {
            let index = (random.next_u64() % square.candidates.len() as u64) as u32;
            let candidate = &square.candidates[index as usize];
            if candidate.distance_from(position) > reach_m
                || !of_kind(candidate.line, candidate.deck_m.is_some())
            {
                continue;
            }
            index
        };
        receivers.push(position);
        chosen.push(index);
    }
    // Launches of a quarter of a million pairs, each pair its own point (its bands apart, 150 MB);
    // a first launch brings the card to its clocks.
    const LAUNCH: usize = 1 << 18;
    let own = vec![0u64; LAUNCH];
    let point_of_pair: Vec<u32> = (0..LAUNCH as u32).collect();
    let mut bands = gpu.bands(LAUNCH)?;
    let warm = pairs.min(20_000);
    gpu.evaluate_pairs(
        &device_square,
        (&receivers[..warm], &own[..warm]),
        (&chosen[..warm], &point_of_pair[..warm]),
        &mut bands,
    )?;
    let (mut gpu_sums, mut failed, mut gpu_bands) = (Vec::new(), Vec::new(), Vec::new());
    let mut gpu_s = 0.0;
    for start in (0..pairs).step_by(LAUNCH) {
        let count = LAUNCH.min(pairs - start);
        bands = gpu.bands(LAUNCH)?;
        let started = Instant::now();
        let (sums, failures) = gpu.evaluate_pairs(
            &device_square,
            (&receivers[start..start + count], &own[..count]),
            (&chosen[start..start + count], &point_of_pair[..count]),
            &mut bands,
        )?;
        gpu_s += started.elapsed().as_secs_f64();
        gpu_sums.extend(sums);
        failed.extend(failures);
        gpu_bands.extend_from_slice(&gpu.download(&bands)?[..count * LAYERS * PERIODS * BANDS]);
    }
    let started = Instant::now();
    let cpu: Vec<Result<(PairEnergy, Bands), String>> = receivers
        .par_iter()
        .zip(&chosen)
        .map_init(Scratch::default, |scratch, (&position, &index)| {
            whole_energy(&square, &Point::at(&square, position)?, index, scratch)
        })
        .collect();
    let cpu_s = started.elapsed().as_secs_f64();
    // Agreement in dB where both answered: each pair's Lden energy mixed and in either state, and
    // each band carrying a thousandth of its period's energy or more.
    let (mut mixed, mut calm, mut downwind, mut spectrum) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut gpu_failed, mut cpu_failed, mut silent) = (0, 0, 0);
    let decibels = |g: f64, c: f64| {
        if g > 0.0 && c > 0.0 {
            Some((10.0 * (g / c).log10()).abs())
        } else if g == 0.0 && c == 0.0 {
            None
        } else {
            Some(f64::INFINITY)
        }
    };
    for pair in 0..pairs {
        if failed[pair] != 0 {
            gpu_failed += 1;
            continue;
        }
        let Ok((energy, cpu_bands)) = &cpu[pair] else {
            cpu_failed += 1;
            continue;
        };
        let card = |k: usize| std::array::from_fn(|p| f64::from(gpu_sums[pair][k * PERIODS + p]));
        let Some(difference) = decibels(lden_energy(&card(0)), lden_energy(&energy.mixed)) else {
            silent += 1;
            continue;
        };
        mixed.push((difference, pair));
        calm.extend(decibels(
            lden_energy(&card(1)),
            lden_energy(&energy.states[0]),
        ));
        downwind.extend(decibels(
            lden_energy(&card(2)),
            lden_energy(&energy.states[1]),
        ));
        let layer = square.candidates[chosen[pair] as usize].layer as usize;
        for (period, cpu_period) in cpu_bands.iter().enumerate() {
            for (band, &value) in cpu_period.iter().enumerate() {
                if value >= 1e-3 * energy.mixed[period] {
                    let at = ((pair * LAYERS + layer) * PERIODS + period) * BANDS + band;
                    spectrum.extend(decibels(f64::from(gpu_bands[at]), value));
                }
            }
        }
    }
    let count = |code: u32| failed.iter().filter(|&&c| c == code).count();
    println!(
        "pairs {pairs}: compared {}, silent {silent}, GPU failed {gpu_failed} (not read {}, no terrain {}, capacity {}), CPU failed {cpu_failed}",
        mixed.len(),
        count(paint_gpu::device::FAILED_NOT_READ),
        count(paint_gpu::device::FAILED_NO_TERRAIN),
        count(paint_gpu::device::FAILED_CAPACITY),
    );
    mixed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut lden: Vec<f64> = mixed.iter().map(|d| d.0).collect();
    for (name, values) in [
        ("Lden", &mut lden),
        ("calm Lden", &mut calm),
        ("downwind Lden", &mut downwind),
        ("band", &mut spectrum),
    ] {
        values.sort_by(f64::total_cmp);
        let at = |q: f64| {
            values
                .get(((values.len() as f64 - 1.0) * q).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        println!(
            "|GPU - CPU| {name} dB: median {:.2e}, p99 {:.2e}, p99.9 {:.2e}, max {:.2e}",
            at(0.5),
            at(0.99),
            at(0.999),
            at(1.0)
        );
    }
    println!(
        "time: GPU {gpu_s:.3} s ({:.0} ns a pair), CPU {cpu_s:.3} s on {} threads ({:.0} ns a pair)",
        gpu_s / pairs as f64 * 1e9,
        rayon::current_num_threads(),
        cpu_s / pairs as f64 * 1e9
    );
    for &(difference, pair) in mixed.iter().take(8) {
        let candidate = &square.candidates[chosen[pair] as usize];
        println!(
            "  worst {difference:.3e} dB: receiver {:.1},{:.1} candidate {} ({}, {:.0} m) GPU {:?} CPU {:?}",
            receivers[pair][0],
            receivers[pair][1],
            chosen[pair],
            if candidate.line { "line" } else { "point" },
            candidate.distance_from(receivers[pair]),
            &gpu_sums[pair][..PERIODS],
            cpu[pair].as_ref().ok().map(|(energy, _)| energy.mixed)
        );
    }
    for pair in (0..pairs).filter(|&p| failed[p] != 0).take(5) {
        println!(
            "  GPU failure {} at pair {pair}: CPU {:?}",
            failed[pair],
            cpu[pair]
                .as_ref()
                .map(|(energy, _)| lden_energy(&energy.mixed))
        );
    }
    Ok(())
}

/// The popup's answers at `pixels` random outdoor pixels of the square, as a sparse reference: the
/// square's tiles under `out`, every other cell not assessed (`compare` scores a map against it).
fn popup_reference(
    release: &Release,
    tile: TileId,
    pixels: usize,
    random: &mut SplitMix,
    out: &std::path::Path,
) -> Result<(), String> {
    const SIDE: usize = 1024;
    let chosen: Vec<usize> = (0..pixels)
        .map(|_| (random.next_u64() % (SIDE * SIDE) as u64) as usize)
        .collect();
    let started = Instant::now();
    let answers: Vec<Option<[u8; LAYERS + 1]>> = chosen
        .par_iter()
        .map(|&at| {
            let (lat, lon) = Mercator {
                x: f64::from(tile.x) + ((at % SIDE) as f64 + 0.5) / SIDE as f64,
                y: f64::from(tile.y) + ((at / SIDE) as f64 + 0.5) / SIDE as f64,
            }
            .to_degrees();
            let options = popup::answer::Options {
                exact: false,
                source: Vec::new(),
                piece: None,
            };
            let mut last = None;
            popup::answer::answer(release, lat, lon, &options, &mut |update| {
                if !update.partial && update.building.is_none() {
                    let mut ndens = [0.0; LAYERS + 1];
                    for (nden, layer) in ndens.iter_mut().zip(&update.layers) {
                        *nden = layer.nden_sone.unwrap_or(0.0);
                    }
                    ndens[LAYERS] = update.loudness.map_or(0.0, |loudness| loudness.nden_sone);
                    last = Some(cells_of(&ndens));
                }
                Ok(())
            })?;
            Ok(last)
        })
        .collect::<Result<_, String>>()?;
    let mut cells = vec![[NO_LEVEL; LAYERS + 1]; SIDE * SIDE];
    for (&at, answer) in chosen.iter().zip(answers) {
        if let Some(answer) = answer {
            cells[at] = answer;
        }
    }
    paint::hm3::write(out, 13, (tile.x, tile.y), &cells)?;
    eprintln!(
        "popup: {pixels} pixels answered in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// One layer's score by the owner's z13 contract (dev1's accuracy ladder, in decibels there) in the
/// stored bytes' loudness level (phon: a byte is half a phon, a phon 7.2 % of loudness), banded by
/// the reference: the rungs over the cells it paints (1 sone, 40 phon, and louder), the last joined
/// by paint-state flips over 6 phon; quieter an error of at most 10 phon; presence changes (across
/// 40 phon unless both lie inside (39.5, 40.5)) at most 0.25 % of the painted cells; the signed
/// mean over the painted cells at most 0.5 phon. Silence reads 0 phon.
#[derive(Default)]
struct Score {
    painted: usize,
    over: [usize; 4],
    presence: usize,
    signed: f64,
    quiet_worst: f64,
    /// The worst difference: phon, the global z13 pixel, the map's and the reference's bytes.
    worst: (f64, u32, u32, u8, u8),
}

/// The rungs: a difference over this many phon at most this percentage of the painted cells.
const RUNGS: [(f64, f64); 4] = [(0.5, 20.0), (1.0, 1.0), (3.0, 0.01), (6.0, 0.001)];

impl Score {
    fn add(&mut self, mine: u8, reference: u8, (x, y): (u32, u32)) {
        const PAINTED: u8 = 80;
        let level = |b: u8| if b >= SILENCE { 0 } else { b };
        let (reference, mine) = (level(reference), level(mine));
        let difference = (f64::from(mine) - f64::from(reference)) / 2.0;
        let edge_band = |b: u8| (79..=81).contains(&b);
        let flip = (reference >= PAINTED) != (mine >= PAINTED)
            && !(edge_band(reference) && edge_band(mine));
        self.presence += usize::from(flip);
        if reference >= PAINTED {
            self.painted += 1;
            self.signed += difference;
            for (k, &(limit, _)) in RUNGS[..3].iter().enumerate() {
                self.over[k] += usize::from(difference.abs() > limit);
            }
        } else {
            self.quiet_worst = self.quiet_worst.max(difference.abs());
            if !flip {
                return;
            }
        }
        self.over[3] += usize::from(difference.abs() > 6.0);
        if difference.abs() > self.worst.0 {
            self.worst = (difference.abs(), x, y, mine, reference);
        }
    }

    fn line(&self, name: &str) -> String {
        let share = |count: usize| 100.0 * count as f64 / self.painted.max(1) as f64;
        let bias = self.signed / self.painted.max(1) as f64;
        let mut line = format!("{name:<10} painted {:>5}:", self.painted);
        for (k, &(limit, allowed)) in RUNGS.iter().enumerate() {
            let mark = if share(self.over[k]) > allowed {
                "!"
            } else {
                ""
            };
            line += &format!(
                " >{limit} {:.2} %{mark} ({})",
                share(self.over[k]),
                self.over[k]
            );
        }
        format!(
            "{line}; presence {:.2} %{} ({}); bias {:+.2} phon{}; quiet worst {:.1} phon{}; worst {:.1} phon at {},{} (map {}, reference {})",
            share(self.presence),
            if share(self.presence) > 0.25 { "!" } else { "" },
            self.presence,
            bias,
            if bias.abs() > 0.5 { "!" } else { "" },
            self.quiet_worst,
            if self.quiet_worst > 10.0 { "!" } else { "" },
            self.worst.0,
            self.worst.1,
            self.worst.2,
            f64::from(self.worst.3) / 2.0,
            f64::from(self.worst.4) / 2.0
        )
    }
}

/// A painted map against a reference (the etalon, or the popup's sparse answers) over the z13
/// squares `squares` ("X,Y;X,Y"), cell by cell where the reference assesses one and either side
/// has a level, by the owner's z13 contract.
fn compare_maps(
    painted: &std::path::Path,
    reference: &std::path::Path,
    squares: &str,
) -> Result<(), String> {
    let mut scores: Vec<Score> = (0..=LAYERS).map(|_| Score::default()).collect();
    let mut cells = 0usize;
    for pair in squares.split(';') {
        let (x, y) = pair.split_once(',').ok_or("--squares is X,Y;X,Y")?;
        let square = (
            x.parse::<u32>().map_err(|_| "--squares x")?,
            y.parse::<u32>().map_err(|_| "--squares y")?,
        );
        let (mine, theirs) = (
            paint::hm3::read(painted, 13, square)?,
            paint::hm3::read(reference, 13, square)?,
        );
        for (at, (mine, theirs)) in mine.iter().zip(&theirs).enumerate() {
            let (theirs_total, mine_total) = (theirs[LAYERS], mine[LAYERS]);
            if theirs_total == NO_LEVEL || (theirs_total >= SILENCE && mine_total >= SILENCE) {
                continue;
            }
            cells += 1;
            let place = (
                square.0 * 1024 + (at % 1024) as u32,
                square.1 * 1024 + (at / 1024) as u32,
            );
            // A layer the reference does not assess there is not scored.
            for (score, (&m, &t)) in scores.iter_mut().zip(mine.iter().zip(theirs)) {
                if t != NO_LEVEL {
                    score.add(m, t, place);
                }
            }
        }
    }
    eprintln!("compare: {cells} cells");
    for (score, name) in scores.iter().zip(paint::hm3::layer_names()) {
        println!("{}", score.line(name));
    }
    Ok(())
}
