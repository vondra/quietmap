//! Average bare-earth models, transform each node to EGM2008, and feather coverage edges.
use super::{blend, config::Band, datum, distance, source::Source, warp};
use tiles::terrain::Window;

pub fn expanded(window: Window, padding: u32) -> Window {
    Window {
        north_node: window.north_node + padding as i32,
        west_node: window.west_node - padding as i32,
        rows: window.rows + 2 * padding,
        columns: window.columns + 2 * padding,
    }
}

pub fn build(sources: &[Source], window: Window) -> Result<Vec<u8>, String> {
    let padding = sources
        .iter()
        .filter(|s| s.spec.band == Band::GroundHeight)
        .filter(|s| s.intersects(expanded(window, s.spec.feather_nodes + 2)))
        .map(|s| s.spec.feather_nodes)
        .max()
        .unwrap_or(0);
    let padding = if padding == 0 { 0 } else { padding + 2 };
    let mut work = expanded(window, padding);
    let mut fine = Vec::new();
    for source in sources
        .iter()
        .filter(|s| s.spec.feather_nodes > 0 && s.intersects(work))
    {
        let mut sampled = warp::read(
            &source.dataset,
            work,
            &source.spec.horizontal_crs,
            (source.spec.nodata, f64::NAN),
            "average",
            "Float64",
            true,
        )?;
        datum::convert(&mut sampled, work, source)?;
        if sampled.iter().any(|h| h.is_finite()) {
            fine.push((source.spec.input.as_str(), sampled));
        }
    }
    // A national mosaic's rectangle can contain large empty areas. Padding those areas
    // changes GDAL's floating-point averaging grid even though no fine model contributes.
    let padding = if fine.is_empty() { 0 } else { padding };
    work = expanded(window, padding);
    // The global producer fills open water left by the world models at sea level.
    let mut heights = vec![0.0; work.node_count()];
    for source in sources.iter().filter(|s| s.spec.band == Band::GroundHeight) {
        if !source.intersects(work) {
            continue;
        }
        let sampled = if source.spec.feather_nodes > 0 {
            let Some(index) = fine.iter().position(|(id, _)| *id == source.spec.input) else {
                continue;
            };
            fine.swap_remove(index).1
        } else {
            let mut sampled = warp::read(
                &source.dataset,
                work,
                &source.spec.horizontal_crs,
                (source.spec.nodata, f64::NAN),
                "average",
                "Float64",
                true,
            )?;
            datum::convert(&mut sampled, work, source)?;
            sampled
        };
        let halo = source.spec.feather_nodes;
        if halo > 0 {
            let distance = distance::to_missing(
                &sampled.iter().map(|h| h.is_finite()).collect::<Vec<_>>(),
                work.rows as usize,
                work.columns as usize,
            );
            blend::apply(
                &mut heights,
                &sampled,
                &distance,
                work.rows as usize,
                work.columns as usize,
                halo,
            );
        } else {
            for (height, sampled) in heights.iter_mut().zip(sampled) {
                if sampled.is_finite() {
                    *height = sampled;
                }
            }
        }
    }
    let mut result = Vec::with_capacity(window.node_count() * 2);
    for row in 0..window.rows as usize {
        let begin = (row + padding as usize) * work.columns as usize + padding as usize;
        for height in &heights[begin..begin + window.columns as usize] {
            let code = (*height + 500.0) * 5.0;
            if !code.is_finite() || !(0.0..=65534.0).contains(&code) {
                return Err("DEM outside native encoding range".into());
            }
            result.extend_from_slice(&((code + 0.5).floor() as u16).to_le_bytes());
        }
    }
    Ok(result)
}
