//! Schedule aircraft boxes and event tiles with bounded memory and progress markers.
use crate::{boxes, cli::*, dev4::Square, year};
use physics::weather::WeatherTable;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(crate) fn run(
    command: &str,
    options: &Arguments,
    inputs: &year::YearInputs,
    out: &Path,
) -> Result<(), String> {
    match command {
        "aircraft-boxes" => {
            let rules = box_rules(options)?;
            let shuffled = inputs.required(crate::year::Kind::Shuffled)?;
            let days = boxes::shuffle::window_days(shuffled, &inputs.manifest.window())?;
            let squares = parse_squares(options.get("squares")?)?;
            boxes::shuffle::holds(shuffled, &squares)?;
            let weather = WeatherTable::read(inputs.required(crate::year::Kind::Weather)?)?;
            let terrain = options.terrain(inputs, out)?;
            let progress = progress_markers(options)?;
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
            boxes::run::build_squares(jobs, memory_budget(options)?, |square| {
                let started = std::time::Instant::now();
                let rules = to_build(square);
                let written = boxes::build_square(
                    (shuffled, &days),
                    (square, &weather),
                    terrain,
                    &rules,
                    out,
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
            let terrain = options.terrain(inputs, out)?;
            let progress = progress_markers(options)?;
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
            boxes::run::build_squares(jobs, memory_budget(options)?, |square| {
                let started = std::time::Instant::now();
                let written = boxes::events::build_square(
                    (shuffled, &days),
                    (square, &weather),
                    terrain,
                    out,
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
        other => Err(format!("unknown aircraft command {other}")),
    }
}
