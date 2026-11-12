//! A block corner's loud sources: the direction each is heard from and those the pixels around the corner trace.

use crate::exact::{LAYERS, PairEnergy};
use physics::bands::lden_energy;

/// Directions of arrival a corner tells apart. What stands around a pixel screens the far sources
/// of one direction alike, so one of them traced at the pixel carries the others: on 1,003 blocks
/// of 17 squares of every class (every loud source traced at 16 pixels a block) sixteen directions
/// with the share below left the ground total 0.06-0.32 dB rms from every source traced, with
/// 27-140 pairs a pixel where a fifth of each layer left out took 98-494; eight directions 0.08-0.39
/// dB (private evidence 2026-11-12/fable-speed).
pub const SECTORS: usize = 16;

/// A direction of a layer holding less than this share of the layer's far energy at a corner keeps
/// no representative of its own: it takes the layer's whole transfer there.
const STRATUM_SHARE: f64 = 0.01;

/// A corner traces its loudest sources, and its loudest far ones, until what it has not traced is
/// at most this share of the layer there, and of the layer's far sources, per layer in
/// `Layer::ALL`'s order (road, railway, industry, building, ship, aircraft). A railway's a fifth:
/// its lines sound by their trains' passes, each line's energy apart, and what a direction's
/// representative says of the others is less true of a few lines than of many roads. Of 833
/// clicks hearing a railway the map missed 16 by over 1 phon with a half, 9 with a fifth at 2 %
/// more time, 7 with a twentieth at 7 % (the world sample, 2026-11-12).
const UNTRACED_SHARES: [f64; LAYERS] = [0.5, 0.2, 0.5, 0.5, 0.5, 0.5];

/// A layer's untraced sources under this Lden energy at a corner (25 dB) need no tracing, and its
/// far sources under it no representative: wrong by half, they move a total of 35 dB, the quietest
/// a visitor sees, by 0.2 dB. Without it a layer of many faint sources (a village's plant at
/// 15 dB) was traced to its half at every pixel.
const UNTRACED_FLOOR: f64 = 316.227_766_016_837_95;

/// The direction a source is heard from, `offset` its nearest point from the receiver (east, north).
pub(crate) fn sector_of(offset: [f64; 2]) -> u8 {
    let turn = (offset[1].atan2(offset[0]) + std::f64::consts::PI) / std::f64::consts::TAU;
    ((turn * SECTORS as f64) as usize % SECTORS) as u8
}

/// A source loud at a corner: its index, its group, its Lden energy, its energy per period (mixed
/// and in either weather state), its emitters' distance from the corner's receiver and the
/// direction it is heard from.
pub(crate) struct Ranked {
    pub index: u32,
    pub group: u64,
    pub lden: f64,
    pub energy: PairEnergy,
    pub slant_distance_m: f64,
    pub sector: u8,
}

/// A block corner: per layer every source loud there, loudest first, the layer's total Lden
/// energy, and the sources the pixels around it trace (sorted).
#[derive(Default)]
pub(crate) struct Corner {
    pub ranked: [Vec<Ranked>; LAYERS],
    pub totals: [f64; LAYERS],
    pub traced: Vec<u32>,
}

impl Corner {
    /// Adds source `index` of `group` and `layer` heard there from `sector` with `energy` per
    /// period, its emitters `slant_distance_m` away.
    pub fn add(
        &mut self,
        (layer, sector): (usize, u8),
        (index, group): (u32, u64),
        (energy, slant_distance_m): (PairEnergy, f64),
    ) {
        let lden = lden_energy(&energy.mixed);
        self.ranked[layer].push(Ranked {
            index,
            group,
            lden,
            energy,
            slant_distance_m,
            sector,
        });
        self.totals[layer] += lden;
    }

    /// Chooses the sources the pixels around the corner trace, its layers ranked loudest first: per
    /// layer and direction the loudest far source (`near` says which sources cross the blocks
    /// around the corner), then the loudest until the untraced are within [`UNTRACED_SHARES`] of the
    /// layer, and the loudest far ones until the untraced far ones are within it of the far.
    pub fn choose_traced(&mut self, near: impl Fn(u32) -> bool) {
        let mut traced = Vec::new();
        for ((ranked, &total), share) in self.ranked.iter().zip(&self.totals).zip(UNTRACED_SHARES) {
            let far: Vec<bool> = ranked.iter().map(|source| !near(source.index)).collect();
            let mut far_by_sector = [0.0; SECTORS];
            for (source, _) in ranked.iter().zip(&far).filter(|(_, far)| **far) {
                far_by_sector[source.sector as usize] += source.lden;
            }
            let far_total: f64 = far_by_sector.iter().sum();
            let mut chosen = vec![false; ranked.len()];
            let mut represented = [false; SECTORS];
            let (mut heard, mut heard_far) = (0.0, 0.0);
            let mut take = |at: usize, chosen: &mut Vec<bool>| {
                chosen[at] = true;
                traced.push(ranked[at].index);
                ranked[at].lden
            };
            for at in 0..ranked.len() {
                let sector = ranked[at].sector as usize;
                if far[at]
                    && !represented[sector]
                    && far_total > UNTRACED_FLOOR
                    && far_by_sector[sector] >= STRATUM_SHARE * far_total
                {
                    represented[sector] = true;
                    let lden = take(at, &mut chosen);
                    heard += lden;
                    heard_far += lden;
                }
            }
            for at in 0..ranked.len() {
                let enough = total - heard <= (share * total).max(UNTRACED_FLOOR);
                let enough_far = far_total - heard_far <= (share * far_total).max(UNTRACED_FLOOR);
                if enough && enough_far {
                    break;
                }
                if !chosen[at] && (!enough || far[at]) {
                    let lden = take(at, &mut chosen);
                    heard += lden;
                    if far[at] {
                        heard_far += lden;
                    }
                }
            }
        }
        traced.sort_unstable();
        self.traced = traced;
    }
}
