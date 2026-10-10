//! A block's groups: the loud sources it does not evaluate at every pixel, grouped by layer,
//! direction and distance around a frame's centre (a far cell's centre). A group's energy is known
//! exactly where its members were evaluated: its near share at the block's corners, its far share
//! at the four points of the block's far cell. A pixel blends both and scales them by one exact
//! ray from the group's loudest member (its probe): the shadow that member meets there. Every block
//! of a far cell groups alike and a pixel near the cell's edge blends the frames of the cells on
//! both sides, so the grouping never switches at an edge (no seams in the map).

use crate::exact::LAYERS;
use crate::paint::bilinear;
use crate::square::Square;
use std::collections::HashMap;

/// A group's directions (degrees wide) and distances (shells doubling from this far) from its
/// frame's centre, and the share of its layer's energy at some of the frame's points from which it
/// gets a probe (the others are blended alone).
const GROUP_SECTOR_DEG: f64 = 10.0;
const GROUP_FIRST_SHELL_M: f64 = 100.0;
const PROBE_SHARE: f64 = 0.003;

/// Each source's energy at four points (north-west, north-east, south-west, south-east), each list
/// sorted by source.
pub(crate) type Four<'a> = [&'a [(u32, f64)]; 4];

/// A group's loudest member and its shares where the group's are known: its near share at the
/// block's corners, its far share at its far cell's points.
pub(crate) struct Probe {
    pub index: u32,
    pub corners: [f64; 4],
    pub cell: [f64; 4],
}

/// A group with a probe: its layer and its members' near shares summed at the block's corners and
/// far shares at its far cell's points.
pub(crate) struct Group {
    pub layer: usize,
    pub corners: [f64; 4],
    pub cell: [f64; 4],
    pub probe: Probe,
}

/// A block's groups around one frame's centre: those with a probe, and per point and layer the
/// shares of those without.
pub(crate) struct Frame {
    /// The frame's far cell, in cells from the block's own.
    pub offset: [i64; 2],
    pub probed: Vec<Group>,
    pub rest_corners: [[f64; LAYERS]; 4],
    pub rest_cell: [[f64; LAYERS]; 4],
}

/// Whether the piece `a`-`b` meets the rectangle `low`-`high` (Liang-Barsky).
pub(crate) fn meets(a: [f64; 2], b: [f64; 2], low: [f64; 2], high: [f64; 2]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = [b[0] - a[0], b[1] - a[1]];
    for axis in 0..2 {
        for (p, q) in [
            (-d[axis], a[axis] - low[axis]),
            (d[axis], high[axis] - a[axis]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
    }
    t0 <= t1
}

/// One group being summed: its shares, its energy at the frame's points and its probe so far.
struct Summing {
    layer: usize,
    corners: [f64; 4],
    cell: [f64; 4],
    at_frame: [f64; 4],
    probe: Option<(f64, Probe)>,
}

/// The groups around `centre` of every source in `corners`, `cell` or `frame` (the frame's own far
/// cell points) that `near` admits: `near` gives a source's near share, `None` for a source the
/// block evaluates at every pixel. A group holding at least `PROBE_SHARE` of its layer
/// (`frame_totals`) at some frame point gets as probe its member loudest at the frame's points
/// among those known at every point their shares are blended from.
pub(crate) fn frame(
    square: &Square,
    (corners, cell, frame): (Four, Four, Four),
    near: &dyn Fn(u32) -> Option<f64>,
    (centre, offset): ([f64; 2], [i64; 2]),
) -> Frame {
    let lists: [&[(u32, f64)]; 12] = std::array::from_fn(|k| match k / 4 {
        0 => corners[k % 4],
        1 => cell[k % 4],
        _ => frame[k % 4],
    });
    let mut cursors = [0usize; 12];
    let mut groups: HashMap<(usize, i64, i64), Summing> = HashMap::new();
    let mut totals = [[0.0; LAYERS]; 4];
    while let Some(index) = (0..12)
        .filter_map(|k| lists[k].get(cursors[k]).map(|e| e.0))
        .min()
    {
        let mut energies = [0.0; 12];
        for k in 0..12 {
            if let Some(&(at, energy)) = lists[k].get(cursors[k])
                && at == index
            {
                energies[k] = energy;
                cursors[k] += 1;
            }
        }
        let Some(share) = near(index) else { continue };
        let candidate = &square.candidates[index as usize];
        let [a, b] = candidate.ends_m;
        let (dx, dy) = (
            (a[0] + b[0]) / 2.0 - centre[0],
            (a[1] + b[1]) / 2.0 - centre[1],
        );
        let sector = (dy.atan2(dx).to_degrees().rem_euclid(360.0) / GROUP_SECTOR_DEG) as i64;
        let shell = (dx.hypot(dy) / GROUP_FIRST_SHELL_M).max(1.0).log2() as i64;
        let layer = candidate.layer as usize;
        let group = groups.entry((layer, sector, shell)).or_insert(Summing {
            layer,
            corners: [0.0; 4],
            cell: [0.0; 4],
            at_frame: [0.0; 4],
            probe: None,
        });
        let at_corners: [f64; 4] = std::array::from_fn(|k| energies[k]);
        let at_cell: [f64; 4] = std::array::from_fn(|k| energies[4 + k]);
        // Where it is decided: a near share at the block's corners, a far share at the frame's.
        let decide: [f64; 4] =
            std::array::from_fn(|k| share * at_corners[k] + (1.0 - share) * energies[8 + k]);
        for k in 0..4 {
            group.corners[k] += share * at_corners[k];
            group.cell[k] += (1.0 - share) * at_cell[k];
            group.at_frame[k] += decide[k];
            totals[k][layer] += decide[k];
        }
        let known = (share == 0.0 || at_corners.iter().all(|&e| e > 0.0))
            && (share == 1.0 || at_cell.iter().all(|&e| e > 0.0));
        let loudness: f64 = decide.iter().sum();
        if known
            && group
                .probe
                .as_ref()
                .is_none_or(|(best, _)| loudness > *best)
        {
            group.probe = Some((
                loudness,
                Probe {
                    index,
                    corners: at_corners.map(|energy| share * energy),
                    cell: at_cell.map(|energy| (1.0 - share) * energy),
                },
            ));
        }
    }
    let mut out = Frame {
        offset,
        probed: Vec::new(),
        rest_corners: [[0.0; LAYERS]; 4],
        rest_cell: [[0.0; LAYERS]; 4],
    };
    for group in groups.into_values() {
        let heard = |k: usize| {
            group.at_frame[k] > 0.0 && group.at_frame[k] >= PROBE_SHARE * totals[k][group.layer]
        };
        match group.probe {
            Some((_, probe)) if (0..4).any(heard) => out.probed.push(Group {
                layer: group.layer,
                corners: group.corners,
                cell: group.cell,
                probe,
            }),
            _ => {
                for k in 0..4 {
                    out.rest_corners[k][group.layer] += group.corners[k];
                    out.rest_cell[k][group.layer] += group.cell[k];
                }
            }
        }
    }
    out
}

/// A group's energy at a pixel, `(fx, fy)` within its block and `(lx, ly)` within its far cell:
/// both shares blended, scaled by its probe's energy `here` at the pixel against the probe's blend.
pub(crate) fn group_energy(
    group: &Group,
    here: f64,
    (fx, fy): (f64, f64),
    (lx, ly): (f64, f64),
) -> f64 {
    let blended = bilinear(group.corners, fx, fy) + bilinear(group.cell, lx, ly);
    let probe = &group.probe;
    let probe_blended = bilinear(probe.corners, fx, fy) + bilinear(probe.cell, lx, ly);
    if probe_blended > 0.0 {
        blended * here / probe_blended
    } else {
        blended
    }
}

/// The weight of the frame `offset` cells from a pixel's own far cell, the pixel `(tx, ty)` pixels
/// (its centre) into a cell `side` pixels wide: its own cell's frame alone but within `band` pixels
/// of an edge, where the frames on both sides blend linearly across the edge.
pub(crate) fn frame_weight(offset: [i64; 2], (tx, ty): (f64, f64), side: f64, band: f64) -> f64 {
    let along = |t: f64, offset: i64| {
        let own = if t < band {
            (t + band) / (2.0 * band)
        } else if t > side - band {
            (side - t + band) / (2.0 * band)
        } else {
            1.0
        };
        match offset {
            0 => own,
            -1 if t < band => 1.0 - own,
            1 if t > side - band => 1.0 - own,
            _ => 0.0,
        }
    };
    along(tx, offset[0]) * along(ty, offset[1])
}
