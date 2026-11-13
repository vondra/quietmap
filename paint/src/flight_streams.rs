//! The flights' classes of passes as fewer streams for the map: adjacent classes merged with their energy and their loudness while they pass.

use physics::bands::{PERIOD_PENALTY_DB, PERIODS, level_db};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use popup::aircraft::{FlightPasses, PASS_CLASSES};
use popup::loudness::{Curve, FlightSound, curves};
use std::sync::LazyLock;

/// Classes a stream holds: eight, 24 dB of a flight's level while it passes. Near an airport a
/// point hears 36 to 42 of the popup's 3 dB classes, each a line of the card's mixtures where one
/// line stood before: the world sample painted 12.5 % slower with every class a line, 4.3 % with
/// streams of four and 2.4 % with streams of eight. Merged as [`fewer_streams`] does, the
/// aircraft layer alone reads over all its classes by 0.09-0.13 phon with four and by 0.23-0.30
/// with eight at 2,048 points of two airport squares (beside a road as loud 0.02-0.03 and 0.07),
/// nowhere by over half a phon (private evidence 2026-11-12/fable-speed).
const STREAM_CLASSES: usize = 8;
const _: () = assert!(PASS_CLASSES.is_multiple_of(STREAM_CLASSES));

/// Halvings of the span between two of a stream's classes that find its level: 24 dB to 0.02 dB.
const LEVEL_HALVINGS: usize = 10;

/// How loudness grows with the level, for the merge: the loudness curve of a spectrum flat over
/// the third octaves. A merge reads only the curve's growth between its classes' levels, which a
/// spectrum hardly moves (by each point's own flights' curve the layer read 0.02-0.05 phon nearer
/// all its classes), and one curve for the world gives a corner on a square's edge the same
/// streams in both squares.
static GROWTH: LazyLock<Curve> = LazyLock::new(|| {
    let flat = FlightSound {
        energy: [1.0; PERIODS],
        spectrum_db: Some([0.0; CLASS_THIRDS]),
    };
    let [curve, ..] = curves(&[], &flat);
    curve
});

/// `passes` in streams of [`STREAM_CLASSES`] adjacent classes, each at its first class. A stream
/// keeps its classes' energy E and the sum S of their shares of the time times their loudness
/// while passing: one pass at a time, it sounds as loud in the mean as they do. Its level L is
/// where loudness over level is S / E, between the levels of the two classes where that ratio
/// is highest and lowest (S / E is a mean of the classes' ratios, so it lies between them), and
/// its share of the time is E / L. (Keeping the classes' shares instead, a stream at their mean
/// level reads 0.2 phon over them by pairs and 2.6-3.0 by eights: the mean of loudness is under
/// the loudness of the mean.) Classes none of which is heard while passing keep their shares.
pub(crate) fn fewer_streams(passes: &FlightPasses) -> FlightPasses {
    let mut fewer = FlightPasses::default();
    for (p, penalty_db) in PERIOD_PENALTY_DB.into_iter().enumerate() {
        for first in (0..PASS_CLASSES).step_by(STREAM_CLASSES) {
            let classes = first..first + STREAM_CLASSES;
            let energy: f64 = passes.energy[p][classes.clone()].iter().sum();
            let passing: f64 = passes.passing[p][classes.clone()].iter().sum();
            let heard = classes
                .map(|class| (passes.energy[p][class], passes.passing[p][class]))
                .filter(|&(energy, passing)| energy > 0.0 && passing > 0.0);
            fewer.energy[p][first] = energy;
            fewer.passing[p][first] = share_keeping_loudness(penalty_db, heard).unwrap_or(passing);
        }
    }
    fewer
}

/// The share of the time of one stream holding the classes `heard` (each its energy and its
/// share): see [`fewer_streams`]. `None` where no class has loudness while passing, or all have
/// the same loudness over level (one class).
fn share_keeping_loudness(
    penalty_db: f64,
    heard: impl Iterator<Item = (f64, f64)> + Clone,
) -> Option<f64> {
    let over_level =
        |level_db: f64| GROWTH.at(level_db + penalty_db) * 10f64.powf(-level_db / 10.0);
    let energy: f64 = heard.clone().map(|(energy, _)| energy).sum();
    let ratios = heard.map(|(energy, passing)| {
        let level_db = level_db(energy / passing);
        (over_level(level_db), level_db, energy)
    });
    let wanted = (ratios.clone())
        .map(|(ratio, _, energy)| ratio * energy)
        .sum::<f64>()
        / energy;
    // The levels of the classes whose loudness over level is highest and lowest.
    let (mut over, mut under) = ratios.fold(
        ((f64::NEG_INFINITY, 0.0), (f64::INFINITY, 0.0)),
        |(over, under), (ratio, level_db, _)| {
            let over = if ratio > over.0 {
                (ratio, level_db)
            } else {
                over
            };
            let under = if ratio < under.0 {
                (ratio, level_db)
            } else {
                under
            };
            (over, under)
        },
    );
    if !(wanted > 0.0 && over.0 > under.0) {
        return None;
    }
    for _ in 0..LEVEL_HALVINGS {
        let middle = 0.5 * (over.1 + under.1);
        if over_level(middle) >= wanted {
            over.1 = middle;
        } else {
            under.1 = middle;
        }
    }
    Some(energy * 10f64.powf(-0.5 * (over.1 + under.1) / 10.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use popup::aircraft::{PASS_CLASS_DB, PASS_LOWEST_DB};

    /// Passes at `(period, level in dB, share of the time)`, each in its class as the popup
    /// places it.
    fn passes(heard: &[(usize, f64, f64)]) -> FlightPasses {
        let mut passes = FlightPasses::default();
        for &(period, level_db, passing) in heard {
            passes.add_pass(period, 10f64.powf(level_db / 10.0) * passing, passing);
        }
        passes
    }

    /// The first class of the stream holding a pass of `level_db`.
    fn stream_of(level_db: f64) -> usize {
        let class = ((level_db - PASS_LOWEST_DB) / PASS_CLASS_DB) as usize;
        class / STREAM_CLASSES * STREAM_CLASSES
    }

    /// What the passes `(level in dB, share)` sound like in the mean, one at a time, in a period
    /// with `penalty_db`.
    fn loudness(heard: &[(f64, f64)], penalty_db: f64) -> f64 {
        heard
            .iter()
            .map(|&(level_db, passing)| passing * GROWTH.at(level_db + penalty_db))
            .sum()
    }

    /// Two classes 6 dB apart become one stream: it keeps their energy, sounds as loud in the
    /// mean while passing as they did, in the day and with the evening's penalty, and passes a
    /// smaller share of the time than both together; a class alone in its stream stays as it is.
    #[test]
    fn a_stream_keeps_its_classes_energy_and_their_loudness_while_they_pass() {
        let (quiet, loud, alone) = ((50.5, 0.1), (56.5, 0.05), (70.5, 0.01));
        assert_eq!(stream_of(quiet.0), stream_of(loud.0));
        assert_ne!(stream_of(quiet.0), stream_of(alone.0));
        let heard: Vec<(usize, f64, f64)> = (0..2)
            .flat_map(|period| [quiet, loud, alone].map(|(level, share)| (period, level, share)))
            .collect();
        let fewer = fewer_streams(&passes(&heard));
        let energy_of = |(level_db, passing): (f64, f64)| 10f64.powf(level_db / 10.0) * passing;
        for (period, penalty_db) in [(0, 0.0), (1, 5.0)] {
            let at = stream_of(quiet.0);
            let (energy, share) = (fewer.energy[period][at], fewer.passing[period][at]);
            assert!((energy / (energy_of(quiet) + energy_of(loud)) - 1.0).abs() < 1e-12);
            let level = level_db(energy / share);
            assert!(quiet.0 < level && level < loud.0, "{level}");
            assert!(share < quiet.1 + loud.1);
            let kept = loudness(&[(level, share)], penalty_db);
            let before = loudness(&[quiet, loud], penalty_db);
            assert!((kept / before - 1.0).abs() < 0.005, "{kept} {before}");
            let lone = stream_of(alone.0);
            let (energy, share) = (fewer.energy[period][lone], fewer.passing[period][lone]);
            assert!((energy / energy_of(alone) - 1.0).abs() < 1e-12);
            assert_eq!(share, alone.1);
        }
        // The night's classes are untouched by the day's and the evening's.
        assert_eq!(fewer.energy[2], [0.0; PASS_CLASSES]);
    }

    /// At the threshold of hearing loudness grows faster than the level: a class too faint to be
    /// heard and a class just heard still become a stream as loud in the mean as they were (kept
    /// at their shares it would pass in silence).
    #[test]
    fn a_stream_across_the_threshold_of_hearing_keeps_its_loudness() {
        let (silent, heard) = ((0.5, 0.1), (16.5, 0.001));
        assert_eq!(stream_of(silent.0), stream_of(heard.0));
        assert_eq!(GROWTH.at(silent.0), 0.0);
        let before = loudness(&[silent, heard], 0.0);
        assert!(before > 0.0);
        let fewer = fewer_streams(&passes(&[(0, silent.0, silent.1), (0, heard.0, heard.1)]));
        let at = stream_of(silent.0);
        let level = level_db(fewer.energy[0][at] / fewer.passing[0][at]);
        let kept = loudness(&[(level, fewer.passing[0][at])], 0.0);
        assert!((kept / before - 1.0).abs() < 0.02, "{kept} {before}");
    }

    /// Classes none of which is heard keep their energy and their shares.
    #[test]
    fn unheard_classes_keep_their_shares() {
        let (first, second) = ((-19.5, 0.1), (-13.5, 0.1));
        assert_eq!(stream_of(first.0), stream_of(second.0));
        let fewer = fewer_streams(&passes(&[(0, first.0, first.1), (0, second.0, second.1)]));
        assert_eq!(fewer.passing[0][stream_of(first.0)], 0.2);
    }
}
