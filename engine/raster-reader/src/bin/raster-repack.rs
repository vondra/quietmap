//! Plan or publish native-byte z9 windows from source-derived coverage supplied by the raster driver.

use grid::{raster::RasterWindow, Square};
use raster_reader::channel::Channel;
use raster_reader::repack::{window_touches, NativeSources, SourceKey};
use serde::Deserialize;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    channel: String,
    tiles: Vec<SourceKey>,
    unknown: Vec<SourceKey>,
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 || !["plan", "publish"].contains(&args[0].as_str()) {
        return Err("usage: raster-repack plan|publish NATIVE_SOURCE_DIR PREPARED_ROOT < source-coverage.json".into());
    }
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| error.to_string())?;
    let coverage: Coverage = serde_json::from_str(&input).map_err(|error| error.to_string())?;
    let channel = Channel::ALL
        .into_iter()
        .find(|channel| channel.name() == coverage.channel)
        .ok_or("invalid channel")?;
    let expected: HashSet<_> = coverage.tiles.iter().copied().collect();
    let unknown: HashSet<_> = coverage.unknown.iter().copied().collect();
    if expected.is_empty()
        || expected.len() != coverage.tiles.len()
        || unknown.len() != coverage.unknown.len()
        || !expected.is_disjoint(&unknown)
        || expected
            .iter()
            .chain(&unknown)
            .any(|&(lat, lon)| !(-90..90).contains(&lat) || !(-180..180).contains(&lon))
    {
        return Err("invalid or duplicate native coverage".into());
    }
    let mut files = 0;
    let mut oceans = 0;
    let mut unavailable = 0;
    let mut payload_bytes = 0_u64;
    let mut allocated_bytes = 0_u64;
    let mut maximum_window_bytes = 0;
    let mut squares = Vec::new();
    for y in 0..512 {
        for x in 0..512 {
            let square = Square { x, y };
            let window = RasterWindow::for_square(square);
            if window_touches(window, &unknown) {
                unavailable += 1;
                continue;
            }
            if window_touches(window, &expected) {
                let bytes = channel.byte_len(window) as u64;
                payload_bytes += bytes;
                allocated_bytes += bytes.div_ceil(4096) * 4096;
                maximum_window_bytes = maximum_window_bytes.max(bytes);
                files += 1;
            } else {
                oceans += 1;
            }
            squares.push(square);
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "channel": coverage.channel, "source_tiles": expected.len(), "unknown_source_tiles": unknown.len(),
            "files": files, "ocean_squares": oceans, "unavailable_squares": unavailable,
            "payload_bytes": payload_bytes, "allocated_bytes_4096": allocated_bytes,
            "maximum_window_bytes": maximum_window_bytes,
        })
    );
    if args[0] == "plan" {
        return Ok(());
    }
    // Unknown land is never silently published as ocean. DEM-only publication is independent.
    if unavailable != 0 {
        return Err(format!(
            "{unavailable} z9 windows require missing land coverage"
        ));
    }
    let mut sources = NativeSources::new(Path::new(&args[1]), channel, expected, unknown)?;
    let root = Path::new(&args[2]);
    for (index, square) in squares.into_iter().enumerate() {
        sources.publish_square(root, square)?;
        if (index + 1) % 1000 == 0 {
            eprintln!("published {} squares", index + 1);
        }
    }
    println!("complete {} native z9 channel", channel.name());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("raster-repack: {error}");
        std::process::exit(1);
    }
}
