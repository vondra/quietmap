//! `qm-paint-gpu check --prepared DIR --year YYYY --square X,Y [--pairs N] [--seed S] [--device D]
//! [--group G] [--near M] [--reach M] [--kind point|line]`:
//! the GPU's energy of random (receiver, source) pairs in one square against the CPU's (the
//! popup's `received_bands` at the painter's outdoor point), half the sources within `--near`
//! metres (300), half anywhere within `--reach` (the ground reach), only points or only lines with
//! `--kind`; prints the agreement, the failures, the worst pairs and both times.
//!
//! `qm-paint-gpu pair ... --square X,Y --receiver x,y --candidate N`: one pair's energies on both,
//! and for a point source its ray's crossings as each walks them.
//!
//! `qm-paint-gpu popup ... --square X,Y --painted DIR [--pixels N] [--seed S] [--answers FILE]`:
//! the painted z13 square against the popup (the reference) at random outdoor pixels, per layer
//! and the total by the owner's z13 contract (rungs over 0.5, 1, 3 and 6 dB, presence, bias, the
//! quiet band; a breached limit marked `!`), and the worst pixel; the popup's answers kept in
//! FILE for the next map.
//!
//! `qm-paint-gpu etalon ... --square X,Y --out DIR [--device D]`: the square's etalon (every
//! outdoor pixel as the popup answers it, `paint::etalon`) written as z13 tiles under DIR.
//!
//! `qm-paint-gpu compare ... --painted DIR --reference DIR --squares X,Y;...`: a
//! painted map against the etalon, cell by cell, by the same contract.

use paint::exact::Point;
use paint::square::{Files, Square};
use paint_gpu::device::{DeviceSquare, Gpu};
use physics::bands::{PERIODS, lden_energy};
use physics::weather::WeatherTable;
use popup::candidates::GROUND_REACH_M;
use popup::evaluate::{Receiver, Scratch, period_sums, received_bands};
use popup::release::Release;
use rayon::prelude::*;
use std::path::PathBuf;
use std::time::Instant;
use tiles::geo::{Mercator, TileId};

/// SplitMix64.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

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
    if !matches!(
        command,
        Some("check" | "pair" | "popup" | "etalon" | "compare")
    ) {
        return Err("usage: qm-paint-gpu check|pair --prepared DIR --year YYYY --square X,Y [--pairs N] [--seed S] [--device D]".into());
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
        return against_popup(
            &release,
            tile,
            &PathBuf::from(required("painted")?),
            parse("pixels", 300)? as usize,
            &mut Random(parse("seed", 1)?),
            option(arguments, "answers").map(std::path::Path::new),
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
    let mut random = Random(parse("seed", 1)?);
    let started = Instant::now();
    let files = Files::read(&release, tile)?;
    let square = Square::new(&weather, tile, &files)?;
    if command == Some("etalon") {
        let out = PathBuf::from(required("out")?);
        let started = Instant::now();
        let gpu = Gpu::new(parse("device", 0)? as usize)?;
        let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather)?;
        let batch = paint_gpu::batch::GpuBatch {
            gpu: &gpu,
            device_square: &device_square,
            square: &square,
        };
        let lattice = paint::lattice::Lattice::new(&square, 1024, (128, 0), true, Some(&batch))?;
        let cells = paint::etalon::etalon(&square, 1024, &lattice, Some(&batch))?;
        paint::hm3::write(&out, 13, (tile.x, tile.y), &cells)?;
        println!(
            "etalon {}/{}: {} sources, {:.1} s on {}",
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
    if command == Some("pair") {
        let coordinates = |name: &str| -> Result<[f64; 2], String> {
            let (x, y) = required(name)?
                .split_once(',')
                .ok_or(format!("--{name} is x,y"))?;
            Ok([
                x.parse().map_err(|_| format!("--{name} x"))?,
                y.parse().map_err(|_| format!("--{name} y"))?,
            ])
        };
        // The receiver in frame metres, or a pixel's centre (--pixel x,y).
        let receiver = match option(arguments, "pixel") {
            Some(_) => {
                let [px, py] = coordinates("pixel")?;
                square.frame.to_metres(Mercator {
                    x: f64::from(tile.x) + (px + 0.5) / 1024.0,
                    y: f64::from(tile.y) + (py + 0.5) / 1024.0,
                })
            }
            None => coordinates("receiver")?,
        };
        return pair(
            &square,
            &gpu,
            &device_square,
            receiver,
            parse("candidate", 0)? as u32,
        );
    }
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
    let kind = option(arguments, "kind").map(|kind| kind == "line");
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
                    && kind.is_none_or(|line| candidate.line == line)
            });
            if near.is_empty() {
                continue;
            }
            near[(random.next() % near.len() as u64) as usize]
        } else {
            let index = (random.next() % square.candidates.len() as u64) as u32;
            let candidate = &square.candidates[index as usize];
            if candidate.distance_from(position) > reach_m
                || kind.is_some_and(|line| candidate.line != line)
            {
                continue;
            }
            index
        };
        receivers.push(position);
        chosen.push(index);
    }
    let own = vec![0u64; pairs];
    // A first launch brings the card to its clocks (QM_GPU_WARM=0: none).
    if std::env::var("QM_GPU_WARM").map_or(true, |v| v != "0") {
        let warm = pairs.min(20_000);
        gpu.evaluate_pairs(
            &device_square,
            &receivers[..warm],
            &own[..warm],
            &chosen[..warm],
        )?;
    }
    let started = Instant::now();
    let (gpu_periods, failed) = gpu.evaluate_pairs(&device_square, &receivers, &own, &chosen)?;
    let gpu_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let cpu_periods: Vec<Result<[f64; PERIODS], String>> = receivers
        .par_iter()
        .zip(&chosen)
        .map_init(Scratch::default, |scratch, (&position, &index)| {
            let point = Point::at(&square, position)?;
            let receiver = Receiver {
                ground: &square.ground,
                obstacles: &square.obstacles,
                position,
                altitude_m: point.altitude_m,
                weather: point.weather,
                reflection_db: 0.0,
                own_footprint: 0,
            };
            let mut candidate = square.candidates[index as usize].clone();
            candidate.distance_m = candidate.distance_from(position);
            let attribute = &square.attributes[candidate.attribute];
            Ok(period_sums(
                &received_bands(&receiver, &candidate, attribute, scratch)?.bands,
            ))
        })
        .collect();
    let cpu_s = started.elapsed().as_secs_f64();
    // Agreement in dB of each pair's Lden energy, where both answered.
    let mut differences: Vec<(f64, usize)> = Vec::new();
    let (mut gpu_failed, mut cpu_failed, mut silent) = (0, 0, 0);
    for pair in 0..pairs {
        if failed[pair] != 0 {
            gpu_failed += 1;
            continue;
        }
        let Ok(cpu) = &cpu_periods[pair] else {
            cpu_failed += 1;
            continue;
        };
        let (g, c) = (lden_energy(&gpu_periods[pair]), lden_energy(cpu));
        if g == 0.0 && c == 0.0 {
            silent += 1;
            continue;
        }
        let difference = if g > 0.0 && c > 0.0 {
            10.0 * (g / c).log10()
        } else {
            f64::INFINITY
        };
        differences.push((difference.abs(), pair));
    }
    differences.sort_by(|a, b| b.0.total_cmp(&a.0));
    let quantile = |q: f64| {
        let mut values: Vec<f64> = differences.iter().map(|d| d.0).collect();
        values.sort_by(f64::total_cmp);
        values
            .get(((values.len() as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "pairs {pairs}: compared {}, silent {silent}, GPU failed {gpu_failed} (not read {}, no terrain {}, capacity {}), CPU failed {cpu_failed}",
        differences.len(),
        failed
            .iter()
            .filter(|&&code| code == paint_gpu::device::FAILED_NOT_READ)
            .count(),
        failed
            .iter()
            .filter(|&&code| code == paint_gpu::device::FAILED_NO_TERRAIN)
            .count(),
        failed
            .iter()
            .filter(|&&code| code == paint_gpu::device::FAILED_CAPACITY)
            .count(),
    );
    println!(
        "|GPU - CPU| Lden dB: median {:.2e}, p99 {:.2e}, p99.9 {:.2e}, max {:.2e}",
        quantile(0.5),
        quantile(0.99),
        quantile(0.999),
        differences.first().map_or(0.0, |d| d.0)
    );
    println!(
        "time: GPU {gpu_s:.3} s ({:.0} ns a pair), CPU {cpu_s:.3} s on {} threads ({:.0} ns a pair)",
        gpu_s / pairs as f64 * 1e9,
        rayon::current_num_threads(),
        cpu_s / pairs as f64 * 1e9
    );
    for &(difference, pair) in differences.iter().take(8) {
        let candidate = &square.candidates[chosen[pair] as usize];
        println!(
            "  worst {difference:.3e} dB: receiver {:.1},{:.1} candidate {} ({}, {:.0} m) GPU {:?} CPU {:?}",
            receivers[pair][0],
            receivers[pair][1],
            chosen[pair],
            if candidate.line { "line" } else { "point" },
            candidate.distance_from(receivers[pair]),
            gpu_periods[pair],
            cpu_periods[pair].as_ref().ok()
        );
    }
    let failures: Vec<(usize, u32)> = (0..pairs)
        .filter(|&p| failed[p] != 0)
        .map(|p| (p, failed[p]))
        .take(5)
        .collect();
    for (pair, code) in failures {
        println!(
            "  GPU failure {code} at pair {pair}: CPU {:?}",
            cpu_periods[pair].as_ref().map(lden_energy)
        );
    }
    Ok(())
}

/// One pair on both sides: the energies, and a point source's crossings as each walks them.
fn pair(
    square: &Square,
    gpu: &Gpu,
    device_square: &DeviceSquare,
    position: [f64; 2],
    index: u32,
) -> Result<(), String> {
    let candidate = &square.candidates[index as usize];
    let (gpu_periods, failed) = gpu.evaluate_pairs(device_square, &[position], &[0], &[index])?;
    let point = Point::at(square, position)?;
    let receiver = Receiver {
        ground: &square.ground,
        obstacles: &square.obstacles,
        position,
        altitude_m: point.altitude_m,
        weather: point.weather,
        reflection_db: 0.0,
        own_footprint: 0,
    };
    let mut evaluated = candidate.clone();
    evaluated.distance_m = evaluated.distance_from(position);
    let attribute = &square.attributes[candidate.attribute];
    let cpu = received_bands(&receiver, &evaluated, attribute, &mut Scratch::default())
        .map(|received| period_sums(&received.bands));
    println!(
        "candidate {index} ({}, {:.1} m, own footprint {}): GPU {:?} (failed {}) CPU {:?}",
        if candidate.line { "line" } else { "point" },
        evaluated.distance_m,
        attribute.footprint_id,
        gpu_periods[0],
        failed[0],
        cpu
    );
    if candidate.line {
        // The quadrature nodes on both sides, in order.
        let mut cpu_nodes = Vec::new();
        popup::evaluate::source_rays(
            &receiver,
            &evaluated,
            attribute,
            &mut Scratch::default(),
            &mut |ray| {
                // evaluate.rs add_ray, summed per period.
                let energy: [f64; PERIODS] = std::array::from_fn(|period| {
                    (0..physics::bands::BANDS)
                        .map(|band| {
                            ray.weight
                                * attribute.energy[period][band]
                                * ray.terms.transfer.periods[period][band]
                        })
                        .sum()
                });
                let [a, _] = candidate.ends_m;
                cpu_nodes.push((
                    (ray.from_m[0] - a[0]).hypot(ray.from_m[1] - a[1]),
                    ray.angle_rad,
                    energy,
                ));
            },
        )?;
        let (gpu_nodes, failed) = gpu.line_pair_nodes(device_square, position, index)?;
        println!(
            "nodes: CPU {}, GPU {} (failed {failed})",
            cpu_nodes.len(),
            gpu_nodes.len()
        );
        for k in 0..cpu_nodes.len().max(gpu_nodes.len()) {
            let cpu = cpu_nodes.get(k).map(|(along, weight, e)| {
                format!("along {along:8.3} w {weight:.5} E {:.4e}", lden_energy(e))
            });
            let gpu = gpu_nodes.get(k).map(|(along, weight, blocked, e)| {
                format!(
                    "along {along:8.3} w {weight:.5} {} E {:.4e}",
                    if *blocked { "B" } else { "-" },
                    lden_energy(e)
                )
            });
            println!(
                "  {k:3}  CPU {:<44}  GPU {}",
                cpu.unwrap_or_default(),
                gpu.unwrap_or_default()
            );
        }
        // The rays of the first two nodes, as each side walks them.
        let [a, b] = candidate.ends_m;
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        for &(along, _, _) in cpu_nodes.iter().take(2) {
            let f = along / length;
            let point = [a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])];
            compare_crossings(square, gpu, device_square, point, position)?;
            // The CPU's terms of this ray, per state.
            let mut scratch = Scratch::default();
            let terms = popup::evaluate::trace(&receiver, point, attribute, &mut scratch)?;
            println!(
                "  profile: {} samples, t {:?}",
                scratch.profile().t.len(),
                &scratch.profile().t
            );
            for (state, boundary) in terms.boundaries.iter().enumerate() {
                println!(
                    "  state {state}: path difference {:.6} m, attenuation {:?}, whole-path ground {:?}",
                    boundary.path_difference_m,
                    boundary.attenuation_db.map(|v| (v * 100.0).round() / 100.0),
                    boundary
                        .whole_path_ground_db
                        .map(|v| (v * 100.0).round() / 100.0)
                );
            }
        }
        return Ok(());
    }
    compare_crossings(square, gpu, device_square, candidate.ends_m[0], position)
}

/// The crossings of one ray as the CPU and the card walk them, side by side.
fn compare_crossings(
    square: &Square,
    gpu: &Gpu,
    device_square: &DeviceSquare,
    source: [f64; 2],
    position: [f64; 2],
) -> Result<(), String> {
    let mut cpu_crossings = Vec::new();
    square
        .obstacles
        .crossings(source, position, &mut cpu_crossings)?;
    let (gpu_crossings, walk_failed) = gpu.ray_crossings(device_square, source, position)?;
    println!(
        "crossings: CPU {}, GPU {} (walk failed {walk_failed})",
        cpu_crossings.len(),
        gpu_crossings.len()
    );
    for k in 0..cpu_crossings.len().max(gpu_crossings.len()) {
        let cpu = cpu_crossings.get(k).map(|c| {
            format!(
                "t {:.7} fp {} h {:.1} {}",
                c.t,
                c.footprint_id,
                c.height_m,
                if c.building { "building" } else { "wall" }
            )
        });
        let gpu = gpu_crossings
            .get(k)
            .map(|&(t, footprint, height, building)| {
                format!(
                    "t {t:.7} fp {footprint} h {height:.1} {}",
                    if building { "building" } else { "wall" }
                )
            });
        println!(
            "  {k:3}  CPU {:<48}  GPU {}",
            cpu.unwrap_or_default(),
            gpu.unwrap_or_default()
        );
    }
    Ok(())
}

/// The painted z13 square at random outdoor pixels against the popup's answer there; the answers
/// read from `answers` when it exists, else computed and written there.
fn against_popup(
    release: &Release,
    tile: TileId,
    painted: &std::path::Path,
    pixels: usize,
    random: &mut Random,
    answers_path: Option<&std::path::Path>,
) -> Result<(), String> {
    use paint::hm3::tile_path;
    const SIDE: u32 = 1024;
    const TILE: u32 = 512;
    // The painted cells: per layer, the square's four z13 tiles.
    let read = |name: &str, x: u32, y: u32| -> Result<u8, String> {
        let path = tile_path(
            painted,
            name,
            13,
            tile.x * 2 + x / TILE,
            tile.y * 2 + y / TILE,
        );
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(bytes[6 + ((y % TILE) * TILE + x % TILE) as usize])
    };
    // One line a pixel: x, y and the popup's Lden energy per layer, or "building".
    if let Some(cached) = answers_path.and_then(|path| std::fs::read_to_string(path).ok()) {
        let mut chosen = Vec::new();
        let mut answers = Vec::new();
        for line in cached.lines() {
            let fields: Vec<&str> = line.split(' ').collect();
            let number = |k: usize| {
                fields[k]
                    .parse::<f64>()
                    .map_err(|_| format!("answers: {line}"))
            };
            chosen.push((number(0)? as u32, number(1)? as u32));
            answers.push(if fields[2] == "building" {
                None
            } else {
                Some(
                    (2..fields.len())
                        .map(number)
                        .collect::<Result<Vec<f64>, String>>()?,
                )
            });
        }
        return score_against_popup(&chosen, &answers, &read);
    }
    // Pixels chosen regardless of the map, so a map missing a level is seen; the popup says
    // which stand in a building.
    let chosen: Vec<(u32, u32)> = (0..pixels)
        .map(|_| {
            let mut pick = || (random.next() % u64::from(SIDE)) as u32;
            (pick(), pick())
        })
        .collect();
    let started = Instant::now();
    let answers: Vec<Option<Vec<f64>>> = chosen
        .par_iter()
        .map(|&(x, y)| {
            let (lat, lon) = Mercator {
                x: f64::from(tile.x) + (f64::from(x) + 0.5) / f64::from(SIDE),
                y: f64::from(tile.y) + (f64::from(y) + 0.5) / f64::from(SIDE),
            }
            .to_degrees();
            let options = popup::answer::Options {
                exact: false,
                pieces: 0,
            };
            let mut last = None;
            popup::answer::answer(release, lat, lon, &options, &mut |update| {
                if !update.partial && update.building.is_none() {
                    last = Some(
                        update
                            .layers
                            .iter()
                            .map(|layer| physics::bands::lden_energy(&layer.energy))
                            .collect::<Vec<f64>>(),
                    );
                }
                Ok(())
            })?;
            Ok(last)
        })
        .collect::<Result<_, String>>()?;
    eprintln!(
        "popup: {} answers in {:.1} s",
        answers.len(),
        started.elapsed().as_secs_f64()
    );
    if let Some(path) = answers_path {
        let lines: Vec<String> = (chosen.iter().zip(&answers))
            .map(|(&(x, y), answer)| match answer {
                Some(energies) => format!(
                    "{x} {y} {}",
                    energies
                        .iter()
                        .map(|e| format!("{e:e}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                None => format!("{x} {y} building"),
            })
            .collect();
        std::fs::write(path, lines.join("\n") + "\n")
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    score_against_popup(&chosen, &answers, &read)
}

/// The map's cells against the popup's answers at the chosen pixels, by the owner's z13 contract.
fn score_against_popup(
    chosen: &[(u32, u32)],
    answers: &[Option<Vec<f64>>],
    read: &dyn Fn(&str, u32, u32) -> Result<u8, String>,
) -> Result<(), String> {
    let names = paint::hm3::layer_names();
    // The popup's level as the map stores it: twice the Lden rounded, 255 under 0 dB.
    let byte = |energy: f64| -> u8 {
        if energy < 1.0 {
            255
        } else {
            (2.0 * physics::bands::level_db(energy)).round().min(254.0) as u8
        }
    };
    // The owner's z13 contract (dev1's accuracy ladder) per layer, in the stored byte, banded by
    // the reference: the rungs over the cells it paints (>= 30 dB), the last joined by paint-state
    // flips over 6 dB; under 30 dB an error of at most 10 dB; presence changes (across 30 dB unless
    // both lie inside (29, 31) dB) at most 0.25 % of the painted cells; the signed mean over the
    // painted cells at most 0.5 dB. Both sides are outdoors, so no level (255) is a level under
    // 0 dB, read as 0 dB.
    const PAINTED: u8 = 60;
    let rungs = [(0.5, 20.0), (1.0, 1.0), (3.0, 0.01), (6.0, 0.001)];
    for (slot, name) in names.iter().enumerate() {
        let (mut painted, mut over, mut presence, mut signed, mut quiet_worst) =
            (0usize, [0usize; 4], 0usize, 0.0f64, 0.0f64);
        let mut worst = (0.0f64, 0u32, 0u32, 0u8, 0u8);
        for (&(x, y), answer) in chosen.iter().zip(answers) {
            let Some(energies) = answer else { continue };
            let reference = byte(if slot < energies.len() {
                energies[slot]
            } else {
                energies.iter().sum()
            });
            let level = |b: u8| if b == 255 { 0 } else { b };
            let (reference, mine) = (level(reference), level(read(name, x, y)?));
            let difference = (f64::from(mine) - f64::from(reference)) / 2.0;
            let edge_band = |b: u8| (59..=61).contains(&b);
            let flip = (reference >= PAINTED) != (mine >= PAINTED)
                && !(edge_band(reference) && edge_band(mine));
            presence += usize::from(flip);
            if reference >= PAINTED {
                painted += 1;
                signed += difference;
                for (k, &(limit, _)) in rungs[..3].iter().enumerate() {
                    over[k] += usize::from(difference.abs() > limit);
                }
            } else {
                quiet_worst = quiet_worst.max(difference.abs());
                if !flip {
                    continue;
                }
            }
            over[3] += usize::from(difference.abs() > 6.0);
            if difference.abs() > worst.0 {
                worst = (difference.abs(), x, y, mine, reference);
            }
        }
        let share = |count: usize| 100.0 * count as f64 / painted.max(1) as f64;
        let mut line = format!("{name:<10} painted {painted:>5}:");
        for (k, &(limit, allowed)) in rungs.iter().enumerate() {
            let mark = if share(over[k]) > allowed { "!" } else { "" };
            line += &format!(" >{limit} {:.2} %{mark} ({})", share(over[k]), over[k]);
        }
        println!(
            "{line}; presence {:.2} %{} ({presence}); bias {:+.2} dB; quiet worst {quiet_worst:.1} dB{}; worst {:.1} dB at {},{} (map {}, popup {})",
            share(presence),
            if share(presence) > 0.25 { "!" } else { "" },
            signed / painted.max(1) as f64,
            if quiet_worst > 10.0 { "!" } else { "" },
            worst.0,
            worst.1,
            worst.2,
            f64::from(worst.3) / 2.0,
            f64::from(worst.4) / 2.0
        );
    }
    Ok(())
}

/// A painted map against a reference map (the etalon) over every cell of the z13 squares
/// `squares` ("X,Y;X,Y"), by the owner's z13 contract.
fn compare_maps(
    painted: &std::path::Path,
    reference: &std::path::Path,
    squares: &str,
) -> Result<(), String> {
    use paint::hm3::tile_path;
    let mut chosen: Vec<(u32, u32)> = Vec::new();
    let mut answers: Vec<Option<Vec<f64>>> = Vec::new();
    let mut tiles = Vec::new();
    for pair in squares.split(';') {
        let (x, y) = pair.split_once(',').ok_or("--squares is X,Y;X,Y")?;
        tiles.push((
            x.parse::<u32>().map_err(|_| "--squares x")?,
            y.parse::<u32>().map_err(|_| "--squares y")?,
        ));
    }
    // The reference's cells as energies, one pixel at a time over every square, for the scorer.
    let names = paint::hm3::layer_names();
    let read_tile =
        |root: &std::path::Path, name: &str, x: u32, y: u32| -> Result<Vec<u8>, String> {
            let path = tile_path(root, name, 13, x, y);
            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
    let mut painted_bytes: std::collections::HashMap<(usize, u32, u32), Vec<u8>> =
        Default::default();
    for (sx, sy) in &tiles {
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = (sx * 2 + dx, sy * 2 + dy);
            let mut reference_layers = Vec::new();
            for (slot, name) in names.iter().enumerate() {
                reference_layers.push(read_tile(reference, name, x, y)?);
                painted_bytes.insert((slot, x, y), read_tile(painted, name, x, y)?);
            }
            for k in 0..512 * 512 {
                if reference_layers[names.len() - 1][6 + k] == 255
                    && painted_bytes[&(names.len() - 1, x, y)][6 + k] == 255
                {
                    continue;
                }
                // The global pixel coordinates (x, y in z13 pixels) carried in `chosen`.
                chosen.push((x * 512 + (k % 512) as u32, y * 512 + (k / 512) as u32));
                // Every layer and the stored total (the scorer reads a slot it is given).
                answers.push(Some(
                    (0..names.len())
                        .map(|slot| match reference_layers[slot][6 + k] {
                            255 => 0.0,
                            b => physics::bands::energy(f64::from(b) / 2.0),
                        })
                        .collect(),
                ));
            }
        }
    }
    let read = |name: &str, gx: u32, gy: u32| -> Result<u8, String> {
        let slot = names.iter().position(|n| *n == name).ok_or("layer")?;
        Ok(painted_bytes[&(slot, gx / 512, gy / 512)][6 + ((gy % 512) * 512 + gx % 512) as usize])
    };
    eprintln!("compare: {} cells", chosen.len());
    score_against_popup(&chosen, &answers, &read)
}
