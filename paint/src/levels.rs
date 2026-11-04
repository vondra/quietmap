//! Quantized Nden and Lden from one receiver's sound, with a shared linear-energy account.

use crate::exact::{LAYERS, PairEnergy};
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy, level_db};
use rayon::prelude::*;
use tiles::sources::Layer;

/// A cell not assessed: a building with no exposed façade, or nothing painted there.
pub const NO_LEVEL: u8 = 255;
/// A cell assessed and silent: under 0 phon, 1/16 sone (the quiet zones count it quiet, a zoom out
/// averages it as no loudness).
pub const SILENCE: u8 = 254;

/// Per pixel (row-major from the north-west) its cell per layer and the total's last ([`cell`]),
/// [`SILENCE`] or [`NO_LEVEL`].
pub type Cells = Vec<[u8; LAYERS + 1]>;

/// An outdoor point painted: per layer its Lden energy (a building's façade is chosen by it) and its
/// loudness per layer and in total (sone).
pub type Painted = ([f64; LAYERS], [f64; LAYERS + 1]);

/// The cell of a loudness (sone): twice its loudness level in phon (40 + 10 log2 sone), so a step
/// is half a phon (3.5 % of loudness) and a doubling 20 steps; [`SILENCE`] under 0 phon.
pub fn cell(sone: f64) -> u8 {
    let byte = 80.0 + 20.0 * sone.log2();
    if byte.is_nan() || byte < 0.0 {
        SILENCE
    } else {
        byte.round().min(f64::from(SILENCE - 1)) as u8
    }
}

/// The cells of a pixel's loudness per layer and in total.
pub fn cells_of(ndens: &[f64; LAYERS + 1]) -> [u8; LAYERS + 1] {
    ndens.map(cell)
}

/// Both quantities from the same assessed pixels, before they enter an atomic square bundle.
#[derive(Clone)]
pub struct PaintedCells {
    pub nden: Cells,
    pub lden: Cells,
}

impl PaintedCells {
    pub fn silence(pixels: usize) -> Self {
        Self {
            nden: vec![[SILENCE; LAYERS + 1]; pixels],
            lden: vec![[SILENCE; LAYERS + 1]; pixels],
        }
    }

    pub fn from_painted(
        pixels: impl IndexedParallelIterator<Item = Option<Painted>>,
    ) -> Result<Self, String> {
        let cells = pixels
            .enumerate()
            .map(|(at, pixel)| match pixel {
                Some((energy, ndens)) => Ok((cells_of(&ndens), lden_cells(&energy, at)?)),
                None => Ok(([NO_LEVEL; LAYERS + 1], [NO_LEVEL; LAYERS + 1])),
            })
            .collect::<Result<Vec<_>, String>>()?;
        let (nden, lden) = cells.into_iter().unzip();
        Ok(Self { nden, lden })
    }
}

/// The six layers' linear Lden energy; reflection applies to ground sources, not aircraft.
pub(crate) fn layer_energy(
    square: &Square,
    steady: &[[f64; PERIODS]; LAYERS],
    flights: &[f64; PERIODS],
    reflection: f64,
    candidates: &[u32],
    evaluated: &[PairEnergy],
) -> [f64; LAYERS] {
    let mut layers = std::array::from_fn(|layer| lden_energy(&steady[layer]));
    for (&index, energy) in candidates.iter().zip(evaluated) {
        layers[square.candidates[index as usize].layer as usize] += lden_energy(&energy.mixed);
    }
    let mut layers = layers.map(|energy| reflection * energy);
    layers[Layer::Aircraft as usize] += lden_energy(flights);
    layers
}

/// Twice Lden in dB: half-dB steps, 254 below 0 dB, 255 reserved for unassessed pixels.
pub(crate) fn lden_cell(energy: f64) -> Result<u8, String> {
    if !energy.is_finite() || energy < 0.0 {
        return Err(format!("invalid Lden energy {energy}"));
    }
    Ok(if energy < 1.0 {
        SILENCE
    } else {
        (2.0 * level_db(energy)).round().min(f64::from(SILENCE - 1)) as u8
    })
}

fn lden_cells(energy: &[f64; LAYERS], pixel: usize) -> Result<[u8; LAYERS + 1], String> {
    let mut cells = [SILENCE; LAYERS + 1];
    for (layer, &value) in energy.iter().enumerate() {
        cells[layer] =
            lden_cell(value).map_err(|e| format!("pixel {pixel}, layer {layer}: {e}"))?;
    }
    cells[LAYERS] =
        lden_cell(energy.iter().sum()).map_err(|e| format!("pixel {pixel}, total: {e}"))?;
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics::bands::energy;

    #[test]
    fn decibels_quantize_energy_and_sum_layers_before_the_logarithm() {
        assert_eq!(lden_cell(0.0).unwrap(), SILENCE);
        assert_eq!(lden_cell(energy(-0.01)).unwrap(), SILENCE);
        assert_eq!(lden_cell(1.0).unwrap(), 0);
        assert_eq!(lden_cell(energy(60.26)).unwrap(), 121);
        assert_eq!(lden_cell(energy(150.0)).unwrap(), 253);
        let mut layers = [0.0; LAYERS];
        layers[0] = energy(60.0);
        layers[Layer::Aircraft as usize] = energy(60.0);
        let cells = lden_cells(&layers, 12).unwrap();
        assert_eq!(cells[LAYERS], 126);
        assert_eq!(cells[1], SILENCE);
        layers[2] = f64::NAN;
        assert!(
            lden_cells(&layers, 12)
                .unwrap_err()
                .contains("pixel 12, layer 2")
        );
        assert!(lden_cell(-1.0).is_err());
        assert!(lden_cell(f64::INFINITY).is_err());
    }
}
