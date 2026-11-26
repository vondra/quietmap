//! The block corners' paint, from the sources already evaluated there and the painter's sound model.
//!
//! Every corner has heard every source loud there: its Lden energies and its loudness cost no ray,
//! and a block whose quadrants' probes agree with their blend needs no lattice of its own.

use crate::exact::{LAYERS, PIXELS_PER_LOUDNESS_BATCH};
use crate::levels::Painted;
use crate::points::PointPainter;
use crate::sone::{
    NOTHING_LEFT_OUT, PixelSound, PixelSoundScratch, SoundBesideExactPairs, corner_pixel_sound,
};
use physics::bands::lden_energy;
use popup::percentiles::Resolution;
use rayon::prelude::*;
use tiles::sources::Layer;

/// Every corner's paint, row-major: its layers' Lden energies and its loudness, the corners' sounds
/// composed on the cores and their loudness taken on the card where there is one.
pub(crate) fn paint_corners(painter: &PointPainter) -> Result<Vec<Painted>, String> {
    let count = painter.corners.corner_points.len();
    let mut painted = Vec::with_capacity(count);
    for first in (0..count).step_by(PIXELS_PER_LOUDNESS_BATCH) {
        let end = (first + PIXELS_PER_LOUDNESS_BATCH).min(count);
        let indices: Vec<usize> = (first..end).collect();
        match painter.batch {
            Some(batch) => {
                let parts = indices
                    .par_chunks(128)
                    .map_init(
                        PixelSoundScratch::default,
                        |scratch, slice| -> Result<_, String> {
                            let mut layers = Vec::with_capacity(slice.len());
                            let mut input = Vec::new();
                            let mut offsets = Vec::with_capacity(slice.len());
                            for &at in slice {
                                let (layer, sound) = corner_sound(painter, scratch, at)?;
                                layers.push(layer);
                                offsets.push(input.len() as u64);
                                sound.pack_with_scratch(&mut scratch.fold, &mut input);
                            }
                            Ok((layers, input, offsets))
                        },
                    )
                    .collect::<Result<Vec<_>, String>>()?;
                let floats: usize = parts.iter().map(|(_, i, _)| i.len()).sum();
                let mut input = Vec::with_capacity(floats);
                let mut offsets = Vec::with_capacity(end - first);
                let mut layers = Vec::with_capacity(end - first);
                for (part_layers, part_in, part_off) in parts {
                    let base = input.len() as u64;
                    offsets.extend(part_off.into_iter().map(|o| base + o));
                    input.extend_from_slice(&part_in);
                    layers.extend(part_layers);
                }
                let card = batch.loudness(&input, &offsets)?;
                painted.extend(
                    layers
                        .into_iter()
                        .zip(card.into_iter().map(|ndens| ndens.map(f64::from))),
                );
            }
            None => {
                let chunk: Vec<Painted> = indices
                    .into_par_iter()
                    .map_init(PixelSoundScratch::default, |scratch, at| {
                        let (layers, sound) = corner_sound(painter, scratch, at)?;
                        Ok((
                            layers,
                            sound.ndens_with_scratch(&mut scratch.fold, Resolution::MAP),
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                painted.extend(chunk);
            }
        }
    }
    Ok(painted)
}

/// Corner `at`'s layers' Lden energies and its sound: every source loud there as evaluated, the hum
/// and the flights there.
fn corner_sound(
    painter: &PointPainter,
    scratch: &mut PixelSoundScratch,
    at: usize,
) -> Result<([f64; LAYERS], PixelSound), String> {
    let grid = painter.grid;
    let side = grid.pixels / grid.block + 1;
    let corners = &painter.corners;
    // The hum and the flights' spectrum of the pixel at the corner, the square's last pixel for a
    // corner on its east or south edge: the lookups blend around pixel centres and would
    // extrapolate past the last.
    let last = grid.pixels - 1;
    let (x, y) = (
        ((at % side) * grid.block).min(last),
        ((at / side) * grid.block).min(last),
    );
    let (steady, bands) = corners.hum.at(x, y);
    let rest = SoundBesideExactPairs {
        steady,
        bands,
        left_out: NOTHING_LEFT_OUT,
        flights: corners.flights[at],
    };
    let point = &corners.corner_points[at];
    let reflection = point.reflection();
    let ranked = &corners.corners[at].ranked;
    let mut layers = std::array::from_fn(|layer| {
        let exact: f64 = ranked[layer].iter().map(|source| source.lden).sum();
        reflection * (lden_energy(&rest.steady[layer]) + exact)
    });
    layers[Layer::Aircraft as usize] += lden_energy(&rest.flights.energy);
    let sound = corner_pixel_sound(
        painter.square,
        &corners.displays,
        scratch,
        reflection,
        (ranked, &corners.corner_bands[at]),
        (&rest, &corners.flight_spectra.at(x, y)),
    )?;
    Ok((layers, sound))
}
