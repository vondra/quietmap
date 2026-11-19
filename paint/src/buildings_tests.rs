//! A building's level: the loudest façade by the ground sources, and the receivers a pass need not sound.

use super::*;

/// A building takes its candidate loudest from the ground sources, not the one loudest with
/// its flights, and every one of its pixels takes it; a building without a painted candidate
/// keeps no level.
#[test]
fn a_building_reads_its_loudest_facade() {
    let aircraft = Layer::Aircraft as usize;
    let mut loud = [1000.0; LAYERS];
    loud[aircraft] = 1.0;
    let mut overflown = [10.0; LAYERS];
    overflown[aircraft] = 1e9;
    let levels = loudest_by_ground(
        [
            (7, Some((overflown, [1.0; LAYERS + 1]))),
            (7, Some((loud, [8.0; LAYERS + 1]))),
            (9, None),
        ]
        .into_iter(),
    );
    let mut pixels: Vec<Option<Painted>> = vec![None; 6];
    let indoor = [(1, 7), (2, 7), (4, 9)];
    fill_buildings(&mut pixels, &indoor, &levels);
    assert_eq!(pixels[1], Some((loud, [8.0; LAYERS + 1])));
    assert_eq!(pixels[2], pixels[1]);
    assert!(pixels[4].is_none() && pixels[0].is_none());
}

/// The receivers a pass does not sound are those no building reads: every building reads the
/// same receiver with them unpainted, wherever the passes cut its receivers apart, equal
/// energies to the first, an energy that is not a number passed over as before.
#[test]
fn a_building_reads_the_same_facade_with_the_others_of_each_pass_unpainted() {
    let footprints = [7, 7, 9, 7, 9, 5, 9, 4, 4, 4];
    let ground = [1.0, 3.0, 2.0, 3.0, 2.0, 0.0, 2.0, 1.0, f64::NAN, 2.0];
    let paint = |k: usize| -> Painted {
        let mut layers = [0.0; LAYERS];
        layers[Layer::Road as usize] = ground[k];
        (layers, [k as f64; LAYERS + 1])
    };
    assert_eq!(
        loudest_receivers(&footprints[..7], &ground[..7]),
        [false, true, true, false, false, true, false]
    );
    // Each building with the receiver it reads, by the receiver's number.
    let read = |levels: HashMap<u64, Painted>| -> Vec<(u64, f64)> {
        let mut read: Vec<_> = (levels.into_iter())
            .map(|(footprint, paint)| (footprint, paint.1[0]))
            .collect();
        read.sort_by_key(|&(footprint, _)| footprint);
        read
    };
    let all = read(loudest_by_ground(
        (0..10).map(|k| (footprints[k], Some(paint(k)))),
    ));
    assert_eq!(all, [(4, 9.0), (5, 5.0), (7, 1.0), (9, 2.0)]);
    for cut in 0..=10 {
        let kept = [0..cut, cut..10].into_iter().flat_map(|pass| {
            let loudest = loudest_receivers(&footprints[pass.clone()], &ground[pass.clone()]);
            pass.zip(loudest)
                .map(|(k, loudest)| (footprints[k], loudest.then(|| paint(k))))
        });
        assert_eq!(read(loudest_by_ground(kept)), all, "passes cut at {cut}");
    }
}
