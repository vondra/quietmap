//! The ground under one ray: a vertex wherever the ray crosses a line of the terrain lattice, its
//! two ends included (the popup's scene fills it). On a lattice line the bilinear terrain is linear
//! between the line's two nodes, so the vertices hold every node row and column the ray passes:
//! no crest of the lattice falls between two of them (a sampling cadence read quiet hills 0.4-1.0
//! dB loud and a railway 3.8 dB, measured 2026-10-10). Within
//! [`FINE_ENDS_M`] of either end the lattice is the 1/6" one where a national model gives it.

/// Metres from either end of a ray within which its ground is walked on the 1/6" lattice: a berm,
/// a cutting or a road's shelf narrower than the 1" lattice screens there (against 5 m heights
/// every 2 m along the whole ray: the D4's berm at Mníšek 3.52 -> 0.12 dB, Pasito A 1.03 -> 0.20,
/// a hillside village 0.19 -> 0.02; 300 m gains at most 0.12 dB more for three times the added vertices;
/// measured 2026-10-10).
pub const FINE_ENDS_M: f64 = 100.0;

/// The ground under one ray, sampled at fractions `t` of its horizontal length (0 the source, 1
/// the receiver), in increasing order; every vector holds one value a sample.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub horizontal_m: f64,
    pub t: Vec<f64>,
    /// Bare-earth altitude (m).
    pub ground_m: Vec<f64>,
    /// CNOSSOS G in [0, 1].
    pub ground_factor: Vec<f64>,
    /// Tree cover in [0, 1].
    pub forest_cover: Vec<f64>,
    /// Height of the tree crowns' top above the ground (m).
    pub canopy_m: Vec<f64>,
}

impl Profile {
    /// Empties the samples of a ray of `horizontal_m`.
    pub fn clear(&mut self, horizontal_m: f64) {
        self.horizontal_m = horizontal_m;
        self.t.clear();
        self.ground_m.clear();
        self.ground_factor.clear();
        self.forest_cover.clear();
        self.canopy_m.clear();
    }

    /// Adds the ground at fraction `t`, beyond every sample already added.
    pub fn push(
        &mut self,
        t: f64,
        ground_m: f64,
        ground_factor: f64,
        forest_cover: f64,
        canopy_m: f64,
    ) {
        debug_assert!(self.t.last().is_none_or(|&last| t > last));
        self.t.push(t);
        self.ground_m.push(ground_m);
        self.ground_factor.push(ground_factor);
        self.forest_cover.push(forest_cover);
        self.canopy_m.push(canopy_m);
    }
}
