//! Regression checks for a corner's choice of traced sources, the transfers by direction and what a block leaves out.

use super::*;
use crate::corner_sources::sector_of;
use crate::exact::PairEnergy;
use physics::bands::lden_energy;
use tiles::sources::Layer;

const EAST: u8 = 8;
const NORTH: u8 = 12;

/// The energy of every period that makes an Lden energy of `lden`.
fn even(lden: f64) -> f64 {
    lden / lden_energy(&[1.0; PERIODS])
}

/// A corner of `layer` sources (index, Lden energy, direction), each its own group, the same
/// energy in every period and state, its index in metres away; ranked and its traced sources
/// chosen, sources from index 90 near.
fn corner(layer: Layer, sources: &[(u32, f64, u8)]) -> Corner {
    let mut corner = Corner::default();
    for &(index, lden, sector) in sources {
        let energy = PairEnergy {
            mixed: [even(lden); PERIODS],
            states: [[even(lden); PERIODS]; 2],
        };
        let source = (index, u64::from(index));
        corner.add((layer as usize, sector), source, (energy, f64::from(index)));
    }
    for ranked in &mut corner.ranked {
        ranked.sort_unstable_by(|a, b| b.lden.total_cmp(&a.lden).then(a.index.cmp(&b.index)));
    }
    corner.choose_traced(|index| index >= 90);
    corner
}

#[test]
fn the_directions_are_sixteen_from_the_west_anticlockwise() {
    assert_eq!(sector_of([1.0, 0.2]), EAST);
    assert_eq!(sector_of([-0.2, 1.0]), NORTH);
    assert_eq!(sector_of([-1.0, -0.2]), 0);
    assert_eq!(sector_of([-1.0, 0.2]), 15);
}

/// A road's corner traces the loudest far source of each direction, then its loudest until half
/// the layer and half its far sources are traced: the two directions' loudest (1, 3) leave 32 of
/// the far 100 untraced, within its half, and the near source (90) brings the layer within
/// its half. One direction of three far sources of 40, 35 and 25 takes the second too. A
/// layer under 25 dB keeps none.
#[test]
fn a_roads_corner_traces_each_directions_loudest_and_half_of_its_layer() {
    let k = 1000.0;
    let two = corner(
        Layer::Road,
        &[
            (1, 60.0 * k, EAST),
            (2, 30.0 * k, EAST),
            (3, 8.0 * k, NORTH),
            (4, 2.0 * k, NORTH),
            (90, 100.0 * k, EAST),
        ],
    );
    assert_eq!(two.traced, vec![1, 3, 90]);
    let one = corner(
        Layer::Road,
        &[
            (1, 40.0 * k, EAST),
            (2, 35.0 * k, EAST),
            (3, 25.0 * k, EAST),
        ],
    );
    assert_eq!(one.traced, vec![1, 2]);
    let faint = corner(Layer::Road, &[(1, 200.0, EAST), (2, 100.0, NORTH)]);
    assert!(faint.traced.is_empty());
}

/// A direction reaches a pixel as the block's far sources of it do against the corners' blend
/// of them: heard at a quarter, a quarter; one the pixel does not trace takes its layer's;
/// louder than the blend at most fourfold; the block's own source says nothing of its
/// direction; corners in a shadow weigh in the blend with what they hear, corners beyond a
/// source's reach not at all.
#[test]
fn a_direction_reaches_a_pixel_as_the_blocks_far_sources_of_it() {
    let k = 1000.0;
    let sources = [
        (1, 60.0 * k, EAST),
        (3, 40.0 * k, NORTH),
        (90, 200.0 * k, 0),
    ];
    let corner = corner(Layer::Road, &sources);
    assert_eq!(corner.traced, vec![1, 3, 90]);
    let block = admit(vec![90], [&corner; 4]);
    let far: Vec<u32> = block.far.iter().map(|far| far.index).collect();
    assert_eq!(far, vec![1, 3]);
    let road = Layer::Road as usize;
    let even = [0.25; 4];
    let reach = block.transfers(even, |index| (index == 1).then_some(15.0 * k));
    let quarter = |reach: f64| (reach - 0.25).abs() < 1e-12;
    assert!(quarter(reach[0][road][EAST as usize]));
    assert!(quarter(reach[3][road][NORTH as usize]));
    assert!(quarter(reach[1][Layer::Railway as usize][0]));
    let loud = |index: u32| Some(if index == 3 { 400.0 * k } else { 0.0 });
    let reach = block.transfers(even, loud);
    assert_eq!(reach[2][road][NORTH as usize], 4.0);
    assert_eq!(reach[2][road][EAST as usize], 0.0);
    assert_eq!(block.transfers(even, |_| None)[0][road][0], 1.0);
    // Two corners in a shadow hear the east at a thousandth: the lit pixel hears the far source
    // as the lit corners do, twice the corners' blend, and what they leave out with it.
    let dim = [
        (1, 0.06 * k, EAST),
        (3, 0.04 * k, NORTH),
        (90, 200.0 * k, 0),
    ];
    let shadowed = self::corner(Layer::Road, &dim);
    let across = admit(vec![90], [&corner, &corner, &shadowed, &shadowed]);
    let reach = across.transfers(even, |index| (index == 1).then_some(60.0 * k));
    assert!((reach[0][road][EAST as usize] - 60.0 / 30.03).abs() < 1e-12);
    // Two corners beyond the far source's reach know nothing of it: the pixel hears it as the
    // corners that hear it do, and their weight does not dilute their blend.
    let beyond = self::corner(Layer::Road, &[(90, 200.0 * k, 0)]);
    let edge = admit(vec![90], [&corner, &corner, &beyond, &beyond]);
    let reach = edge.transfers(even, |index| (index == 1).then_some(60.0 * k));
    assert!((reach[0][road][EAST as usize] - 1.0).abs() < 1e-12);
}

/// A block's pixels trace its own sources and its corners'; what the corners hear beside
/// them is left out by direction, and each group's weather bins hold exactly its share.
#[test]
fn a_block_leaves_out_what_its_corners_do_not_trace_by_direction() {
    let k = 1000.0;
    let sources = [
        (1, 60.0 * k, EAST),
        (2, 30.0 * k, EAST),
        (3, 8.0 * k, NORTH),
        (4, 2.0 * k, NORTH),
    ];
    let corner = corner(Layer::Road, &sources);
    assert_eq!(corner.traced, vec![1, 3]);
    let block = admit(vec![7], [&corner; 4]);
    assert_eq!(block.exact, vec![1, 3, 7]);
    // A deeper trace would add the second: the most a pixel may trace is four.
    assert_eq!(
        (block.most, admission_counts(vec![7], [&corner; 4])),
        (4, (4, true))
    );
    let road = Layer::Road as usize;
    for at_corner in block.left_out.iter() {
        assert_eq!(at_corner[road][EAST as usize], [even(30.0 * k); PERIODS]);
        assert_eq!(at_corner[road][NORTH as usize], [even(2.0 * k); PERIODS]);
    }
    let keys: Vec<u64> = block.left_groups.iter().map(|group| group.key).collect();
    assert_eq!(keys, vec![2, 4]);
    let second = &block.left_groups[0];
    assert_eq!((second.loudest, second.nearest), (2, [2; 4]));
    let bins = &block.left_bins[second.bins.start as usize..second.bins.end as usize];
    assert_eq!(bins.len(), 4 * PERIODS);
    assert!((bins.iter()).all(|bin| bin.sector == EAST && bin.values == [even(30.0 * k); 3]));
    let all = admit(vec![1, 2, 3, 4], [&corner; 4]);
    assert!(all.left_groups.is_empty() && all.left_bins.is_empty());
    assert_eq!(admission_counts(vec![1, 2, 3, 4], [&corner; 4]), (4, false));
}

/// A pixel tracing deeper adds each corner's loudest until what the corner has not traced is a
/// fifth of the block's quietest corner's layer, a twentieth for a railway: of five far roads of
/// 40, 30, 15, 10 and 5 the corner traces the first two (30 of 100 are left: within its half),
/// the deeper list the first three (15 are left); of five such railway pieces the corner the
/// first three (within its fifth), the deeper list all but the last. The block bounds its pixels
/// by the deeper list's count, and the deeper block leaves out what is left.
#[test]
fn a_deeper_trace_leaves_a_fifth_of_the_quietest_corners_layer_and_a_twentieth_of_a_railways() {
    let k = 1000.0;
    let sources = [
        (1, 40.0 * k, EAST),
        (2, 30.0 * k, EAST),
        (3, 15.0 * k, EAST),
        (4, 10.0 * k, EAST),
        (5, 5.0 * k, EAST),
    ];
    for (layer, traced, deeper) in [
        (Layer::Road, vec![1, 2], vec![1, 2, 3]),
        (Layer::Railway, vec![1, 2, 3], vec![1, 2, 3, 4]),
    ] {
        let corner = corner(layer, &sources);
        assert_eq!(corner.traced, traced);
        let block = admit(vec![], [&corner; 4]);
        assert_eq!((block.exact.clone(), block.most), (traced, deeper.len()));
        assert_eq!(admission_counts(vec![], [&corner; 4]), (deeper.len(), true));
        let deep = admit_deeper(&block, [&corner; 4]);
        assert_eq!(
            (deep.exact.clone(), deep.most),
            (deeper.clone(), deeper.len())
        );
        let left: f64 = (deep.left_out[0][layer as usize].iter())
            .map(lden_energy)
            .sum();
        let expected: f64 = (sources.iter())
            .filter(|(index, _, _)| !deeper.contains(index))
            .map(|&(_, lden, _)| lden)
            .sum();
        assert!((left / expected - 1.0).abs() < 1e-12);
    }
    // One corner hearing the same roads at half: the fifth is of its 50, so the louder corners
    // trace down to 10 left, the fourth road too.
    let halved: Vec<(u32, f64, u8)> = (sources.iter())
        .map(|&(index, lden, sector)| (index, lden / 2.0, sector))
        .collect();
    let (loud, quiet) = (corner(Layer::Road, &sources), corner(Layer::Road, &halved));
    assert_eq!(quiet.traced, vec![1, 2]);
    let block = admit(vec![], [&loud, &loud, &loud, &quiet]);
    assert_eq!((block.exact.clone(), block.most), (vec![1, 2], 4));
    let deep = admit_deeper(&block, [&loud, &loud, &loud, &quiet]);
    assert_eq!(deep.exact, vec![1, 2, 3, 4]);
}

/// A pixel asks a deeper trace where the transfers change more than three tenths of what it
/// hears of its loud sources: a direction heard at a quarter of the corners' blend moves three
/// quarters of what the corners leave out of it.
#[test]
fn a_pixel_asks_a_deeper_trace_where_the_transfers_change_much_of_its_sound() {
    let k = 1000.0;
    let sources = [(1, 60.0 * k, EAST), (2, 40.0 * k, EAST)];
    let corner = corner(Layer::Road, &sources);
    assert_eq!(corner.traced, vec![1]);
    let block = admit(vec![], [&corner; 4]);
    let even = [0.25; 4];
    let lit = block.reaching(even, |_| Some(60.0 * k));
    assert!(!lit.asks_deeper(60.0 * k));
    // Heard at 15: 40 left out arrive as 10 and 30 were moved, of 15 + 10 heard.
    let shadowed = block.reaching(even, |_| Some(15.0 * k));
    assert!(shadowed.asks_deeper(15.0 * k));
    // Beside 200 of its own sources the same 30 are less than three tenths.
    assert!(!shadowed.asks_deeper(215.0 * k));
}

/// A group left out in two directions keeps each direction's energy in its own bins, and under
/// any transfers its bins hold exactly what the layer's left-out energy holds of it: its line
/// leaves no steady remainder.
#[test]
fn a_left_groups_bins_hold_what_the_layer_leaves_out_of_it_by_direction() {
    let k = 1000.0;
    let mut corner = Corner::default();
    let piece = |lden: f64, favourable: f64| PairEnergy {
        mixed: [even(lden); PERIODS],
        states: [
            [even(lden) / 2.0; PERIODS],
            [even(lden) * favourable; PERIODS],
        ],
    };
    let road = Layer::Road as usize;
    // Two traced far sources, and one group's two pieces left out, one in either direction.
    corner.add((road, EAST), (1, 1), (piece(60.0 * k, 2.0), 100.0));
    corner.add((road, NORTH), (2, 2), (piece(50.0 * k, 2.0), 100.0));
    corner.add((road, EAST), (3, 7), (piece(20.0 * k, 3.0), 300.0));
    corner.add((road, NORTH), (4, 7), (piece(10.0 * k, 5.0), 200.0));
    for ranked in &mut corner.ranked {
        ranked.sort_unstable_by(|a, b| b.lden.total_cmp(&a.lden).then(a.index.cmp(&b.index)));
    }
    corner.choose_traced(|_| false);
    assert_eq!(corner.traced, vec![1, 2]);
    let block = admit(vec![], [&corner; 4]);
    assert_eq!(block.left_groups.len(), 1);
    let group = &block.left_groups[0];
    assert_eq!((group.key, group.loudest, group.nearest), (7, 3, [4; 4]));
    let bins = &block.left_bins[group.bins.start as usize..group.bins.end as usize];
    assert_eq!(bins.len(), 4 * 2 * PERIODS);
    // The east heard at a half of the corners' blend, the north at twice it.
    let heard = |index: u32| Some(if index == 1 { 30.0 * k } else { 100.0 * k });
    let reaching = block.reaching([0.25; 4], heard);
    for period in 0..PERIODS {
        let of_group: f64 = (bins.iter())
            .filter(|bin| bin.period as usize == period)
            .map(|bin| {
                reaching.transfers[bin.corner as usize][road][bin.sector as usize] * bin.values[2]
            })
            .sum();
        let of_layer: f64 = reaching.energy[road][period].iter().sum();
        assert!((of_group / of_layer - 1.0).abs() < 1e-12);
        assert!((of_layer / (even(20.0 * k) * 0.5 + even(10.0 * k) * 2.0) - 1.0).abs() < 1e-12);
    }
}
