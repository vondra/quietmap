//! Fixed fine CPU judge field arm.

#[path = "h0_v3_sweep.rs"]
mod sweep;

use tile_painter::h0_pair_reference::H0V3PairArm;

fn main() -> anyhow::Result<()> {
    sweep::run(H0V3PairArm::JudgeFine)
}
