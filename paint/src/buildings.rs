//! A building on the map: its loudest façade, as a click inside it answers (`popup::building`),
//! taken from the outdoor pixels along its outline instead of the façade receivers.

use crate::exact::LAYERS;
use crate::paint::Painted;
use std::collections::HashMap;
use tiles::sources::Layer;

/// The ground layers' energy of a pixel: what the popup ranks façades by (the flights apart).
fn ground_energy(layers: &[f64; LAYERS]) -> f64 {
    (layers.iter().enumerate())
        .filter(|&(layer, _)| layer != Layer::Aircraft as usize)
        .map(|(_, energy)| energy)
        .sum()
}

/// The decibels a building's own façade adds to the reflection at `position` (click metres): the
/// receiver's reflection with every building against it without `footprint` (`popup::building`).
pub fn own_reflection_db(
    square: &crate::square::Square,
    position: [f64; 2],
    footprint: u64,
) -> Result<f64, String> {
    Ok(square.obstacles.reflection_db(position, None)?
        - square.obstacles.reflection_db(position, Some(footprint))?)
}

/// A painted façade with the reflection its own building adds taken away, `incident` the energy's
/// ratio: its ground layers' energy times it, its flights as they are; its loudness as its
/// loudness level follows the energy's level (a loudness level a phon per decibel: sone times the
/// ratio to the power log10 2), the total by its whole energy's ratio.
fn incident((energies, ndens): Painted, incident: f64) -> Painted {
    let aircraft = Layer::Aircraft as usize;
    let air = energies[aircraft];
    let ground = ground_energy(&energies);
    let follow = |sone: f64, ratio: f64| sone * ratio.powf(std::f64::consts::LOG10_2);
    let mut layers = energies;
    let mut loudness = ndens;
    for layer in (0..LAYERS).filter(|&layer| layer != aircraft) {
        layers[layer] *= incident;
        loudness[layer] = follow(ndens[layer], incident);
    }
    if ground + air > 0.0 {
        loudness[LAYERS] = follow(ndens[LAYERS], (incident * ground + air) / (ground + air));
    }
    (layers, loudness)
}

/// Gives each building's pixels (`indoor`: a pixel's place in the square and its footprint) its
/// loudest façade: of the outdoor pixels next to any of them (`pixels`, `None` where nothing is
/// painted), the one loudest from the ground sources, without the reflection its own building's
/// façade adds there (`own_reflection_db` of that pixel and building: CNOSSOS-EU 2.8 takes the sound
/// falling on a façade; [`incident`]). A building without a painted neighbour keeps no level; one
/// across a square's edge takes each side's own.
pub fn fill_buildings(
    pixels: &mut [Option<Painted>],
    n: usize,
    indoor: &[(usize, u64)],
    own_reflection_db: &dyn Fn(usize, u64) -> Result<f64, String>,
) -> Result<(), String> {
    // Per building its loudest outdoor neighbour: (ground energy, the neighbour's place).
    let mut loudest: HashMap<u64, (f64, usize)> = HashMap::new();
    for &(at, footprint) in indoor {
        let (x, y) = (at % n, at / n);
        for (nx, ny) in (x.saturating_sub(1)..=(x + 1).min(n - 1))
            .flat_map(|nx| (y.saturating_sub(1)..=(y + 1).min(n - 1)).map(move |ny| (nx, ny)))
        {
            let neighbour = ny * n + nx;
            // A building's own pixels have no energies until the end: only outdoor ones answer.
            let Some((layers, _)) = pixels[neighbour] else {
                continue;
            };
            // Equal energies go to the first pixel in the square's order, whatever the passes' order.
            let ground = ground_energy(&layers);
            let best = loudest.entry(footprint).or_insert((ground, neighbour));
            if ground > best.0 || (ground == best.0 && neighbour < best.1) {
                *best = (ground, neighbour);
            }
        }
    }
    let mut facades: HashMap<u64, Painted> = HashMap::with_capacity(loudest.len());
    for (&footprint, &(_, neighbour)) in &loudest {
        let facade = pixels[neighbour].expect("a painted neighbour");
        let ratio = 10f64.powf(-own_reflection_db(neighbour, footprint)? / 10.0);
        facades.insert(footprint, incident(facade, ratio));
    }
    for &(at, footprint) in indoor {
        if let Some(&facade) = facades.get(&footprint) {
            pixels[at] = Some(facade);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 building in a 4x4 square takes its neighbour loudest from the ground sources (not the
    /// one loudest with its flights), without its own façade's reflection, the flights unchanged:
    /// 3 dB less energy, 3 phon less loudness level on the ground layers (sone times 2^-0.3); a
    /// building whose neighbours are all unpainted stays without level.
    #[test]
    fn a_building_reads_its_loudest_facade() {
        let n = 4;
        let (road, aircraft) = (Layer::Road as usize, Layer::Aircraft as usize);
        let mut pixels: Vec<Option<Painted>> = vec![None; n * n];
        let mut loud = [1000.0; LAYERS];
        loud[aircraft] = 1.0;
        let mut overflown = [10.0; LAYERS];
        overflown[aircraft] = 1e9;
        pixels[0] = Some((overflown, [1.0; LAYERS + 1]));
        pixels[3] = Some((loud, [8.0; LAYERS + 1]));
        let indoor: Vec<(usize, u64)> = [5, 6, 9, 10].iter().map(|&at| (at, 7)).collect();
        let three_db = |_: usize, _: u64| Ok(3.0);
        fill_buildings(&mut pixels, n, &indoor, &three_db).unwrap();
        let halved = 1000.0 * 10f64.powf(-0.3);
        for &(at, _) in &indoor {
            let (layers, ndens) = pixels[at].unwrap();
            assert!((layers[road] - halved).abs() < 1e-9 && layers[aircraft] == 1.0);
            assert!((ndens[road] - 8.0 * 2f64.powf(-0.3)).abs() < 1e-3 && ndens[aircraft] == 8.0);
        }
        let mut unpainted: Vec<Option<Painted>> = vec![None; n * n];
        fill_buildings(&mut unpainted, n, &indoor, &three_db).unwrap();
        assert!(unpainted.iter().all(Option::is_none));
    }
}
