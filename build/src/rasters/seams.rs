//! Removes tile-seam trenches and nodata-bleed sinkholes from a terrain model before blending.
use super::dem::CODE_CEILING_M;

fn is_sample(height: f64) -> bool {
    height.is_finite() && height <= CODE_CEILING_M
}

/// Lowest dry land on Earth (the Dead Sea shore); narrow voids below it are seeded without a
/// relative-depth check.
const GROUND_FLOOR_M: f64 = -431.0;

/// Global 10^-7 quantile of 3-node ring drop (149.8 m across 1.12 billion land nodes).
const TRENCH_DROP_M: f64 = 150.0;

/// Global 10^-7 quantile of local 7x7 drop (100.4 m); as a one-node step it separates all seam
/// walls (>= 108.4 m) from the steepest terraced open-pit mine benches (<= 60.6 m).
const WALL_STEP_M: f64 = 100.0;

const WIDE_RADIUS: usize = 10;
const NARROW_RADIUS: usize = 3;
const HALO_RADIUS: usize = 6;
const CLOSED_RIM_MARGIN_M: f64 = 15.0;
const NARROW_RIM_MARGIN_M: f64 = 20.0;
const MAX_CLOSED_CELLS: usize = 2_500;
const BLOCK_SIDE: usize = 16;

const AXES: [(isize, isize); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
const NEIGHBOURS_8: [(isize, isize); 8] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
];

/// Minimum and 3x3-neighbourhood maximum heights over 16x16 blocks; since WIDE_RADIUS <= 16, a
/// block whose 3x3 neighbourhood rises by less than the drop threshold holds no candidate cell.
struct BlockBounds {
    brows: usize,
    bcols: usize,
    min_h: Vec<f64>,
    max_3x3: Vec<f64>,
}

impl BlockBounds {
    fn compute(heights: &[f64], rows: usize, columns: usize) -> Self {
        let (brows, bcols) = (rows.div_ceil(BLOCK_SIDE), columns.div_ceil(BLOCK_SIDE));
        let mut min_h = vec![f64::INFINITY; brows * bcols];
        let mut max_h = vec![f64::NEG_INFINITY; brows * bcols];
        for r in 0..rows {
            let br = r / BLOCK_SIDE;
            for (bc, chunk) in heights[r * columns..(r + 1) * columns]
                .chunks(BLOCK_SIDE)
                .enumerate()
            {
                let bidx = br * bcols + bc;
                let (mut lo, mut hi) = (min_h[bidx], max_h[bidx]);
                for &h in chunk {
                    if is_sample(h) {
                        lo = lo.min(h);
                        hi = hi.max(h);
                    }
                }
                min_h[bidx] = lo;
                max_h[bidx] = hi;
            }
        }
        let mut max_3x3 = vec![f64::NEG_INFINITY; brows * bcols];
        for br in 0..brows {
            for bc in 0..bcols {
                let mut hi = f64::NEG_INFINITY;
                for nr in br.saturating_sub(1)..=(br + 1).min(brows - 1) {
                    for nc in bc.saturating_sub(1)..=(bc + 1).min(bcols - 1) {
                        hi = hi.max(max_h[nr * bcols + nc]);
                    }
                }
                max_3x3[br * bcols + bc] = hi;
            }
        }
        Self {
            brows,
            bcols,
            min_h,
            max_3x3,
        }
    }

    fn active(&self, br: usize, bc: usize, drop_threshold: f64) -> bool {
        let bidx = br * self.bcols + bc;
        self.min_h[bidx] < GROUND_FLOOR_M || self.max_3x3[bidx] - self.min_h[bidx] >= drop_threshold
    }
}

fn step_coord(
    rows: usize,
    columns: usize,
    r: usize,
    c: usize,
    dr: isize,
    dc: isize,
) -> Option<usize> {
    let (nr, nc) = (r as isize + dr, c as isize + dc);
    (nr >= 0 && nr < rows as isize && nc >= 0 && nc < columns as isize)
        .then(|| nr as usize * columns + nc as usize)
}

fn axial_rim(heights: &[f64], rows: usize, columns: usize, r: usize, c: usize, rad: usize) -> f64 {
    AXES.into_iter()
        .map(|(dr, dc)| {
            let side = |s: isize| {
                (1..=rad as isize)
                    .filter_map(|k| step_coord(rows, columns, r, c, s * dr * k, s * dc * k))
                    .map(|idx| heights[idx])
                    .filter(|&h| is_sample(h))
                    .fold(f64::NEG_INFINITY, f64::max)
            };
            side(1).min(side(-1))
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

fn axial_drop_at_least(
    heights: &[f64],
    rows: usize,
    columns: usize,
    r: usize,
    c: usize,
    rad: usize,
    threshold: f64,
) -> bool {
    let idx = r * columns + c;
    let target = heights[idx] + threshold;
    if r >= rad && r + rad < rows && c >= rad && c + rad < columns {
        return AXES.into_iter().any(|(dr, dc)| {
            let stride = dr * columns as isize + dc;
            let reach = |s: isize| {
                (1..=rad as isize).any(|k| {
                    let v = heights[(idx as isize + s * stride * k) as usize];
                    is_sample(v) && v >= target
                })
            };
            reach(1) && reach(-1)
        });
    }
    AXES.into_iter().any(|(dr, dc)| {
        let side = |s: isize| {
            (1..=rad as isize).any(|k| {
                step_coord(rows, columns, r, c, s * dr * k, s * dc * k)
                    .is_some_and(|i| is_sample(heights[i]) && heights[i] >= target)
            })
        };
        side(1) && side(-1)
    })
}

fn gather_window(
    heights: &[f64],
    rows: usize,
    columns: usize,
    (r, c, rad): (usize, usize, usize),
    out: &mut Vec<f64>,
) {
    out.clear();
    for wr in r.saturating_sub(rad)..=(r + rad).min(rows - 1) {
        for wc in c.saturating_sub(rad)..=(c + rad).min(columns - 1) {
            let v = heights[wr * columns + wc];
            if is_sample(v) {
                out.push(v);
            }
        }
    }
}

fn percentile_linear(sorted: &[f64], pct: f64) -> f64 {
    let pos = (pct / 100.0) * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (pos - lo as f64) * (sorted[hi] - sorted[lo])
}

fn median_in_place(values: &mut [f64]) -> f64 {
    let len = values.len();
    let mid = len / 2;
    let (_, &mut upper, _) = values.select_nth_unstable_by(mid, f64::total_cmp);
    if len % 2 == 1 {
        upper
    } else {
        let lower = values[..mid]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        0.5 * (lower + upper)
    }
}

pub(super) fn clean(heights: &mut [f64], rows: usize, columns: usize) {
    if rows <= 2 * NARROW_RADIUS || columns <= 2 * NARROW_RADIUS {
        return;
    }
    let mut bounds = BlockBounds::compute(heights, rows, columns);
    if !(0..bounds.brows).any(|br| (0..bounds.bcols).any(|bc| bounds.active(br, bc, WALL_STEP_M))) {
        return;
    }
    if rows > 2 * WIDE_RADIUS
        && columns > 2 * WIDE_RADIUS
        && clean_closed_depressions(heights, rows, columns, &bounds)
    {
        bounds = BlockBounds::compute(heights, rows, columns);
    }
    clean_narrow_trenches(heights, rows, columns, &bounds);
}

fn clean_closed_depressions(
    heights: &mut [f64],
    rows: usize,
    columns: usize,
    bounds: &BlockBounds,
) -> bool {
    let mut candidates = Vec::new();
    for br in 0..bounds.brows {
        let (r_start, r_end) = (
            (br * BLOCK_SIDE).max(1),
            ((br + 1) * BLOCK_SIDE).min(rows - 1),
        );
        for bc in 0..bounds.bcols {
            if r_start >= r_end || !bounds.active(br, bc, WALL_STEP_M) {
                continue;
            }
            let (c_start, c_end) = (
                (bc * BLOCK_SIDE).max(1),
                ((bc + 1) * BLOCK_SIDE).min(columns - 1),
            );
            let hi = bounds.max_3x3[br * bounds.bcols + bc];
            for r in r_start..r_end {
                let (row_base, above, below) = (r * columns, (r - 1) * columns, (r + 1) * columns);
                for c in c_start..c_end {
                    let h = heights[row_base + c];
                    if !is_sample(h) || hi - h < WALL_STEP_M {
                        continue;
                    }
                    let target = h + WALL_STEP_M;
                    let step1_ok = [
                        heights[row_base + c - 1],
                        heights[row_base + c + 1],
                        heights[above + c - 1],
                        heights[above + c],
                        heights[above + c + 1],
                        heights[below + c - 1],
                        heights[below + c],
                        heights[below + c + 1],
                    ]
                    .into_iter()
                    .any(|nh| is_sample(nh) && nh >= target);
                    if step1_ok
                        && axial_drop_at_least(
                            heights,
                            rows,
                            columns,
                            r,
                            c,
                            WIDE_RADIUS,
                            WALL_STEP_M,
                        )
                    {
                        candidates.push((r, c));
                    }
                }
            }
        }
    }
    if candidates.is_empty() {
        return false;
    }

    candidates.sort_by(|&(r1, c1), &(r2, c2)| {
        heights[r1 * columns + c1].total_cmp(&heights[r2 * columns + c2])
    });
    let (mut fixed, mut failed_ceil) = (Vec::new(), Vec::new());
    let (mut seen_stamp, mut boundary_stamp, mut stamp) = (Vec::new(), Vec::new(), 0u32);
    let (mut comp, mut boundary, mut patch, mut local_rim) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut changed = false;

    for (r0, c0) in candidates {
        let idx0 = r0 * columns + c0;
        if !fixed.is_empty() && fixed[idx0] {
            continue;
        }
        let h0 = heights[idx0];
        let rim_axial = axial_rim(heights, rows, columns, r0, c0, WIDE_RADIUS);
        if rim_axial - h0 < WALL_STEP_M {
            continue;
        }
        gather_window(heights, rows, columns, (r0, c0, WIDE_RADIUS), &mut patch);
        patch.sort_by(f64::total_cmp);
        let rim_h = rim_axial.min(percentile_linear(&patch, 80.0));
        if rim_h - h0 < WALL_STEP_M {
            continue;
        }
        let ceil_h = rim_h - CLOSED_RIM_MARGIN_M;
        if !failed_ceil.is_empty() && ceil_h >= failed_ceil[idx0] {
            continue;
        }
        if fixed.is_empty() {
            fixed = vec![false; rows * columns];
            failed_ceil = vec![f64::INFINITY; rows * columns];
            seen_stamp = vec![0u32; rows * columns];
            boundary_stamp = vec![0u32; rows * columns];
        }

        stamp = stamp.wrapping_add(1);
        comp.clear();
        boundary.clear();
        comp.push((r0, c0));
        seen_stamp[idx0] = stamp;
        let (mut closed, mut min_comp_h, mut head) = (true, h0, 0);

        while head < comp.len() {
            let (r, c) = comp[head];
            head += 1;
            for (dr, dc) in NEIGHBOURS_8 {
                let Some(nidx) = step_coord(rows, columns, r, c, dr, dc) else {
                    closed = false;
                    break;
                };
                if seen_stamp[nidx] == stamp {
                    continue;
                }
                let nh = heights[nidx];
                if !is_sample(nh) {
                    closed = false;
                    break;
                }
                if nh < ceil_h {
                    seen_stamp[nidx] = stamp;
                    min_comp_h = min_comp_h.min(nh);
                    comp.push((nidx / columns, nidx % columns));
                    if comp.len() > MAX_CLOSED_CELLS {
                        closed = false;
                        break;
                    }
                } else if boundary_stamp[nidx] != stamp {
                    boundary_stamp[nidx] = stamp;
                    boundary.push((nidx / columns, nidx % columns, nh));
                }
            }
            if !closed {
                break;
            }
        }

        if closed && !boundary.is_empty() && rim_h - min_comp_h >= TRENCH_DROP_M {
            local_rim.clear();
            local_rim.extend(boundary.iter().map(|&(_, _, h)| h));
            let global_fill = median_in_place(&mut local_rim);
            for &(r, c) in &comp {
                local_rim.clear();
                for &(br, bc, bh) in &boundary {
                    if br.abs_diff(r) <= WIDE_RADIUS && bc.abs_diff(c) <= WIDE_RADIUS {
                        local_rim.push(bh);
                    }
                }
                let fill_value = if local_rim.is_empty() {
                    global_fill
                } else {
                    median_in_place(&mut local_rim)
                };
                fixed[r * columns + c] = true;
                heights[r * columns + c] = fill_value;
            }
            changed = true;
        } else if !closed {
            for &(r, c) in &comp {
                let cell = &mut failed_ceil[r * columns + c];
                *cell = cell.min(ceil_h);
            }
        }
    }
    changed
}

fn clean_narrow_trenches(heights: &mut [f64], rows: usize, columns: usize, bounds: &BlockBounds) {
    let mut cores = Vec::new();
    let mut patch = Vec::with_capacity(169);
    for br in 0..bounds.brows {
        let (r_start, r_end) = (br * BLOCK_SIDE, ((br + 1) * BLOCK_SIDE).min(rows));
        for bc in 0..bounds.bcols {
            if !bounds.active(br, bc, TRENCH_DROP_M) {
                continue;
            }
            let (c_start, c_end) = (bc * BLOCK_SIDE, ((bc + 1) * BLOCK_SIDE).min(columns));
            let hi = bounds.max_3x3[br * bounds.bcols + bc];
            for r in r_start..r_end {
                for c in c_start..c_end {
                    let h = heights[r * columns + c];
                    if !is_sample(h) {
                        continue;
                    }
                    if h < GROUND_FLOOR_M {
                        cores.push((r, c));
                        continue;
                    }
                    if hi - h < TRENCH_DROP_M
                        || (-5.0..=2.0).contains(&h)
                        || !axial_drop_at_least(
                            heights,
                            rows,
                            columns,
                            r,
                            c,
                            NARROW_RADIUS,
                            TRENCH_DROP_M,
                        )
                    {
                        continue;
                    }
                    gather_window(heights, rows, columns, (r, c, NARROW_RADIUS), &mut patch);
                    if patch.len() < 21 {
                        continue;
                    }
                    patch.sort_by(f64::total_cmp);
                    let n = patch.len();
                    if patch[(17 * n) / 49] - h >= TRENCH_DROP_M
                        && h - patch[(4 * n) / 49] <= NARROW_RIM_MARGIN_M
                    {
                        cores.push((r, c));
                    }
                }
            }
        }
    }
    if cores.is_empty() {
        return;
    }

    let mut trench = vec![false; rows * columns];
    let mut rim_local = vec![f64::NAN; rows * columns];
    let mut low_local = vec![f64::INFINITY; rows * columns];
    for &(r, c) in &cores {
        trench[r * columns + c] = true;
        gather_window(heights, rows, columns, (r, c, HALO_RADIUS), &mut patch);
        patch.sort_by(f64::total_cmp);
        let rim_h = percentile_linear(&patch, 75.0);
        let q25_h = percentile_linear(&patch, 25.0);
        for wr in r.saturating_sub(HALO_RADIUS)..=(r + HALO_RADIUS).min(rows - 1) {
            for wc in c.saturating_sub(HALO_RADIUS)..=(c + HALO_RADIUS).min(columns - 1) {
                let idx = wr * columns + wc;
                if rim_local[idx].is_nan() || rim_h > rim_local[idx] {
                    rim_local[idx] = rim_h;
                }
                low_local[idx] = low_local[idx].min(q25_h);
            }
        }
    }

    let mut head = 0;
    while head < cores.len() {
        let (r, c) = cores[head];
        head += 1;
        for (dr, dc) in NEIGHBOURS_8 {
            let Some(nidx) = step_coord(rows, columns, r, c, dr, dc) else {
                continue;
            };
            let nh = heights[nidx];
            if !trench[nidx]
                && is_sample(nh)
                && !((-5.0..=2.0).contains(&nh) && low_local[nidx] <= NARROW_RIM_MARGIN_M)
            {
                let (nr, nc) = (nidx / columns, nidx % columns);
                let rh = rim_local[nidx];
                if rh.is_finite()
                    && rh - nh >= NARROW_RIM_MARGIN_M
                    && axial_drop_at_least(
                        heights,
                        rows,
                        columns,
                        nr,
                        nc,
                        NARROW_RADIUS,
                        NARROW_RIM_MARGIN_M,
                    )
                {
                    trench[nidx] = true;
                    cores.push((nr, nc));
                }
            }
        }
    }

    for &(r, c) in &cores {
        patch.clear();
        for wr in r.saturating_sub(HALO_RADIUS)..=(r + HALO_RADIUS).min(rows - 1) {
            for wc in c.saturating_sub(HALO_RADIUS)..=(c + HALO_RADIUS).min(columns - 1) {
                let widx = wr * columns + wc;
                if !trench[widx] && is_sample(heights[widx]) {
                    patch.push(heights[widx]);
                }
            }
        }
        let idx = r * columns + c;
        heights[idx] = if !patch.is_empty() {
            median_in_place(&mut patch)
        } else {
            rim_local[idx]
        };
    }
}
