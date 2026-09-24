//! Measured carriageway priors for classes 0-4. PROVISIONAL: w3-major pilot fit (r260919 training squares, before the
//! slice-1 adapter fixes; the unknown built-up column keeps the r260910 values). Regenerate with
//! pipeline/fit-road-traffic-priors.ts --write on a finalized slice-1 world before release.

use crate::defaults::CarriagewayPrior;

const fn p(vehicles_per_lane: f64, untagged: f64) -> CarriagewayPrior {
    CarriagewayPrior { vehicles_per_lane, untagged }
}

/// `[class][one-way, two-way][built-up unknown, rural, urban]`.
pub const MEASURED_CARRIAGEWAY_PRIORS: [[[CarriagewayPrior; 3]; 2]; 5] = [
    [[p(6379.0, 5200.0), p(5972.0, 5208.0), p(11464.0, 19372.0)], [p(3010.0, 6019.0), p(2914.0, 6297.0), p(3328.0, 7640.0)]],
    [[p(4533.0, 1810.0), p(4651.0, 5834.0), p(5642.0, 9588.0)], [p(2594.0, 3045.0), p(2736.0, 4628.0), p(4504.0, 8166.0)]],
    [[p(4250.0, 5882.0), p(3180.0, 4440.0), p(4913.0, 7504.0)], [p(2800.0, 3719.0), p(2406.0, 3652.0), p(4067.0, 8034.0)]],
    [[p(0.0, 5166.0), p(0.0, 5166.0), p(0.0, 8822.0)], [p(0.0, 3130.0), p(0.0, 2456.0), p(0.0, 5294.0)]],
    [[p(0.0, 2409.0), p(0.0, 2409.0), p(0.0, 4577.0)], [p(0.0, 2040.0), p(0.0, 1748.0), p(0.0, 2774.0)]],
];
