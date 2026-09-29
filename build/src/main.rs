//! `qm-build`: builds the prepared z12 tiles of a release, one builder per kind.
//!
//! Until the builders read the sources themselves, `qm-build dev4` converts squares of the dev4
//! z9 tree: `qm-build dev4 --prepared DIR --rasters DIR --out DIR --squares X:Y[,X:Y..]
//! [--kinds terrain,obstacles,sources]`; `qm-build weather --rasters DIR --out FILE` cuts the global
//! weather table; `qm-build complete --out DIR --note TEXT` writes the completion marker last.

mod dev4;
mod low_profile;
mod obstacles;
mod output;
mod screening;
mod sources;
mod structures;
mod terrain;
mod weather;

use dev4::{Dev4, Square};
use std::path::PathBuf;

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

fn parse_squares(text: &str) -> Result<Vec<Square>, String> {
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

fn run(arguments: &[String]) -> Result<(), String> {
    let (command, rest) = arguments
        .split_first()
        .ok_or("usage: qm-build dev4|weather|complete --key value ..")?;
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
                    "terrain" => terrain::build(&dev4, &squares, &out)?,
                    "obstacles" => obstacles::build(&dev4, &squares, &out)?,
                    "sources" => sources::build(&dev4, &squares, &out)?,
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
