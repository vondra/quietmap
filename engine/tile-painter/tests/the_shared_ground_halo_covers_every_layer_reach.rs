//! The shared ground halo may never be narrower than the reach of a layer that shares it.

use clap::ValueEnum;
use tile_painter::surface_region::{layer_meta, Source, WIDEST_GROUND_HALO_M};

/// `WIDEST_GROUND_HALO_M` enumerates the five per-layer halos a second time, beside the
/// `layer_meta` table that answers with them. A sixth ground layer reaching further than rail,
/// added to the enum and the table but not to that list, would leave the GPU painter sampling
/// its last kilometres from the clamped halo edge — the very defect the constant exists to
/// close. Walking `value_variants` puts every future layer in front of this assertion.
#[test]
fn the_shared_ground_halo_covers_every_layer_reach() {
    let widest = Source::value_variants()
        .iter()
        .filter(|source| !matches!(source, Source::Ground))
        .map(|&source| layer_meta(source).1)
        .fold(0.0_f64, f64::max);
    assert_eq!(
        WIDEST_GROUND_HALO_M, widest,
        "the shared ground halo must cover the widest per-layer reach"
    );
}
