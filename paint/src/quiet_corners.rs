//! The block corners' paint, from the sources already evaluated there and the painter's sound model.
//!
//! Every corner has heard every source loud there: its Lden energies and its loudness cost no ray,
//! and a block whose quadrants' probes agree with their blend needs no lattice of its own.

use crate::exact::{LAYERS, PIXELS_PER_LOUDNESS_BATCH, PairEnergy};
use crate::levels::{Painted, layer_energy};
use crate::points::PointPainter;
use crate::sone::{
    NOTHING_LEFT_OUT, PixelSound, PixelSoundScratch, SoundBesideExactPairs, pixel_sound,
};
use popup::percentiles::Resolution;
use rayon::prelude::*;

/// Every corner's paint, row-major: its layers' Lden energies and its loudness, the corners' sounds
/// composed on the cores and their loudness taken on the card where there is one.
pub(crate) fn paint_corners(painter: &PointPainter) -> Result<Vec<Painted>, String> {
    let count = painter.corners.corner_points.len();
    let mut painted = Vec::with_capacity(count);
    for first in (0..count).step_by(PIXELS_PER_LOUDNESS_BATCH) {
        let chunk = first..(first + PIXELS_PER_LOUDNESS_BATCH).min(count);
        // A scratch of its own for every corner: a shared one keeps the capacity its earlier corners
        // left, the groups then sum in another order, and the parallel jobs' cuts would show in
        // the last bit of a corner's loudness.
        let sounds: Vec<([f64; LAYERS], PixelSound)> = chunk
            .into_par_iter()
            .map(|at| corner_sound(painter, &mut PixelSoundScratch::default(), at))
            .collect::<Result<_, String>>()?;
        let ndens: Vec<[f64; LAYERS + 1]> = match painter.batch {
            Some(batch) => {
                let (mut input, mut offsets) = (Vec::new(), Vec::new());
                for (_, sound) in &sounds {
                    offsets.push(input.len() as u64);
                    sound.pack(&mut input);
                }
                let card = batch.loudness(&input, &offsets)?;
                card.into_iter().map(|ndens| ndens.map(f64::from)).collect()
            }
            None => (sounds.par_iter())
                .map(|(_, sound)| sound.ndens(Resolution::MAP))
                .collect(),
        };
        painted.extend(
            sounds
                .into_iter()
                .zip(ndens)
                .map(|((layers, _), ndens)| (layers, ndens)),
        );
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
    let (candidates, energies): (Vec<u32>, Vec<PairEnergy>) = (corners.corners[at].ranked.iter())
        .flatten()
        .map(|source| (source.index, source.energy))
        .unzip();
    let (steady, bands) = corners.hum.at(x, y);
    let rest = SoundBesideExactPairs {
        steady,
        bands,
        left_out: NOTHING_LEFT_OUT,
        flights: corners.flights[at],
    };
    let point = &corners.corner_points[at];
    let reflection = point.reflection();
    let layers = layer_energy(
        painter.square,
        &rest.steady,
        &rest.flights.energy,
        reflection,
        &candidates,
        &energies,
    );
    let sound = pixel_sound(
        painter.square,
        &corners.displays,
        scratch,
        (point, reflection),
        (&candidates, &energies, &corners.corner_bands[at]),
        (&rest, &corners.flight_spectra.at(x, y)),
    )?;
    Ok((layers, sound))
}
