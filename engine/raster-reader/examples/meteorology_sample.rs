//! Inspect receiver climatology from one square's window file.
use grid::Square;
use raster_reader::meteorology::Meteorology;

fn main() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 5 {
        return Err("usage: meteorology_sample RASTERS_ROOT X Y LATITUDE LONGITUDE".into());
    }
    let square = Square {
        x: arguments[1].parse().map_err(|_| "invalid square x")?,
        y: arguments[2].parse().map_err(|_| "invalid square y")?,
    };
    let table = Meteorology::load(
        &Meteorology::path(std::path::Path::new(&arguments[0]), square),
        square,
    )?;
    let lat: f64 = arguments[3].parse().map_err(|_| "invalid latitude")?;
    let lon: f64 = arguments[4].parse().map_err(|_| "invalid longitude")?;
    let sample = table.at(lat, lon)?;
    println!(
        "{}",
        serde_json::json!({"p": sample.p, "alpha_mean": sample.alpha_mean,
        "alpha_variance": sample.alpha_variance, "window_p_max": table.maximum_probability()})
    );
    Ok(())
}
