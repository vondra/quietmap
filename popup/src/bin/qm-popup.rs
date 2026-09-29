//! `qm-popup --prepared DIR --year YYYY --lat LAT --lon LON [--exact 1] [--pieces N]`: answers one
//! click and prints each streamed update as a line of JSON (the benchmark's and the server's
//! reference); `--pieces N` adds the N loudest evaluated pieces per layer to the final update.

use popup::answer::{Options, answer};
use popup::json::update_line;
use popup::release::Release;
use std::io::Write;
use std::path::PathBuf;

fn value<'a>(arguments: &'a [String], key: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == format!("--{key}"))
        .map(|pair| pair[1].as_str())
}

fn run(arguments: &[String]) -> Result<(), String> {
    let required = |key: &str| value(arguments, key).ok_or_else(|| format!("missing --{key}"));
    let number = |key: &str| {
        required(key)?
            .parse::<f64>()
            .map_err(|_| format!("--{key} is not a number"))
    };
    let release = Release::open(&PathBuf::from(required("prepared")?), required("year")?)?;
    let options = Options {
        exact: value(arguments, "exact") == Some("1"),
        pieces: value(arguments, "pieces")
            .map(|text| text.parse().map_err(|_| "--pieces is not a count"))
            .transpose()?
            .unwrap_or(0),
    };
    let (lat, lon) = (number("lat")?, number("lon")?);
    let mut stdout = std::io::stdout().lock();
    let mut sequence = 0;
    answer(&release, lat, lon, &options, &mut |update| {
        sequence += 1;
        writeln!(stdout, "{}", update_line(update, sequence)?)
            .map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())
    })
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-popup: {error}");
        std::process::exit(1);
    }
}
