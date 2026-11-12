//! How much of what a block's corners leave out reaches a pixel, by layer and direction, and where a pixel traces deeper for it.

use crate::blocks::{Block, Strata};
use crate::corner_sources::SECTORS;
use crate::exact::LAYERS;
use physics::bands::{PERIODS, lden_energy};

/// The most a direction's far sources may sound louder at a pixel than the corners' blend of them
/// (6 dB): corners behind walls are no measure of a pixel in the open (at most 1 put the ground
/// total 0.1-0.2 dB low, 2 and 4 alike).
const TRANSFER_MOST: f64 = 4.0;

/// The share of a pixel's loud ground sources' sound (Lden energy: what it traced and what reaches
/// it of the left out, without the hum) the transfers may change before the pixel traces deeper
/// (`crate::blocks::admit_deeper`). Where they change much, what a direction's traced
/// sources lose says little of the others: in a suburb's blocks half behind an embankment the
/// loudest road's pieces fell 25 dB from the lit corners to a pixel that the direction's other
/// sources still reached, and it read 2 dB quiet. On the pairs probe (1,003 blocks) 0.3 sends
/// 0.7-6 % of the pixels deeper and the ground total's error over 1 dB falls from 2.8 to 1.1 % of a
/// suburb's pixels, over 3 dB from 0.5 % to none (private evidence 2026-11-12/fable-speed).
const DEEPER_FROM: f64 = 0.3;

impl Block {
    /// How much of what each corner leaves out of each layer and direction reaches a pixel with
    /// bilinear `weights` of the corners: the block's far sources of that direction as the pixel
    /// hears them (`heard`: a source's Lden energy there, `None` where the pixel does not trace
    /// it) over the blend of them among the corners that hear them, at most [`TRANSFER_MOST`]: a
    /// corner beyond a source's reach knows nothing of it, and counted as silence it would raise
    /// what the other corners leave out, up to fourfold beside it. A direction without one
    /// takes its layer's, a layer without one every layer's, a block without one all. Against
    /// the blend, not against each corner: where a shadow's edge crosses the block the corners in
    /// the shadow hold little of a direction, in what is left out as in the far sources traced,
    /// and the ratio takes that out (each corner's own ratio read a pixel on the lit side of a
    /// suburb's block 4 phon quiet beside its neighbours). What a corner beyond their reach holds
    /// of none of them fades toward it with its weight: sources at the edge of their reach, of
    /// 20 dB at most there.
    pub fn transfers(
        &self,
        weights: [f64; 4],
        heard: impl Fn(u32) -> Option<f64>,
    ) -> [Strata<f64>; 4] {
        let mut here = [[[0.0; SECTORS]; LAYERS]; 4];
        let mut there = [[[0.0; SECTORS]; LAYERS]; 4];
        for far in &self.far {
            let Some(energy) = heard(far.index) else {
                continue;
            };
            // Among the corners that hear the source: one beyond its reach knows nothing of it.
            let (mut blend, mut hearing) = (0.0, 0.0);
            for c in (0..4).filter(|&c| far.at_corners[c] > 0.0) {
                blend += weights[c] * far.at_corners[c];
                hearing += weights[c];
            }
            if hearing <= 0.0 {
                continue;
            }
            blend /= hearing;
            for c in 0..4 {
                here[c][far.layer as usize][far.sector[c] as usize] += energy;
                there[c][far.layer as usize][far.sector[c] as usize] += blend;
            }
        }
        let ratio =
            |here: f64, there: f64| (there > 0.0).then(|| (here / there).min(TRANSFER_MOST));
        let sum = |values: &[f64; SECTORS]| values.iter().sum::<f64>();
        // Every corner holds every far source once: the layers' sums are the same at each.
        let whole = ratio(
            here[0].iter().map(sum).sum(),
            there[0].iter().map(sum).sum(),
        );
        std::array::from_fn(|c| {
            std::array::from_fn(|layer| {
                let of_layer = ratio(sum(&here[c][layer]), sum(&there[c][layer]))
                    .or(whole)
                    .unwrap_or(1.0);
                std::array::from_fn(|sector| {
                    ratio(here[c][layer][sector], there[c][layer][sector]).unwrap_or(of_layer)
                })
            })
        })
    }

    /// What the block leaves out as it reaches a pixel with bilinear `weights` of the corners
    /// ([`Block::transfers`] by `heard`).
    pub fn reaching(&self, weights: [f64; 4], heard: impl Fn(u32) -> Option<f64>) -> Reaching {
        let mut transfers = self.transfers(weights, heard);
        let mut energy = [[[0.0; 4]; PERIODS]; LAYERS];
        let mut moved = 0.0;
        for c in 0..4 {
            for layer in 0..LAYERS {
                let strata = transfers[c][layer].iter_mut().zip(&self.left_out[c][layer]);
                for (transfer, left) in strata {
                    moved += weights[c] * lden_energy(left) * (*transfer - 1.0).abs();
                    *transfer *= weights[c];
                    for p in 0..PERIODS {
                        energy[layer][p][c] += *transfer * left[p];
                    }
                }
            }
        }
        Reaching {
            transfers,
            energy,
            moved,
        }
    }
}

/// What a block leaves out as it reaches a pixel: each corner's transfers times the pixel's weight
/// of the corner; per layer, period and corner the energy arriving; and the Lden energy the
/// transfers changed.
pub(crate) struct Reaching {
    pub transfers: [Strata<f64>; 4],
    pub energy: [[[f64; 4]; PERIODS]; LAYERS],
    moved: f64,
}

impl Reaching {
    /// Whether the transfers changed more than [`DEEPER_FROM`] of the pixel's loud ground sources'
    /// sound, `traced` the Lden energy of the sources it traced.
    pub fn asks_deeper(&self, traced: f64) -> bool {
        let left: f64 = (self.energy.iter())
            .map(|layer| lden_energy(&layer.map(|corners| corners.iter().sum())))
            .sum();
        self.moved > DEEPER_FROM * (traced + left)
    }
}
