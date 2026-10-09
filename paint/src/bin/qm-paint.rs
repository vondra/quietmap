//! `qm-paint --prepared DIR --year YYYY --zoom 12|13 --out DIR (--squares X,Y[;X,Y...] | --bbox
//! SOUTH,WEST,NORTH,EAST)`: paints the z12 squares one after another (each on every core) into
//! heatmap tiles at `--zoom`, skipping squares already painted; one line per square on stdout.
//! `qm-paint pack --out DIR --zoom Z --tiles DIR --build bNNN`: the map's archives and manifest.

use paint::hm3::{painted, write};
use paint::paint::{Grid, paint};
use paint::square::{Files, Square};
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
    let grid = match zoom {
        12 => Grid {
            pixels: 512,
            block: 8,
            coarse: 64,
        },
        13 => Grid {
            pixels: 1024,
            block: 16,
            coarse: 128,
        },
        _ => return Err("--zoom is 12 or 13".into()),
    };
    let squares = squares(arguments)?;
    let mut stdout = std::io::stdout().lock();
    for (number, tile) in squares.iter().enumerate() {
        if painted(&out, zoom, (tile.x, tile.y)) {
            continue;
        }
        let started = Instant::now();
        let files = Files::read(&release, *tile)?;
        let read_s = started.elapsed().as_secs_f64();
        let square = Square::new(&weather, *tile, &files)?;
        let built_s = started.elapsed().as_secs_f64() - read_s;
        let cells = paint(&square, grid)?;
        let painted_s = started.elapsed().as_secs_f64() - read_s - built_s;
        write(&out, zoom, (tile.x, tile.y), &cells)?;
        writeln!(
            stdout,
            "qm-paint: square {}/{} ({} of {}): {:.0} MB, {} sources, read {read_s:.1} s, built {built_s:.1} s, painted {painted_s:.1} s",
            tile.x,
            tile.y,
            number + 1,
            squares.len(),
            files.bytes() as f64 / 1e6,
            square.candidates.len(),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("qm-paint: {error}");
        std::process::exit(1);
    }
}
