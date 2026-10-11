//! A building on the map: its loudest façade, as a click inside it answers (`popup::building`),
//! taken from the outdoor pixels along its outline instead of the façade receivers.

use crate::exact::LAYERS;
use crate::paint::{Cells, NO_LEVEL};
use std::collections::HashMap;

/// Gives each building's pixels (`indoor`: a pixel's place in the square and its footprint) the
/// cells of the loudest outdoor pixel next to any of them, by the total and with every layer from
/// that pixel, as the popup answers every layer at the façade whose total is loudest. A building
/// without a painted neighbour keeps no level; one across a square's edge takes each side's own.
pub fn fill_buildings(cells: &mut Cells, n: usize, indoor: &[(usize, u64)]) {
    let mut footprint_at = vec![0u64; n * n];
    for &(at, footprint) in indoor {
        footprint_at[at] = footprint;
    }
    let mut loudest: HashMap<u64, [u8; LAYERS + 1]> = HashMap::new();
    for &(at, footprint) in indoor {
        let (x, y) = (at % n, at / n);
        for (nx, ny) in (x.saturating_sub(1)..=(x + 1).min(n - 1))
            .flat_map(|nx| (y.saturating_sub(1)..=(y + 1).min(n - 1)).map(move |ny| (nx, ny)))
        {
            let neighbour = ny * n + nx;
            let cell = cells[neighbour];
            if footprint_at[neighbour] != 0 || cell[LAYERS] == NO_LEVEL {
                continue;
            }
            let best = loudest.entry(footprint).or_insert(cell);
            if cell[LAYERS] > best[LAYERS] {
                *best = cell;
            }
        }
    }
    for &(at, footprint) in indoor {
        if let Some(&cell) = loudest.get(&footprint) {
            cells[at] = cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 building in a 4x4 square takes its loudest neighbour, every layer from it; a building
    /// whose neighbours are all silent stays without level.
    #[test]
    fn a_building_reads_its_loudest_neighbour() {
        let n = 4;
        let quiet = [NO_LEVEL; LAYERS + 1];
        let mut cells = vec![quiet; n * n];
        let mut loud = [10; LAYERS + 1];
        loud[LAYERS] = 90;
        let mut softer = [50; LAYERS + 1];
        softer[LAYERS] = 60;
        cells[0] = softer;
        cells[3] = loud;
        let indoor: Vec<(usize, u64)> = [5, 6, 9, 10].iter().map(|&at| (at, 7)).collect();
        fill_buildings(&mut cells, n, &indoor);
        for &(at, _) in &indoor {
            assert_eq!(cells[at], loud);
        }
        let mut silent = vec![quiet; n * n];
        fill_buildings(&mut silent, n, &indoor);
        assert!(silent.iter().all(|cell| *cell == quiet));
    }
}
