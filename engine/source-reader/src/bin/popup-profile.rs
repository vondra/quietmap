//! Run the popup's point computation outside Node — for `perf` on a host that
//! allows it, and for parity checks against the server's answer. Standalone
//! binary: `popup-profile <prepared_h3r4_dir> <lat> <lng> [repeats] [json_out]`.
use std::path::{Path, PathBuf};
use std::time::Instant;

use source_reader::hex_store::{load_hex, HexData};

fn usage() -> ! {
    eprintln!("Usage: popup-profile <prepared_h3r4_dir> <lat> <lng> [repeats] [json_out]");
    std::process::exit(2);
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args.len() > 6 {
        usage();
    }
    let h3r4_dir = PathBuf::from(&args[1]);
    let lat: f64 = args[2]
        .parse()
        .map_err(|_| format!("bad lat {}", args[2]))?;
    let lng: f64 = args[3]
        .parse()
        .map_err(|_| format!("bad lng {}", args[3]))?;
    let repeats: usize = match args.get(4) {
        Some(s) => s.parse().map_err(|_| format!("bad repeats {s}"))?,
        None => 1,
    };
    // Rasters and the index cache live two levels up (`data/prepared/…`) — the
    // same derivation `source_init` makes for the popup.
    let data_dir = h3r4_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("{} has no prepared root", h3r4_dir.display()))?;
    let rasters = raster_reader::RealRasters::new(data_dir);
    noise_compute::admin::set_admin_h3r4_directory(&h3r4_dir);

    let t = Instant::now();
    let hex_ids = source_reader::geo::grid_disk_r4(lat, lng);
    let hexes: Vec<HexData> = std::thread::scope(|scope| {
        let handles: Vec<_> = hex_ids
            .iter()
            .map(|id| {
                let dir = format!("{}/{id}", h3r4_dir.display());
                scope.spawn(move || {
                    load_hex(&dir).unwrap_or_else(|e| {
                        eprintln!("popup-profile: {id}: {e}");
                        HexData::empty()
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("hex load panicked"))
            .collect()
    });
    let refs: Vec<&HexData> = hexes.iter().collect();
    eprintln!(
        "popup-profile: {} hexes opened in {:.0} ms (DEM: {})",
        refs.len(),
        t.elapsed().as_secs_f64() * 1000.0,
        if rasters.has_data() { "loaded" } else { "stub" }
    );

    let mut last = None;
    for i in 1..=repeats {
        let t = Instant::now();
        let result = source_reader::popup::compute_point(
            &refs,
            lat,
            lng,
            source_reader::popup::SEGMENT_TOP_K_PER_KIND,
            &h3r4_dir,
            data_dir,
            &rasters,
        )?;
        eprintln!(
            "popup-profile: run {i}: {:.0} ms, total {:.2} dB, {} segments",
            t.elapsed().as_secs_f64() * 1000.0,
            result.total_lden,
            result.segments.len()
        );
        last = Some(result);
    }
    if let (Some(path), Some(result)) = (args.get(5), last) {
        let json = serde_json::to_string(&result).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}
