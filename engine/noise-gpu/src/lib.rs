//! Shared host-side helpers for the GPU airborne renderer.

/// Region-resident GPU airborne scatter, shared by the `e2-airborne` validator and the
/// `gpu-airborne` production builder (cudarc-backed, so gated on the `gpu` feature).
#[cfg(feature = "gpu")]
pub mod airborne;
#[cfg(feature = "gpu")]
mod airborne_building_horizon;
#[cfg(feature = "gpu")]
mod airborne_terrain_horizon;

use noise_compute::emission::aircraft::{Installation, SegmentPrepared, M_PER_DEG_LAT};
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};

fn inst_code(inst: Installation) -> i32 {
    match inst {
        Installation::Wing => 0,
        Installation::Fuselage => 1,
        Installation::Propeller => 2,
    }
}

/// Pack a tile's receiver lattice (rll = lat|lon|m_per_deg_lon, rxa = elevation) —
/// uploaded once; near and every far level index into the same lattice.
pub fn pack_airborne_receivers(tile: &FusedTileZ13) -> (Vec<f64>, Vec<f32>) {
    let n = TILE_PX;
    let mut rll = Vec::with_capacity(3 * n);
    rll.extend_from_slice(&tile.rx_lat); // [0..n] receiver latitude
    rll.extend_from_slice(&tile.rx_lon); // [n..2n] receiver longitude
    for py in 0..n {
        // Mirror airborne.rs:175 — m_per_deg_lon per receiver row.
        rll.push(M_PER_DEG_LAT * tile.rx_lat[py].to_radians().cos().max(0.2));
    }
    (rll, tile.rx_alt_m.clone())
}

/// Pack a sub-segment list into the per-segment device SoA (sll, sf, si) —
/// shared by the near launch and each far level.
pub fn pack_airborne_segs(segs: &[(SegmentPrepared, u8)]) -> (Vec<f64>, Vec<f32>, Vec<i32>) {
    let nseg = segs.len();
    let mut sll = vec![0.0f64; 2 * nseg];
    let mut sf = Vec::with_capacity(12 * nseg);
    let mut si = Vec::with_capacity(4 * nseg);
    for (s, (p, period)) in segs.iter().enumerate() {
        sll[s] = p.start_lat;
        sll[nseg + s] = p.start_lon;
        sf.extend_from_slice(&[
            p.start_alt_m as f32,
            p.d_lon as f32,
            p.sdy as f32,
            p.sdz as f32,
            p.dv as f32,
            p.d_bar_m as f32,
            p.di_a as f32,
            p.di_b as f32,
            p.di_c as f32,
            p.reach_sq as f32,
            p.terrain_start_cut_m as f32,
            p.terrain_end_cut_m as f32,
        ]);
        si.extend_from_slice(&[
            inst_code(p.inst),
            p.class_idx as i32,
            p.is_departure as i32,
            (*period).min(2) as i32,
        ]);
    }
    (sll, sf, si)
}
