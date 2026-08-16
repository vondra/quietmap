//! Fixed 3-degree H0 V3 CPU field arm.

#[path = "h0_v3_sweep.rs"]
mod sweep;

use noise_compute::propagation::h0_v3::H0V3Theta;
use tile_painter::h0_pair_reference::H0V3PairArm;

fn main() -> anyhow::Result<()> {
    sweep::run(H0V3PairArm::Production(H0V3Theta::Degrees3))
}
