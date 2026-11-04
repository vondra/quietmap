//! Sparse popup references for a square.

use super::*;

/// The popup's answers at `pixels` random pixels of the square (a building's at its loudest façade),
/// as a sparse reference: the square's tiles under `out`, every other cell not assessed (`compare`
/// scores a map against it).
pub(super) fn popup_reference(
    release: &Release,
    tile: TileId,
    pixels: usize,
    random: &mut SplitMix,
    out: &std::path::Path,
) -> Result<(), String> {
    const SIDE: usize = 1024;
    let chosen: Vec<usize> = (0..pixels)
        .map(|_| (random.next_u64() % (SIDE * SIDE) as u64) as usize)
        .collect();
    let started = Instant::now();
    let answers: Vec<Option<paint::levels::Painted>> = chosen
        .par_iter()
        .map(|&at| {
            let (lat, lon) = Mercator {
                x: f64::from(tile.x) + ((at % SIDE) as f64 + 0.5) / SIDE as f64,
                y: f64::from(tile.y) + ((at / SIDE) as f64 + 0.5) / SIDE as f64,
            }
            .to_degrees();
            let options = popup::answer::Options {
                exact: false,
                source: Vec::new(),
                piece: None,
            };
            let mut last = None;
            popup::answer::answer(release, lat, lon, &options, &mut |update| {
                let assessed = (update.building).is_none_or(|building| building.facade.is_some());
                if !update.partial && assessed {
                    let mut ndens = [0.0; LAYERS + 1];
                    let mut energy = [0.0; LAYERS];
                    for layer in &update.layers {
                        ndens[layer.layer as usize] = layer.nden_sone.unwrap_or(0.0);
                        energy[layer.layer as usize] = lden_energy(&layer.energy);
                    }
                    ndens[LAYERS] = update.loudness.map_or(0.0, |loudness| loudness.nden_sone);
                    last = Some((energy, ndens));
                }
                Ok(())
            })?;
            Ok(last)
        })
        .collect::<Result<_, String>>()?;
    let mut sampled_pixels = vec![None; SIDE * SIDE];
    for (&at, answer) in chosen.iter().zip(answers) {
        if let Some(answer) = answer {
            sampled_pixels[at] = Some(answer);
        }
    }
    let cells = paint::levels::PaintedCells::from_painted(sampled_pixels.into_par_iter())?;
    paint::hm3::write(out, 13, (tile.x, tile.y), &cells)?;
    eprintln!(
        "popup: {pixels} pixels answered in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
