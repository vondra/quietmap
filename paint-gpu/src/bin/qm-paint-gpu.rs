//! GPU physics checks, sparse popup references, exact etalons and cell-by-cell drift comparisons.
//!
//! `qm-paint-gpu check --prepared DIR --year YYYY --square X,Y [--pairs N] [--seed S] [--device D]
//! [--group G] [--near M] [--reach M] [--kind point|line]`:
//! the GPU's energy of random (receiver, source) pairs in one square against the CPU's (the
//! popup's `received_bands` at the painter's outdoor point), half the sources within `--near`
//! metres (300), half anywhere within `--reach` (the ground reach), only points or only lines with
//! `--kind`; prints the agreement, the failures, the worst pairs and both times.
//!
//! `qm-paint-gpu popup ... --square X,Y --out DIR [--pixels N] [--seed S]`: the popup's loudness
//! (Nden per layer and in total) at N random pixels of the square (300), a building's at its
//! loudest façade as a click inside it is answered, written as a sparse reference under DIR (every
//! other cell, and a building without an exposed façade, not assessed), to score a map or an etalon
//! against with `compare`.
//!
//! `qm-paint-gpu etalon ... --square X,Y --out DIR [--device D] [--sample-step K]`: the square's
//! etalon (every outdoor pixel's loudness with every loud source exact, `paint::etalon`; with K one
//! drawn in each K x K cell, the rest not assessed) written as z13 tiles under DIR.
//!
//! `qm-paint-gpu check-loudness ... --square X,Y [--pixels N] [--seed S] [--device D]`: the card's
//! loudness (`paint_gpu::loudness`) of N random outdoor pixels' sounds (`paint::etalon::sounds_at`,
//! 500) against the cores' (`PixelSound::ndens`): the differences in phon per layer and in total,
//! the worst, and both times.
//!
//! `qm-paint-gpu check-flights ... --square X,Y [--device D]`: the card's aircraft sums at the
//! square's hum lattice (far and distant boxes) and every second block corner (near boxes),
//! `paint_gpu::aircraft`, against the cores' (`paint::flight_boxes::FlightBoxes::sums`): the
//! differences in dB per band, kind and period, the worst, and both times.
//!
//! `qm-paint-gpu compare ... --painted DIR --reference DIR --squares X,Y;...`: a painted map against
//! a reference (an etalon, or the popup's sparse answers) cell by cell where the reference assesses
//! one, per layer and the total by the owner's z13 contract in loudness level (rungs over 0.5, 1, 3
//! and 6 phon, presence, bias, the quiet band; a breached limit marked `!`), and the worst cell.

use paint::exact::{LAYERS, PairEnergy, Point, whole_energy};
use paint::levels::{NO_LEVEL, SILENCE};
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

#[path = "checks/compare.rs"]
mod compare;
#[path = "checks/flights.rs"]
mod flights;
#[path = "checks/loudness.rs"]
mod loudness;
#[path = "checks/pairs.rs"]
mod pairs;
#[path = "checks/popup.rs"]
mod reference;

#[path = "checks/etalon.rs"]
mod etalon;

mod common;
use common::option;

/// A numeric option with its default when absent.
pub fn number(arguments: &[String], name: &str, default: u64) -> Result<u64, String> {
    option(arguments, name).map_or(Ok(default), |value| {
        value.parse().map_err(|_| format!("--{name}: not a number"))
    })
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
        Some("check" | "check-loudness" | "check-flights" | "popup" | "etalon" | "compare")
    ) {
        return Err("usage: qm-paint-gpu check|check-loudness|check-flights|popup|etalon|compare --prepared DIR --year YYYY ... (--square X,Y; compare: --painted DIR --reference DIR --squares X,Y;...)".into());
    }
    let required = |name: &str| option(arguments, name).ok_or(format!("--{name} is required"));
    let parse = |name, default| number(arguments, name, default);
    let release = Release::open(&PathBuf::from(required("prepared")?), required("year")?)?;
    if command == Some("compare") {
        return compare::compare_maps(
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
        return reference::popup_reference(
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
    let started = Instant::now();
    let files = Files::read(&release, tile)?;
    let square = Square::new(&weather, tile, &files)?;
    if command == Some("check-flights") {
        return flights::check_flights(&square, parse("device", 0)? as usize);
    }
    if command == Some("check-loudness") {
        return loudness::check_loudness(
            &square,
            parse("pixels", 500)? as usize,
            parse("seed", 1)?,
            parse("device", 0)? as usize,
        );
    }
    if command == Some("etalon") {
        return etalon::run(arguments, &square, &files, &weather);
    }
    eprintln!(
        "square {}/{}: {} candidates, read and built in {:.1} s",
        tile.x,
        tile.y,
        square.candidates.len(),
        started.elapsed().as_secs_f64()
    );
    pairs::check(
        arguments,
        &square,
        &files,
        &weather,
        parse("device", 0)? as usize,
        (pairs, parse("seed", 1)?),
    )
}
