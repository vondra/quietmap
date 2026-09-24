//! Inspect receiver climatology from a complete release table.
use raster_reader::meteorology::Meteorology;

fn main() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 3 {
        return Err("usage: meteorology_sample FILE LATITUDE LONGITUDE".into());
    }
    let table = Meteorology::load(std::path::Path::new(&arguments[0]))?;
    let lat: f64 = arguments[1].parse().map_err(|_| "invalid latitude")?;
    let lon: f64 = arguments[2].parse().map_err(|_| "invalid longitude")?;
    let sample = table.at(lat, lon)?;
    println!(
        "{}",
        serde_json::json!({"p": sample.p, "alpha_mean": sample.alpha_mean,
        "alpha_variance": sample.alpha_variance, "global_p_max":table.maximum_probability()})
    );
    Ok(())
}
