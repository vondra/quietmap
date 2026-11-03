//! Builds one manifest's inputs into the popup and painter's files.
//! `qm-build COMMAND --year years/YEAR.json --sources ROOT --out OUTPUT`.
//! Spatial steps take `--squares X:Y,X:Y` (or `all`, or `@FILE`); `dev4` also takes
//! `--kinds terrain,obstacles,sources`. The source inventory and flow dates live only in the
//! manifest. Completion verifies the required model inputs and binds the output to its manifest.

#[path = "../../fetch/structure_inputs.rs"]
mod structure_inputs;

mod aircraft;
mod airport;
mod barriers;
mod boxes;
mod climate;
mod dev4;
mod low_profile;
mod obstacles;
mod osm;
#[path = "../../fetch/osm_regions.rs"]
mod osm_regions;
mod period;
mod rasters;
mod rings;
#[path = "../../fetch/road_counts.rs"]
mod road_counts;
mod roads;
mod screening;
mod source_outlines;
mod source_tables;
mod sources;
mod stage1;
mod structures;
mod terrain;
mod terrain_fine;
mod traffic;
mod weather;
mod year;

use dev4::{Dev4, Square};
use physics::weather::WeatherTable;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
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
                    let key = &key[2..];
                    if ![
                        "year",
                        "sources",
                        "out",
                        "squares",
                        "kinds",
                        "kind",
                        "threads",
                        "boxes",
                        "halo",
                        "memory-gb",
                        "progress",
                        "level-step-db",
                        "pieces",
                        "points",
                        "only",
                        "diagnose",
                        "note",
                        "scratch",
                    ]
                    .contains(&key)
                    {
                        return Err(format!(
                            "unknown option --{key}; input files and flow dates belong in --year"
                        ));
                    }
                    if values.iter().any(|(name, _)| name == key) {
                        return Err(format!("repeated option --{key}"));
                    }
                    values.push((key.to_string(), value.clone()))
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

    /// The terrain input, the year root a builder reads the ground from: the tree it builds (`--out`,
    /// not complete yet) or a complete one of this layout; one of another layout would read as
    /// all ocean.
    fn terrain<'a>(&self, inputs: &'a year::YearInputs, out: &Path) -> Result<&'a Path, String> {
        let terrain = inputs.required(crate::year::Kind::Terrain)?;
        if terrain != out {
            inputs.check_complete(terrain)?;
        }
        Ok(terrain)
    }
}

/// The squares of `--squares`: `X:Y,X:Y,..`, `all` (every z9 square of the world) or `@FILE`
/// (the same pairs, by commas or lines: a world's list is too long for one argument).
fn parse_squares(text: &str) -> Result<Vec<Square>, String> {
    if text == "all" {
        return Ok((0..dev4::SQUARES_PER_AXIS)
            .flat_map(|x| (0..dev4::SQUARES_PER_AXIS).map(move |y| Square { x, y }))
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

/// Every national terrain model admitted by the manifest, in its declared precedence.
fn national_models(
    inputs: &year::YearInputs,
) -> Result<Vec<terrain::national::NationalHeights>, String> {
    inputs
        .national_dem_paths()?
        .iter()
        .map(|path| terrain::national::NationalHeights::read(path))
        .collect()
}

/// The aircraft box rule: `--kind aircraft|aircraft-far`, `--level-step-db` (D) and `--pieces`
/// (kept per box), else the kind's defaults.
/// The box rules of `--kind` (a list, `aircraft` by default), with `--level-step-db` and
/// `--pieces` laid over each.
fn box_rules(options: &Arguments) -> Result<Vec<boxes::BoxRule>, String> {
    options
        .optional("kind")
        .unwrap_or("aircraft")
        .split(',')
        .map(|kind| {
            let mut rule = match kind {
                "aircraft" => boxes::BoxRule::default(),
                "aircraft-far" => boxes::BoxRule::far(),
                other => return Err(format!("bad --kind {other}")),
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
        })
        .collect()
}

/// `--memory-gb`: the memory the squares built at once may take by their estimates (GB, 128 by
/// default; the estimates are bounds: the heaviest squares took about half of theirs, the rest a
/// third or less, 2026-10-16).
fn memory_budget(options: &Arguments) -> Result<u64, String> {
    let gigabytes: u64 = options
        .optional("memory-gb")
        .unwrap_or("128")
        .parse()
        .map_err(|_| "bad --memory-gb".to_string())?;
    Ok(gigabytes << 30)
}

/// `--progress DIR`: a marker per square and kind built, so that a rerun resumes.
fn progress_markers(options: &Arguments) -> Result<Option<PathBuf>, String> {
    let progress = options.optional("progress").map(PathBuf::from);
    if let Some(progress) = &progress {
        std::fs::create_dir_all(progress).map_err(|error| error.to_string())?;
    }
    Ok(progress)
}

/// An events table as JSON: per band its threshold, flights a day by period, mean height (m) and
/// most frequent type; helicopters a day in the lowest band.
fn events_json(events: &boxes::events::EventCounts) -> serde_json::Value {
    let bands: Vec<serde_json::Value> = boxes::events::EVENT_BANDS_DB
        .iter()
        .enumerate()
        .map(|(band, above_db)| {
            serde_json::json!({
                "above_db": above_db,
                "per_day": events.per_day[band],
                "height_m": events.mean_height_m(band),
                "type": events
                    .top_designator(band)
                    .map(|designator| String::from_utf8_lossy(&designator).trim().to_string()),
            })
        })
        .collect();
    serde_json::json!({"bands": bands, "helicopters_per_day": events.helicopters})
}

fn run(arguments: &[String]) -> Result<(), String> {
    let (command, rest) = arguments.split_first().ok_or(
        "usage: qm-build COMMAND --year MANIFEST --sources ROOT --out OUTPUT [--key value ...]",
    )?;
    let options = Arguments::parse(rest)?;
    let out = PathBuf::from(options.get("out")?);
    if command == "manifest" {
        let path = Path::new(options.get("year")?);
        let manifest = year::Manifest::read(path)?;
        let mut projection: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let admitted = manifest
            .inputs
            .iter()
            .filter_map(|input| match manifest.admitted(&input.id) {
                Ok(true) => Some(Ok(input.id.as_str())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<HashSet<_>, _>>()?;
        projection["inputs"]
            .as_array_mut()
            .ok_or("manifest inputs must be an array")?
            .retain(|input| input["id"].as_str().is_some_and(|id| admitted.contains(id)));
        let json = serde_json::to_string(&projection).map_err(|e| e.to_string())?;
        if out == Path::new("-") {
            println!("{json}");
            return Ok(());
        }
        return std::fs::write(out, json).map_err(|e| e.to_string());
    }
    if command == "complete" {
        let path = Path::new(options.get("year")?);
        let inputs = year::YearInputs::read(path, Path::new(options.get("sources")?))?;
        source_tables::SourceTables::read(&inputs)?;
        inputs.check_published_tables()?;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return tiles::mark_year_complete(&out, &bytes, options.get("note")?);
    }
    let inputs = year::YearInputs::read(
        Path::new(options.get("year")?),
        Path::new(options.get("sources")?),
    )?;
    match command.as_str() {
        "road-observations" => road_counts::convert(&inputs, &out).map_err(|e| e.to_string()),
        "roads" => roads::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
            .map_err(|e| e.to_string()),
        "structure-height-model" => structures::height_calibration::fit(
            &inputs,
            &parse_squares(options.get("squares")?)?,
            &out,
        )
        .map_err(|e| e.to_string()),
        "structure-observations" => {
            structure_inputs::convert(&inputs, &parse_squares(options.get("squares")?)?, &out)
                .map_err(|e| e.to_string())
        }
        "structures" => {
            structures::prepare::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
                .map_err(|e| e.to_string())
        }
        "osm-regions" => osm_regions::convert(&inputs, &out).map_err(|error| error.to_string()),
        "osm" => osm::build(
            &inputs,
            &parse_squares(options.get("squares")?)?,
            &out,
            Path::new(options.get("scratch")?),
        )
        .map_err(|error| error.to_string()),
        "prepare-rasters" => {
            rasters::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
        }
        "source-outlines" => source_outlines::build(
            &Dev4 {
                prepared: inputs.required(crate::year::Kind::Prepared)?.into(),
                rasters: PathBuf::new(),
            },
            &parse_squares(options.get("squares")?)?,
            &out,
        ),
        "dev4" => {
            let dev4 = Dev4 {
                prepared: inputs.required(crate::year::Kind::Prepared)?.into(),
                rasters: inputs.required(crate::year::Kind::Rasters)?.into(),
            };
            let squares = parse_squares(options.get("squares")?)?;
            let kinds = options.optional("kinds").unwrap_or("terrain");
            for kind in kinds.split(',') {
                let started = std::time::Instant::now();
                let written = match kind {
                    "terrain" => terrain::build(&dev4, &squares, &national_models(&inputs)?, &out)?,
                    "obstacles" => obstacles::build(
                        &dev4,
                        &barriers::Barriers::load(inputs.required(crate::year::Kind::Barriers)?)?,
                        &squares,
                        &out,
                    )?,
                    "sources" => sources::build(
                        (
                            &dev4,
                            &climate::Climate::load(inputs.required(crate::year::Kind::Climate)?)?,
                            &sources::tent::FreightNetwork::load(
                                inputs.required(crate::year::Kind::Tent)?,
                            )?,
                            &sources::turbines::Turbines::load(
                                inputs.required(crate::year::Kind::Turbines)?,
                            )?,
                            &sources::water::Water::open(
                                inputs.required(crate::year::Kind::Water)?,
                            )?,
                            &source_tables::SourceTables::read(&inputs)?,
                        ),
                        (
                            inputs.required(crate::year::Kind::AirportTraffic)?,
                            Some(inputs.required(crate::year::Kind::RoadTraffic)?),
                            Some(&sources::bus::BusRoutes::load(
                                inputs.required(crate::year::Kind::Bus)?,
                            )?),
                            sources::Places {
                                worship: Some(&sources::worship::WorshipSites::load(
                                    inputs.required(crate::year::Kind::Worship)?,
                                )?),
                                venues: Some(&sources::venues::Venues::load(
                                    inputs.required(crate::year::Kind::Venues)?,
                                )?),
                            },
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
                rasters: inputs.required(crate::year::Kind::Rasters)?.into(),
            };
            weather::build(&dev4, &out)
        }
        "terrain-fine" => {
            let started = std::time::Instant::now();
            let models: Vec<&Path> = inputs
                .paths(crate::year::Kind::FineDem)?
                .iter()
                .map(PathBuf::as_path)
                .collect();
            let written = terrain_fine::build(
                &parse_squares(options.get("squares")?)?,
                &models,
                options.terrain(&inputs, &out)?,
                &out,
            )?;
            eprintln!(
                "terrain-fine: {written} tiles in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        "geoid" => aircraft::geoid::build(inputs.required(crate::year::Kind::Tiff)?, &out),
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
            let window = inputs.manifest.window();
            let days = aircraft::Days::listed(&window.baseline_days, &window.increment_days)?;
            aircraft::run(aircraft::Run {
                days,
                primary: inputs.required(crate::year::Kind::Primary)?,
                secondary: if window.increment_days.is_empty() {
                    None
                } else {
                    Some(inputs.required(crate::year::Kind::Secondary)?)
                },
                rasters: inputs.required(crate::year::Kind::Rasters)?,
                geoid: inputs.required(crate::year::Kind::Geoid)?,
                boxes: options.optional("boxes"),
                out: &out,
                national: national_models(&inputs)?,
            })
        }
        "aircraft-shuffle" => {
            let window = inputs.manifest.window();
            let mut squares = parse_squares(options.get("squares")?)?;
            // `--halo`: with the squares the events of these read (within their reach).
            if options.optional("halo").is_some() {
                squares = boxes::events::halo(&squares);
            }
            let scope = squares.into_iter().collect();
            let left = boxes::shuffle::shuffle(
                inputs.required(crate::year::Kind::Segments)?,
                &window,
                &scope,
                &out,
            )?;
            eprintln!("aircraft shuffle: {left} days not written yet");
            Ok(())
        }
        "aircraft-boxes" => {
            let rules = box_rules(&options)?;
            let shuffled = inputs.required(crate::year::Kind::Shuffled)?;
            let days = boxes::shuffle::window_days(shuffled, &inputs.manifest.window())?;
            let squares = parse_squares(options.get("squares")?)?;
            boxes::shuffle::holds(shuffled, &squares)?;
            let weather = WeatherTable::read(inputs.required(crate::year::Kind::Weather)?)?;
            let terrain = options.terrain(&inputs, &out)?;
            let progress = progress_markers(&options)?;
            eprintln!("aircraft boxes: {} days", days.len());
            let marker = |rule: &boxes::BoxRule, square: Square| {
                progress.as_ref().map(|progress| {
                    progress.join(format!("{}-{}-{}", rule.kind.name(), square.x, square.y))
                })
            };
            let to_build = |square: Square| -> Vec<boxes::BoxRule> {
                rules
                    .iter()
                    .filter(|rule| !marker(rule, square).is_some_and(|marker| marker.exists()))
                    .copied()
                    .collect()
            };
            let jobs: Vec<boxes::run::Job> = squares
                .par_iter()
                .filter(|&&square| !to_build(square).is_empty())
                .map(|&square| {
                    let bytes = boxes::shuffle::square_bytes(shuffled, square);
                    // A box takes about 900 bytes. A square's fine boxes stayed under 1,414 times
                    // its day files' bytes to the power 0.4 at 7,323 squares of the Americas (the
                    // most, 8.5 million in 20.9 GB, 2.2 times under), its far boxes under 0.3 times
                    // its fine (2026-10-16); each kind to build counts.
                    let fine = 1_414.0 * (bytes as f64).powf(0.4);
                    let boxes: f64 = to_build(square)
                        .iter()
                        .map(|rule| match rule.kind {
                            tiles::Kind::AircraftFar => 0.3 * fine,
                            _ => fine,
                        })
                        .sum();
                    boxes::run::Job {
                        square,
                        work: bytes,
                        memory: (boxes * 900.0) as u64 + (64 << 20),
                    }
                })
                .collect();
            boxes::run::build_squares(jobs, memory_budget(&options)?, |square| {
                let started = std::time::Instant::now();
                let rules = to_build(square);
                let written = boxes::build_square(
                    (shuffled, &days),
                    (square, &weather),
                    terrain,
                    &rules,
                    &out,
                )?;
                let mut line = format!("aircraft boxes: square {}/{}:", square.x, square.y);
                for (rule, (tiles, boxes)) in rules.iter().zip(written) {
                    line += &format!(" {} {boxes} boxes in {tiles} tiles,", rule.kind.name());
                    if let Some(marker) = marker(rule, square) {
                        std::fs::write(marker, b"").map_err(|error| error.to_string())?;
                    }
                }
                Ok(format!("{line} {:.1} s", started.elapsed().as_secs_f64()))
            })
        }
        "aircraft-events" => {
            let shuffled = inputs.required(crate::year::Kind::Shuffled)?;
            let days = boxes::shuffle::window_days(shuffled, &inputs.manifest.window())?;
            let squares = parse_squares(options.get("squares")?)?;
            boxes::shuffle::holds(shuffled, &boxes::events::halo(&squares))?;
            let weather = WeatherTable::read(inputs.required(crate::year::Kind::Weather)?)?;
            let terrain = options.terrain(&inputs, &out)?;
            let progress = progress_markers(&options)?;
            let marker = |square: Square| {
                progress
                    .as_ref()
                    .map(|progress| progress.join(format!("events-{}-{}", square.x, square.y)))
            };
            let todo: Vec<Square> = squares
                .into_iter()
                .filter(|&square| !marker(square).is_some_and(|marker| marker.exists()))
                .collect();
            let inputs: HashSet<Square> = boxes::events::halo(&todo).into_iter().collect();
            let bytes: HashMap<Square, u64> = inputs
                .par_iter()
                .map(|&square| (square, boxes::shuffle::square_bytes(shuffled, square)))
                .collect();
            let jobs: Vec<boxes::run::Job> = todo
                .iter()
                .map(|&square| {
                    let work: u64 = boxes::events::squares_within_reach(square)
                        .iter()
                        .map(|input| bytes.get(input).copied().unwrap_or(0))
                        .sum();
                    // Two days of the inputs decoded (152 bytes a segment of 80 in the day files)
                    // and each flight's segments copied once more, and the square's counts.
                    boxes::run::Job {
                        square,
                        work,
                        memory: 6 * work / days.len().max(1) as u64 + (64 << 20),
                    }
                })
                .collect();
            boxes::run::build_squares(jobs, memory_budget(&options)?, |square| {
                let started = std::time::Instant::now();
                let written = boxes::events::build_square(
                    (shuffled, &days),
                    (square, &weather),
                    terrain,
                    &out,
                )?;
                if let Some(marker) = marker(square) {
                    std::fs::write(marker, b"").map_err(|error| error.to_string())?;
                }
                Ok(format!(
                    "aircraft events: square {}/{}: {written} tiles, {:.1} s",
                    square.x,
                    square.y,
                    started.elapsed().as_secs_f64()
                ))
            })
        }
        "aircraft-tracks" => aircraft::tracks::build(
            &inputs
                .required(crate::year::Kind::Segments)?
                .join("segments"),
            &inputs.manifest.window(),
            &out,
        ),
        "aircraft-check" => {
            let window = inputs.manifest.window();
            let points: Vec<serde_json::Value> = serde_json::from_str(
                &std::fs::read_to_string(options.get("points")?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let only: Vec<&str> = options
                .optional("only")
                .map_or(Vec::new(), |names| names.split(',').collect());
            let terrain = options.terrain(&inputs, &out)?;
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
            let weather = WeatherTable::read(inputs.required(crate::year::Kind::Weather)?)?;
            let compared = boxes::check::compare(
                (inputs.required(crate::year::Kind::Segments)?, &weather),
                &window,
                (&out, box_rules(&options)?[0].level_step_db),
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
                        "exact_events": events_json(&report.events),
                        "written_events": report.events_written.bands.map(|band| {
                            serde_json::json!([band.per_day, band.night_per_day, band.height_m])
                        }),
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
                                "flights_per_day": found.flights_per_day,
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
                prepared: inputs.required(crate::year::Kind::Prepared)?.into(),
                rasters: PathBuf::new(),
            };
            let started = std::time::Instant::now();
            let written = traffic::build(
                &source_tables::SourceTables::read(&inputs)?,
                &dev4,
                &parse_squares(options.get("squares")?)?,
                &out,
            )?;
            eprintln!(
                "traffic: {written} squares in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        "airport-traffic" => {
            let dev4 = Dev4 {
                prepared: inputs.required(crate::year::Kind::Prepared)?.into(),
                rasters: PathBuf::new(),
            };
            let written = airport::build(
                &dev4,
                inputs.required(crate::year::Kind::Segments)?,
                &inputs.manifest.window(),
                &parse_squares(options.get("squares")?)?,
                (options.terrain(&inputs, &out)?, &out),
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
