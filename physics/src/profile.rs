//! The ground under one ray: a vertex wherever the ray crosses a line of the terrain lattice, its
//! two ends included (the popup's scene fills it). On a lattice line the bilinear terrain is linear
//! between the line's two nodes, so the vertices hold every node row and column the ray passes:
//! no crest of the lattice falls between two of them (a sampling cadence read quiet hills 0.4-1.0
//! dB loud and a railway 3.8 dB, `evidence/2026-10-10/propagation-architecture/`).

/// The ground under one ray, sampled at fractions `t` of its horizontal length (0 the source, 1
/// the receiver), in increasing order.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub horizontal_m: f64,
    pub t: Vec<f64>,
    /// Bare-earth altitude (m).
    pub ground_m: Vec<f64>,
    /// CNOSSOS G in [0, 1].
    pub ground_factor: Vec<f64>,
}

impl Profile {
    /// Empties the samples of a ray of `horizontal_m`.
    pub fn clear(&mut self, horizontal_m: f64) {
        self.horizontal_m = horizontal_m;
        self.t.clear();
        self.ground_m.clear();
        self.ground_factor.clear();
    }

    /// Adds the ground at fraction `t`, beyond every sample already added.
    pub fn push(&mut self, t: f64, ground_m: f64, ground_factor: f64) {
        debug_assert!(self.t.last().is_none_or(|&last| t > last));
        self.t.push(t);
        self.ground_m.push(ground_m);
        self.ground_factor.push(ground_factor);
    }
}
