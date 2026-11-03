//! Canopy height is the area mean over positive native heights, with observed zero kept.
use super::{config::Band, source::Source, warp};
use tiles::terrain::Window;

fn average(source: &Source, window: Window) -> Result<Vec<f64>, String> {
    let mut result = vec![f64::NAN; window.node_count()];
    for start in (0..window.rows).step_by(16) {
        let count = 16.min(window.rows - start);
        let stripe = Window {
            north_node: window.north_node - start as i32,
            rows: count,
            ..window
        };
        let [west, south, east, north] = warp::bounds(stripe);
        let mut options: Vec<String> = ["-of", "VRT", "-projwin_srs", "EPSG:4326", "-projwin"]
            .map(str::to_string)
            .to_vec();
        options.extend([west, north, east, south].map(|v| v.to_string()));
        let projected = warp::translate(&source.dataset, &options)?;
        let original = source.dataset.geo_transform().map_err(|e| e.to_string())?;
        let clipped = projected.geo_transform().map_err(|e| e.to_string())?;
        let row_offset = ((clipped[3] - original[3]) / original[5]).round() as i64 - 2;
        let column_offset = ((clipped[0] - original[0]) / original[1]).round() as i64 - 2;
        let (width, height) = projected.raster_size();
        let (width, height) = (width + 4, height + 4);
        let mut options: Vec<String> = ["-of", "MEM", "-ot", "Float64", "-srcwin"]
            .map(str::to_string)
            .to_vec();
        options.extend([
            column_offset.to_string(),
            row_offset.to_string(),
            width.to_string(),
            height.to_string(),
        ]);
        let crop = warp::translate(&source.dataset, &options)?;
        let mut heights = warp::values(&crop)?;
        let (source_width, source_height) = source.dataset.raster_size();
        for (i, value) in heights.iter_mut().enumerate() {
            let row = row_offset + (i / width) as i64;
            let column = column_offset + (i % width) as i64;
            if !(0.0..=250.0).contains(value)
                || Some(*value) == source.spec.nodata
                || row < 0
                || column < 0
                || row >= source_height as i64
                || column >= source_width as i64
            {
                *value = f64::NAN;
            }
        }
        let transform = crop.geo_transform().map_err(|e| e.to_string())?;
        let coverage = warp::memory(
            width,
            height,
            transform,
            &source.spec.horizontal_crs,
            heights.clone(),
        )?;
        let coverage = warp::read(
            &coverage,
            stripe,
            &source.spec.horizontal_crs,
            (None, f64::NAN),
            "average",
            "Float64",
            true,
        )?;
        for value in &mut heights {
            if *value == 0.0 {
                *value = f64::NAN;
            }
        }
        let positive = warp::memory(
            width,
            height,
            transform,
            &source.spec.horizontal_crs,
            heights,
        )?;
        let positive = warp::read(
            &positive,
            stripe,
            &source.spec.horizontal_crs,
            (None, f64::NAN),
            "average",
            "Float64",
            true,
        )?;
        for (i, (cover, height)) in coverage.into_iter().zip(positive).enumerate() {
            result[start as usize * window.columns as usize + i] =
                if cover == 0.0 && !height.is_finite() {
                    0.0
                } else {
                    height
                };
        }
    }
    Ok(result)
}

pub fn build(sources: &[Source], window: Window) -> Result<Vec<u8>, String> {
    let mut values = vec![f64::NAN; window.node_count()];
    for source in sources.iter().filter(|s| s.spec.band == Band::TreeHeight) {
        if !source.intersects(window) {
            continue;
        }
        for (value, sampled) in values.iter_mut().zip(average(source, window)?) {
            if sampled.is_finite() {
                *value = sampled;
            }
        }
    }
    if values.iter().all(|v| !v.is_finite()) {
        return Ok(Vec::new());
    }
    Ok(values
        .into_iter()
        .map(|value| {
            if value.is_finite() {
                (value + 0.5).floor() as u8
            } else {
                0
            }
        })
        .collect())
}
