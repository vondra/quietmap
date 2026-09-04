//! What an obstacle's HEIGHT lets it screen: the geometric bound used to prune the airborne
//! heatmap's vector-building horizon.
//!
//! A roof edge or terrain sample whose top sits `h` above the receiver's ear at horizontal
//! range `r` presents the tangent `h / r`. Both horizon queries
//! (`screening::screening_from_bands`, `horizon::ReceiverHorizon::screening_dz`) keep an edge
//! only while the source tangent is STRICTLY below that, so an obstacle can matter only for
//! aircraft whose elevation angle from the receiver is below its own. That is the whole
//! criterion: not a radius, but `h > tangent · r` for some aircraft that is actually there.
//!
//! The bound is only as good as the lowest aircraft tangent it is taken against, and taking
//! that minimum over ALL sub-segments of a tile makes it useless: one far, low sub-segment —
//! an aircraft on the runway seen from three kilometres away — sits at ≈ 0° and keeps every
//! roof in the tile. But that aircraft lies in ONE DIRECTION from the receiver, and the
//! horizon it queries is stored per azimuth sector, so it can only ever be compared against
//! roofs in that same sector. The floor is therefore taken PER SECTOR GROUP: a roof is kept
//! only when some aircraft in its own direction is low enough to be shaded by it.
//! These Rust functions are the tested reference formulas; the production CUDA kernels mirror
//! them by hand, while `build.rs` injects the constants from this module into that translation.

use super::screening::BUILDING_LOCAL_HORIZON_SECTORS;

/// The obstacle-height criterion. `nearest_range_m` may be a lower bound on the true range
/// (the nearest point of an obstacle grid cell) when `lowest_source_tangent >= 0`, because the
/// inequality only tightens; a negative bound would need the exact range.
///
/// Dropping such an edge cannot change a query. The stored horizon keeps one range-max entry
/// per (sector, band) and the queries compare against its tangent, so removing the band's
/// maximum could otherwise expose a lower edge that the original discarded — but this
/// criterion is a pure tangent threshold, and the band maximum is the tangent maximum: if the
/// band's tallest edge fails it, every edge in that band fails it too.
#[inline]
pub fn horizon_edge_cannot_screen(
    top_rel_alt_m: f64,
    nearest_range_m: f64,
    lowest_source_tangent: f64,
) -> bool {
    top_rel_alt_m <= lowest_source_tangent * nearest_range_m
}

/// Azimuth groups the lowest source tangent is bounded over, each a whole number of the 256
/// query sectors (5.625°, four query sectors). Finer groups bound direction better but cost
/// one more device minimum per group a sub-segment spans; 128 groups bought only 0.73% on the
/// dense tuning cell and was rejected under the 3% rule.
pub const LOWEST_SOURCE_TANGENT_SECTOR_GROUPS: usize = 64;
const _: () =
    assert!(BUILDING_LOCAL_HORIZON_SECTORS.is_multiple_of(LOWEST_SOURCE_TANGENT_SECTOR_GROUPS));

/// Slack taken off every lowest-source-tangent bound: the kernels form the source tangent in
/// f32 (about 1e-7 relative). The 1e-3 relative margin inflates the separately computed
/// receiver-row metric and block-displacement bounds; the 1e-4 absolute tangent margin covers
/// cancellation near a horizontal line of sight.
pub const LOWEST_SOURCE_TANGENT_MARGIN_REL: f64 = 1e-3;
pub const LOWEST_SOURCE_TANGENT_MARGIN_ABS: f64 = 1e-4;
/// Metre slack on receiver-to-segment distances for the same rounding.
pub const LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M: f64 = 0.01;
/// Radian slack on every source-direction interval, covering both the f32 `atan2` the query
/// forms its sector with and the per-row metres-per-degree spread inside a receiver block
/// (3e-4 relative, i.e. under 3e-4 rad of direction).
pub const LOWEST_SOURCE_TANGENT_ANGLE_MARGIN_RAD: f64 = 1e-3;

/// Lower bound on the source tangent (`source_rel_alt / lateral`, what both horizon queries
/// compare an edge against) that any receiver of one pixel block sees from one sub-segment's
/// source point, given the lowest altitude that point can have and the block centre's distance
/// to the locus it lies on. The point's lateral distance lies within the block half-diagonal
/// of that distance and the block's highest receiver bounds the numerator. A block's floor
/// for one direction group is the minimum over every sub-segment whose source point can lie
/// in that group.
pub fn lowest_source_tangent(
    source_min_alt_m: f64,
    block_max_receiver_alt_m: f64,
    centre_to_locus_m: f64,
    block_half_diagonal_m: f64,
) -> f64 {
    let rel = LOWEST_SOURCE_TANGENT_MARGIN_REL;
    let numerator = source_min_alt_m - block_max_receiver_alt_m;
    let bound = if numerator >= 0.0 {
        let farthest = (centre_to_locus_m + block_half_diagonal_m) * (1.0 + rel)
            + LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M;
        numerator / farthest
    } else {
        let nearest = ((centre_to_locus_m - block_half_diagonal_m) * (1.0 - rel)
            - LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M)
            .max(1e-3);
        numerator / nearest
    };
    bound - bound.abs() * rel - LOWEST_SOURCE_TANGENT_MARGIN_ABS
}

/// Lowest altitude the physical closest point of a finite aircraft sub-segment can have for
/// any receiver in a block. Orthogonal projection onto the segment is 1-Lipschitz: moving the
/// receiver by at most the block half-diagonal moves its unbounded segment parameter by at
/// most `half_diagonal / segment_length` in one metric. Receiver rows use slightly different
/// longitude scales. If their ratio is `q`, anisotropic projection can additionally move the
/// closest point along the segment by at most `|q - 1/q| / 2` times the perpendicular line
/// distance. `max_metric_projection_ratio` is the maximum of that exact coefficient over the
/// block's receiver rows. For a displaced receiver the line distance is at most the block's
/// centre-to-segment distance plus its half-diagonal. Clamping the resulting interval to the
/// physical segment and taking its lower altitude avoids letting a remote low endpoint poison
/// a high closest point.
#[inline]
pub fn lowest_physical_source_altitude(
    start_alt_m: f64,
    delta_alt_m: f64,
    centre_projection: f64,
    segment_length_m: f64,
    centre_to_segment_m: f64,
    max_metric_projection_ratio: f64,
    block_half_diagonal_m: f64,
) -> f64 {
    if segment_length_m <= 0.0 {
        return start_alt_m;
    }
    let rel = LOWEST_SOURCE_TANGENT_MARGIN_REL;
    let metric_shift_m =
        (centre_to_segment_m + block_half_diagonal_m) * max_metric_projection_ratio;
    let half_t = ((block_half_diagonal_m + metric_shift_m + LOWEST_SOURCE_TANGENT_RANGE_MARGIN_M)
        / segment_length_m)
        * (1.0 + rel);
    let t_lo = (centre_projection - half_t).clamp(0.0, 1.0);
    let t_hi = (centre_projection + half_t).clamp(0.0, 1.0);
    (start_alt_m + t_lo * delta_alt_m).min(start_alt_m + t_hi * delta_alt_m)
}

/// Sharp coefficient for the along-line projection shift caused by scaling longitude by `q`
/// relative to the block-centre metric. Decomposing the receiver-to-line vector into along-
/// and cross-line parts leaves the cross-line coefficient
/// `|(q^2 - 1) sin(a) cos(a)| / (q^2 cos(a)^2 + sin(a)^2)`, whose maximum over line direction
/// `a` is `|q - 1/q| / 2`.
#[inline]
pub fn longitude_metric_projection_ratio(q: f64) -> f64 {
    0.5 * (q - q.recip()).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_roof_below_the_lowest_line_of_sight_cannot_screen() {
        // The owner's example: a 3 m roof one metre away, ear at 4 m — it cannot screen an
        // aircraft that is never below the horizontal.
        assert!(horizon_edge_cannot_screen(-1.0, 1.0, 0.0));
        // A 100 m building 100 m away shades anything below 45°.
        assert!(!horizon_edge_cannot_screen(96.0, 100.0, 0.5));
        // The same building cannot shade an aircraft steeper than itself.
        assert!(horizon_edge_cannot_screen(96.0, 100.0, 1.0));
    }

    #[test]
    fn the_nearest_range_of_a_cell_is_safe_only_for_a_non_negative_floor() {
        // With a non-negative floor the inequality tightens as the range shrinks, so proving
        // it at the cell's nearest point proves it everywhere in the cell.
        assert!(horizon_edge_cannot_screen(4.0, 100.0, 0.05));
        assert!(horizon_edge_cannot_screen(4.0, 200.0, 0.05));
    }

    #[test]
    fn the_block_bound_is_below_every_receiver_tangent_it_covers() {
        // One sub-segment 500 m from the block centre, 60 m above the block's highest
        // receiver; a receiver anywhere in a 16-pixel block (half-diagonal 70 m) sees a
        // tangent of at least the bound.
        let bound = lowest_source_tangent(160.0, 100.0, 500.0, 70.0);
        assert!(bound <= 60.0 / 570.0, "{bound}");
        assert!(bound > 60.0 / 580.0, "{bound}");
    }

    #[test]
    fn a_source_below_the_block_divides_by_the_nearest_distance() {
        // A negative numerator is most negative at the closest receiver, so the bound must
        // use the block's near edge, not its far edge.
        let bound = lowest_source_tangent(40.0, 100.0, 500.0, 70.0);
        assert!(bound <= -60.0 / 430.0, "{bound}");
        assert!(bound > -60.0 / 420.0, "{bound}");
    }

    #[test]
    fn physical_source_altitude_uses_only_the_reachable_projection_interval() {
        let lowest = lowest_physical_source_altitude(100.0, 500.0, 0.6, 1_000.0, 500.0, 0.0, 50.0);
        // The 0 m endpoint is not a possible closest point for this receiver block.
        assert!(lowest > 370.0, "{lowest}");
        // Both extreme receiver projections remain above the conservative bound.
        for receiver_t in [0.55, 0.6, 0.65] {
            assert!(
                lowest <= 100.0 + receiver_t * 500.0,
                "{lowest} {receiver_t}"
            );
        }

        let descending =
            lowest_physical_source_altitude(600.0, -500.0, 0.6, 1_000.0, 500.0, 0.0, 50.0);
        for receiver_t in [0.55, 0.6, 0.65] {
            assert!(
                descending <= 600.0 - receiver_t * 500.0,
                "{descending} {receiver_t}"
            );
        }
    }

    #[test]
    fn physical_source_altitude_covers_receiver_row_metric_distortion() {
        // A 0.1% anisotropic longitude-scale change can move the projection of a line 5 km
        // away by almost 5 m along that line. The sharp coefficient includes that displacement
        // in addition to the receiver block's own 50 m half-diagonal.
        let ratio = longitude_metric_projection_ratio(1.001);
        assert!((ratio - 0.000_999_500_499_4).abs() < 1e-12, "{ratio}");
        let lowest =
            lowest_physical_source_altitude(100.0, 500.0, 0.6, 1_000.0, 5_000.0, ratio, 50.0);
        assert!(lowest <= 100.0 + 0.545 * 500.0, "{lowest}");
    }
}
