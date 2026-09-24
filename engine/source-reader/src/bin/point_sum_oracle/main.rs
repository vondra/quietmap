//! `point-sum-oracle` — reference measurements for the line quadrature: every road and rail
//! piece evaluated by the production quadrature (5 in-plane buckets, each node on its own ray)
//! and by a fine CNOSSOS point sum (§2.5.3) whose nodes run the same production ray, on
//! synthetic line scenes and on real receivers of a prepared release. Prints one JSON document.
//!
//! ```text
//! point-sum-oracle lines [<max_angle_deg> <max_length_m>]
//! point-sum-oracle receivers <prepared-year-dir> <receivers.csv> [<max_angle_deg> <max_length_m>]
//! ```
//! `receivers.csv` rows are `name,lat,lon`. Threads follow `RAYON_NUM_THREADS`.
//!
//! Built without the `node` feature: that feature turns the library into an N-API addon whose
//! exports only link inside the Node host, so under it this binary is an empty stub.

#[cfg(not(feature = "node"))]
mod lines;
#[cfg(not(feature = "node"))]
mod piece;
#[cfg(not(feature = "node"))]
mod receivers;

#[cfg(not(feature = "node"))]
use noise_compute::propagation::point_sum::NodeSpacing;
#[cfg(not(feature = "node"))]
use std::path::Path;

#[cfg(feature = "node")]
fn main() {}

#[cfg(not(feature = "node"))]
fn spacing(args: &[String]) -> NodeSpacing {
    let arg = |i: usize, default: f64| args.get(i).map_or(default, |v| v.parse().expect("number"));
    NodeSpacing {
        max_angle_rad: arg(0, 1.0).to_radians(),
        max_length_m: arg(1, 10.0),
    }
}

#[cfg(not(feature = "node"))]
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("lines") => lines::run(spacing(&args[1..])),
        Some("receivers") => {
            let dir = args.get(1).ok_or("receivers needs <prepared-year-dir>")?;
            let csv = std::fs::read_to_string(args.get(2).ok_or("receivers needs <receivers.csv>")?)
                .map_err(|e| e.to_string())?;
            let points: Vec<(String, f64, f64)> = csv
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .map(|l| {
                    let f: Vec<&str> = l.split(',').map(str::trim).collect();
                    (f[0].to_owned(), f[1].parse().expect("lat"), f[2].parse().expect("lon"))
                })
                .collect();
            receivers::run(Path::new(dir), &points, spacing(&args[3..]))?
        }
        _ => return Err("usage: point-sum-oracle lines | receivers <dir> <csv>".into()),
    };
    println!("{}", serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?);
    Ok(())
}
