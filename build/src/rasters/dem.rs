//! Average bare-earth models, each on its own country's land, transform each node to EGM2008, and
//! feather coverage edges.
use super::{blend, config::Band, countries::Countries, datum, distance, source::Source, warp};
use tiles::terrain::Window;

pub fn expanded(window: Window, padding: u32) -> Window {
    Window {
        north_node: window.north_node + padding as i32,
        west_node: window.west_node - padding as i32,
        rows: window.rows + 2 * padding,
        columns: window.columns + 2 * padding,
    }
}

/// The band's code: 0.2 m steps from 500 m below the sea, so a u16 holds -500 to 12,606.8 m.
const CODE_FLOOR_M: f64 = -500.0;
const CODES_PER_M: f64 = 5.0;
const CODE_CEILING_M: f64 = CODE_FLOOR_M + 65534.0 / CODES_PER_M;

/// A height the code cannot hold is no ground: its floor lies below the lowest shore, the Dead Sea's
/// at 430 m below the sea, and its ceiling above the highest summit. CA-MRDEM-30 holds a row of -578
/// to -912 m across Lake Nipigon (50.242 N, 88.37 W) and one of -618 to -695 m off Vancouver Island
/// (50.647 N, 127.99 W), its tiles' seams. Such a node keeps the model beneath it but still counts as
/// its model's coverage, so the feather keeps its weights.
pub(super) fn is_ground(height: f64) -> bool {
    (CODE_FLOOR_M..=CODE_CEILING_M).contains(&height)
}

/// A national model's heights read on another country's land are its mosaic's fill, not its survey
/// (RGE ALTI holds zeros over Belgium and other heights over Switzerland and Spain): they go missing
/// before the datum and the blend, so that country's models and the world model hold there and the
/// border feathers as any other edge of the model. The sea and disputed land belong to no country.
fn keep_own_country(heights: &mut [f64], own: u16, labels: &[u16]) {
    for (height, &label) in heights.iter_mut().zip(labels) {
        if label != 0 && label != own {
            *height = f64::NAN;
        }
    }
}

/// Reads one source onto `work`, missing outside its country and converted to EGM2008; `labels`
/// keeps the country labels of the last window read.
fn read(
    source: &Source,
    work: Window,
    countries: &Countries,
    labels: &mut Option<(Window, Vec<u16>)>,
) -> Result<Vec<f64>, String> {
    let mut sampled = warp::read(
        &source.dataset,
        work,
        &source.spec.horizontal_crs,
        (source.spec.nodata, f64::NAN),
        "average",
        "Float64",
        true,
    )?;
    if let Some(own) = source.country {
        if labels.as_ref().is_some_and(|(window, _)| *window != work) {
            *labels = None;
        }
        let (_, labels) = labels.get_or_insert_with(|| (work, countries.labels(work)));
        keep_own_country(&mut sampled, own, labels);
    }
    datum::convert(&mut sampled, work, source)?;
    Ok(sampled)
}

pub fn build(sources: &[Source], countries: &Countries, window: Window) -> Result<Vec<u8>, String> {
    let padding = sources
        .iter()
        .filter(|s| s.spec.band == Band::GroundHeight)
        .filter(|s| s.intersects(expanded(window, s.spec.feather_nodes + 2)))
        .map(|s| s.spec.feather_nodes)
        .max()
        .unwrap_or(0);
    let padding = if padding == 0 { 0 } else { padding + 2 };
    let mut work = expanded(window, padding);
    let mut labels = None;
    let mut fine = Vec::new();
    for source in sources
        .iter()
        .filter(|s| s.spec.feather_nodes > 0 && s.intersects(work))
    {
        let sampled = read(source, work, countries, &mut labels)?;
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
            read(source, work, countries, &mut labels)?
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
                if is_ground(sampled) {
                    *height = sampled;
                }
            }
        }
    }
    let mut result = Vec::with_capacity(window.node_count() * 2);
    for row in 0..window.rows as usize {
        let begin = (row + padding as usize) * work.columns as usize + padding as usize;
        for height in &heights[begin..begin + window.columns as usize] {
            if !is_ground(*height) {
                return Err("DEM outside native encoding range".into());
            }
            let code = (*height - CODE_FLOOR_M) * CODES_PER_M;
            result.extend_from_slice(&((code + 0.5).floor() as u16).to_le_bytes());
        }
    }
    Ok(result)
}
