//! Forest density uses each survey until an observed loss after that survey and before the cutoff.
use super::{config::Band, source::Source, warp};
use gdal::Dataset;
use tiles::terrain::Window;

fn loss_lookup(base_year: i32, observed_year: i32, target_year: i32) -> String {
    (0..=255)
        .map(|code| {
            format!(
                "{code}:{}",
                u8::from(
                    code > 0 && base_year + code > observed_year && base_year + code <= target_year
                )
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn forest_loss(
    source: &Source,
    window: Window,
    observed_year: i32,
    target_year: i32,
) -> Result<Vec<f64>, String> {
    let source = source.for_window(window);
    let lookup = loss_lookup(
        source
            .spec
            .loss_base_year
            .ok_or("loss raster needs its base year")?,
        observed_year,
        target_year,
    );
    let mask: Dataset = source.mapped(&lookup, 255.0)?;
    // GDAL 3.8's byte maximum loses zero when source and target nodata differ.
    // The native mask remains Byte; a floating target preserves the exact maximum.
    warp::read(
        &mask,
        window,
        &source.spec.horizontal_crs,
        (Some(255.0), 0.0),
        "max",
        "Float32",
        false,
    )
}

pub fn build(sources: &[Source], window: Window, year: i32) -> Result<Vec<u8>, String> {
    let mut forest = vec![0u8; window.node_count()];
    for source in sources.iter().filter(|s| s.spec.band == Band::TreeCover) {
        if !source.intersects(window) {
            continue;
        }
        let source = source.for_window(window);
        let values = warp::read(
            &source.dataset,
            window,
            &source.spec.horizontal_crs,
            (source.spec.nodata, source.spec.nodata.unwrap_or(0.0)),
            "average",
            "Float32",
            false,
        )?;
        let observed_year = source.observed_year;
        let loss = source
            .spec
            .loss_input
            .as_ref()
            .map(|id| {
                let source = sources
                    .iter()
                    .find(|s| s.spec.input == *id)
                    .ok_or_else(|| format!("missing admitted loss input {id}"))?;
                forest_loss(source, window, observed_year, year)
            })
            .transpose()?;
        for (i, value) in values.into_iter().enumerate() {
            if (0.0..=100.0).contains(&value) {
                // The producer quantized Float32 averages, including the addition at exact halves.
                forest[i] = if loss.as_ref().is_some_and(|l| l[i] > 0.0) {
                    0
                } else {
                    (value as f32 + 0.5).floor() as u8
                };
            }
        }
    }
    Ok(forest)
}
