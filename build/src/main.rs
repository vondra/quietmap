//! `qm-build`: builds the prepared z12 tiles of a release, one builder per kind.
//!
//! Until the builders read the sources themselves, `qm-build dev4` converts squares of the dev4
//! z9 tree: `qm-build dev4 --prepared DIR --rasters DIR --out DIR --squares X:Y[,X:Y..]
//! [--kinds terrain,obstacles,sources] [--airport-traffic DIR] [--climate DIR] [--tent FILE]
//! [--road-traffic DIR] [--bus FILE] [--worship FILE] [--national-dem FILE,..]` (sources need the
//! airport traffic, the climate grids of `fetch/worldclim.sh` and the TEN-T freight lines of
//! `fetch/tent.sh`, and take the roads' building traffic of `qm-build traffic`, the bus routes of
//! `fetch/bus.sh` and the places of worship of `fetch/worship.sh` when given; terrain lays the
//! national models over dev4's heights);
//! `qm-build weather --rasters DIR --out FILE` cuts the global weather table; `qm-build complete
//! --out DIR --note TEXT` writes the completion marker last.
//!
//! Aircraft: `qm-build geoid --tiff FILE --out FILE` converts the EGM2008 GeoTIFF once;
//! `qm-build aircraft-segments (--days D,D.. [--increment-days D,..] | --anchor YYYY-MM)
//! --primary DIR [--secondary DIR] --rasters DIR --geoid FILE --out DIR [--boxes S,W,N,E;..]
//! [--threads N] [--national-dem FILE,..]` writes per-day segments, flight tables and receipts
//! into a scratch directory;
//! `qm-build airport-traffic --prepared DIR --segments DIR --days D,.. [--increment-days D,..]
//! --squares X:Y[,X:Y..] --out DIR` projects the window's ground legs onto the aeroway lines of
//! the squares (and their neighbours) once, one traffic file per square.
//!
//! Roads: `qm-build traffic --prepared DIR --squares X:Y[,X:Y..] --out DIR` routes the buildings'
//! trip ends down the local streets and grids them, one file per square, for the sources.

mod aircraft;
mod airport;
mod boxes;
mod climate;
mod dev4;
mod low_profile;
mod obstacles;
mod output;
mod period;
mod screening;
mod sources;
mod structures;
mod terrain;
mod traffic;
mod weather;

use dev4::{Dev4, Square};
use std::path::{Path, PathBuf};

struct Arguments {
    values: Vec<(String, String)>,
}

impl Arguments {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut values = Vec::new();
        for pair in arguments.chunks(2) {
            match pair {
                [key, value] if key.starts_with("--") => {
                    values.push((key[2..].to_string(), value.clone()))
                }
                _ => return Err(format!("expected --key value pairs, got {pair:?}")),
            }
        }
        Ok(Arguments { values })
    }

    fn get(&self, key: &str) -> Result<&str, String> {
        self.optional(key).ok_or_else(|| format!("missing --{key}"))
    }

    fn optional(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// The squares of `--squares`: `X:Y,X:Y,..`, `all` (every z9 square of the world) or `@FILE`
/// (the same pairs, by commas or lines: a world's list is too long for one argument).
fn parse_squares(text: &str) -> Result<Vec<Square>, String> {
    if text == "all" {
        return Ok((0..dev4::Z9_PER_AXIS)
            .flat_map(|x| (0..dev4::Z9_PER_AXIS).map(move |y| Square { x, y }))
            .collect());
    }
    if let Some(path) = text.strip_prefix('@') {
        let listed = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        let pairs: Vec<&str> = listed
            .split([',', '\n'])
            .map(str::trim)
            .filter(|pair| !pair.is_empty())
            .collect();
        return parse_squares(&pairs.join(","));
    }
    text.split(',')
        .map(|pair| {
            let (x, y) = pair
                .split_once(':')
                .ok_or_else(|| format!("square {pair:?} is not X:Y"))?;
            Ok(Square {
                x: x.parse().map_err(|_| format!("bad x in {pair:?}"))?,
                y: y.parse().map_err(|_| format!("bad y in {pair:?}"))?,
            })
        })
        .collect()
}

/// The sampling window: `--days` (baseline) and `--increment-days`, comma separated.
/// The national terrain models of `--national-dem FILE,..` (none without it).
fn national_models(options: &Arguments) -> Result<Vec<terrain::national::NationalHeights>, String> {
    options
        .optional("national-dem")
        .map_or(Ok(Vec::new()), |paths| {
            paths
                .split(',')
                .map(|path| terrain::national::NationalHeights::read(Path::new(path)))
                .collect()
        })
}

fn sampling_window(options: &Arguments) -> boxes::Window {
    let listed = |key: &str| {
        options
            .optional(key)
            .map(|days| days.split(',').map(str::to_string).collect())
            .unwrap_or_default()
    };
    boxes::Window {
        baseline_days: listed("days"),
        increment_days: listed("increment-days"),
    }
}

/// The aircraft box rule: `--kind aircraft|aircraft-far`, `--level-step-db` (D) and `--pieces`
/// (kept per box), else the kind's defaults.
fn box_rule(options: &Arguments) -> Result<boxes::BoxRule, String> {
    let mut rule = match options.optional("kind") {
        None | Some("aircraft") => boxes::BoxRule::default(),
        Some("aircraft-far") => boxes::BoxRule::far(),
        Some(other) => return Err(format!("bad --kind {other}")),
    };
    if let Some(step) = options.optional("level-step-db") {
        rule.level_step_db = step
            .parse()
            .map_err(|_| format!("bad --level-step-db {step}"))?;
    }
    if let Some(pieces) = options.optional("pieces") {
        rule.pieces = pieces
            .parse()
            .map_err(|_| format!("bad --pieces {pieces}"))?;
    }
    Ok(rule)
}

/// The global weather table (`qm-build weather`): the yearly atmosphere the boxes are summed in.
fn read_weather(path: &str) -> Result<physics::weather::WeatherTable, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    physics::weather::WeatherTable::parse(&bytes)
}

fn run(arguments: &[String]) -> Result<(), String> {
    let (command, rest) = arguments
        .split_first()
        .ok_or("usage: qm-build dev4|weather|complete|geoid|aircraft-segments|airport-traffic --key value ..")?;
    let options = Arguments::parse(rest)?;
    let out = PathBuf::from(options.get("out")?);
    match command.as_str() {
        "dev4" => {
            let dev4 = Dev4 {
                prepared: options.get("prepared")?.into(),
                rasters: options.get("rasters")?.into(),
            };
            let squares = parse_squares(options.get("squares")?)?;
            let kinds = options.optional("kinds").unwrap_or("terrain");
            for kind in kinds.split(',') {
                let started = std::time::Instant::now();
                let written = match kind {
                    "terrain" => {
                        terrain::build(&dev4, &squares, &national_models(&options)?, &out)?
                    }
                    "obstacles" => obstacles::build(&dev4, &squares, &out)?,
                    "sources" => sources::build(
                        (
                            &dev4,
                            &climate::Climate::load(Path::new(options.get("climate")?))?,
                            &sources::tent::FreightNetwork::load(Path::new(options.get("tent")?))?,
                        ),
                        (
                            Path::new(options.get("airport-traffic")?),
                            options.optional("road-traffic").map(Path::new),
                            options
                                .optional("bus")
                                .map(|path| sources::bus::BusRoutes::load(Path::new(path)))
                                .transpose()?
                                .as_ref(),
                            options
                                .optional("worship")
                                .map(|path| sources::worship::WorshipSites::load(Path::new(path)))
                                .transpose()?
                                .as_ref(),
                        ),
                        &squares,
                        &out,
                    )?,
                    other => return Err(format!("unknown kind {other}")),
                };
                eprintln!(
                    "{kind}: {written} tiles in {:.1} s",
                    started.elapsed().as_secs_f64()
                );
            }
            Ok(())
        }
        "weather" => {
            let dev4 = Dev4 {
                prepared: PathBuf::new(),
                rasters: options.get("rasters")?.into(),
            };
            weather::build(&dev4, &out)
        }
        "complete" => output::mark_complete(&out, options.get("note")?),
        "geoid" => aircraft::geoid::build(Path::new(options.get("tiff")?), &out),
        "aircraft-segments" => {
            if let Some(threads) = options.optional("threads") {
                let threads = threads
                    .parse()
                    .map_err(|_| format!("bad --threads {threads}"))?;
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build_global()
                    .map_err(|error| error.to_string())?;
            }
            let days = match (options.optional("days"), options.optional("anchor")) {
                (Some(days), None) => {
                    aircraft::Days::listed(days, options.optional("increment-days"))?
                }
                (None, Some(anchor)) => aircraft::Days::anchor(anchor)?,
                _ => return Err("give --days or --anchor".into()),
            };
            aircraft::run(aircraft::Run {
                days,
                primary: Path::new(options.get("primary")?),
                secondary: options.optional("secondary").map(Path::new),
                rasters: Path::new(options.get("rasters")?),
                geoid: Path::new(options.get("geoid")?),
                boxes: options.optional("boxes"),
                out: &out,
                national: national_models(&options)?,
            })
        }
        "aircraft-shuffle" => {
            let window = sampling_window(&options);
            let scope = parse_squares(options.get("squares")?)?
                .into_iter()
                .collect();
            let left = boxes::shuffle::shuffle(
                Path::new(options.get("segments")?),
                &window,
                &scope,
                &out,
            )?;
            eprintln!("aircraft shuffle: {left} days not written yet");
            Ok(())
        }
        "aircraft-boxes" => {
            let rule = box_rule(&options)?;
            let shuffled = Path::new(options.get("shuffled")?);
            let days = boxes::shuffle::shuffled_days(shuffled)?;
            let weather = read_weather(options.get("weather")?)?;
            eprintln!("aircraft boxes: {} days", days.len());
            // `--progress DIR`: a marker per square built, so that a rerun resumes.
            let progress = options.optional("progress").map(PathBuf::from);
            if let Some(progress) = &progress {
                std::fs::create_dir_all(progress).map_err(|error| error.to_string())?;
            }
            for square in parse_squares(options.get("squares")?)? {
                let marker = progress.as_ref().map(|progress| {
                    progress.join(format!("{}-{}-{}", rule.kind.name(), square.x, square.y))
                });
                if marker.as_ref().is_some_and(|marker| marker.exists()) {
                    continue;
                }
                let started = std::time::Instant::now();
                let (tiles, written) = boxes::build_square(
                    (shuffled, &days),
                    (square, &weather),
                    Path::new(options.get("terrain")?),
                    rule,
                    &out,
                )?;
                eprintln!(
                    "aircraft boxes: square {}/{}: {written} boxes in {tiles} tiles, {:.0} s",
                    square.x,
                    square.y,
                    started.elapsed().as_secs_f64()
                );
                if let Some(marker) = &marker {
                    std::fs::write(marker, b"").map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        }
        "aircraft-check" => {
            let window = sampling_window(&options);
            let points: Vec<serde_json::Value> = serde_json::from_str(
                &std::fs::read_to_string(options.get("points")?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let only: Vec<&str> = options
                .optional("only")
                .map_or(Vec::new(), |names| names.split(',').collect());
            let terrain = Path::new(options.get("terrain")?);
            let chosen: Vec<(&str, boxes::check::CheckPoint)> = points
                .iter()
                .map(|point| {
                    let (lat, lon) = (
                        point["lat"].as_f64().unwrap_or(0.0),
                        point["lon"].as_f64().unwrap_or(0.0),
                    );
                    let ground_m = boxes::check::ground_at(terrain, (lat, lon));
                    let name = point["name"].as_str().unwrap_or("");
                    (name, boxes::check::CheckPoint { lat, lon, ground_m })
                })
                .filter(|(name, _)| only.is_empty() || only.contains(name))
                .collect();
            let diagnosed = match options.optional("diagnose") {
                Some(name) => Some(
                    chosen
                        .iter()
                        .position(|(chosen, _)| *chosen == name)
                        .ok_or_else(|| format!("--diagnose {name}: no such point"))?,
                ),
                None => None,
            };
            let weather = read_weather(options.get("weather")?)?;
            let compared = boxes::check::compare(
                (Path::new(options.get("segments")?), &weather),
                &window,
                (&out, box_rule(&options)?.level_step_db),
                terrain,
                (
                    &chosen.iter().map(|(_, point)| *point).collect::<Vec<_>>(),
                    diagnosed,
                ),
            )?;
            for ((name, _), report) in chosen.iter().zip(compared) {
                println!(
                    "{}",
                    serde_json::json!({
                        "point": name,
                        "exact_leq": report.exact,
                        "boxed_leq": report.boxed,
                        "beyond_reach_leq": report.beyond,
                        "boxed_as_read_leq": report.boxed_as_read,
                        "megabytes_fine_as_read": report.megabytes,
                        "near_ground": {
                            "exact_leq": report.near_ground[0],
                            "boxed_leq": report.near_ground[1],
                        },
                        "by_distance": report.by_distance,
                        "aloft": {
                            "exact_leq": report.aloft[0],
                            "boxed_leq": report.aloft[1],
                        },
                        "exact_top": report
                            .exact_top
                            .iter()
                            .map(|(flight, sel)| serde_json::json!([format!("{flight:016x}"), sel]))
                            .collect::<Vec<_>>(),
                        "diagnosis": report
                            .diagnosis
                            .iter()
                            .map(|found| serde_json::json!({
                                "centroid": found.centroid,
                                "distance_m": found.distance_m,
                                "zoom": found.zoom,
                                "clearance_m": found.clearance_m,
                                "flights": found.flights,
                                "axis_deg": found.axis_deg,
                                "gradient": found.gradient,
                                "gradient_spread": found.gradient_spread,
                                "piece_length_m": found.piece_length_m,
                                "exact_db": found.exact_db,
                                "boxed_db": found.boxed_db,
                                "exact_altitude_m": found.exact_altitude_m,
                                "exact_scaled_distance_m": found.exact_scaled_distance_m,
                                "boxed_scaled_distance_m": found.boxed_scaled_distance_m,
                                "exact_offset_m": found.exact_offset_m,
                                "exact_length_m": found.exact_length_m,
                            }))
                            .collect::<Vec<_>>(),
                        "lists": report
                            .lists
                            .iter()
                            .map(|list| serde_json::json!({
                                "pieces": list.pieces,
                                "search_is_exhaustive": list.search_is_exhaustive,
                                "recall": list.recall,
                                "tolerant_recall": list.tolerant_recall,
                                "listed": list
                                    .listed
                                    .iter()
                                    .map(|(flight, sel, gap)| {
                                        serde_json::json!([format!("{flight:016x}"), sel, gap])
                                    })
                                    .collect::<Vec<_>>(),
                            }))
                            .collect::<Vec<_>>(),
                    })
                );
            }
            Ok(())
        }
        "traffic" => {
            let dev4 = Dev4 {
                prepared: options.get("prepared")?.into(),
                rasters: PathBuf::new(),
            };
            let started = std::time::Instant::now();
            let written = traffic::build(&dev4, &parse_squares(options.get("squares")?)?, &out)?;
            eprintln!(
                "traffic: {written} squares in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        "airport-traffic" => {
            let dev4 = Dev4 {
                prepared: options.get("prepared")?.into(),
                rasters: PathBuf::new(),
            };
            let written = airport::build(
                &dev4,
                Path::new(options.get("segments")?),
                &sampling_window(&options),
                &parse_squares(options.get("squares")?)?,
                &out,
            )?;
            eprintln!("airport traffic: {written} squares with traffic");
            Ok(())
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-build: {error}");
        std::process::exit(1);
    }
}
