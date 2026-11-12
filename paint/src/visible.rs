//! What a visitor sees of a level, and whether a probe agrees with a blend as far as that shows.

use crate::exact::LAYERS;
use crate::levels::Painted;
use physics::bands::level_db;

/// What a visitor sees of a level: its colour from 1 sone or 35 dB (a phon a tenth of a colour's
/// doubling), the quiet zones' edge there, and the hover's number. A probe must agree with its
/// blend as far as that shows (the owner 2026-11-12: under the first colour a tenth or two of a
/// sone does not matter; the layers may be looser than the total). In two rural squares the old
/// test (half a phon and half a decibel in every layer from 0 dB) refused 26-30 % of the fine
/// patches, 85-97 % of them for a layer's Lden alone, a third where nothing shows at all.
mod tolerance {
    /// The total within this of its blend from [`TOTAL_FROM_SONE`] (phon) and [`TOTAL_FROM_DB`] (dB).
    pub const TOTAL_PHON: f64 = 0.5;
    pub const TOTAL_DB: f64 = 0.5;
    /// From a fifth under the first colour, so that the quiet zones' default edge stays exact.
    pub const TOTAL_FROM_SONE: f64 = 0.8;
    pub const TOTAL_FROM_DB: f64 = 32.0;
    /// A quieter total within this (sone, dB).
    pub const QUIET_TOTAL_SONE: f64 = 0.1;
    pub const QUIET_TOTAL_DB: f64 = 2.0;
    /// A layer within this of its blend from 1 sone (phon) and from 35 dB (dB).
    pub const LAYER_PHON: f64 = 1.0;
    pub const LAYER_DB: f64 = 1.0;
    pub const LAYER_FROM_SONE: f64 = 1.0;
    pub const LAYER_FROM_DB: f64 = 35.0;
    /// A quieter layer within this (sone; dB from [`QUIET_LAYER_FROM_DB`], under it any).
    pub const QUIET_LAYER_SONE: f64 = 0.2;
    pub const QUIET_LAYER_DB: f64 = 3.0;
    pub const QUIET_LAYER_FROM_DB: f64 = 20.0;
}

/// Whether two loudnesses (sone) agree: within `phon` where either reaches `from_sone`, else
/// within `quiet_sone`.
fn loudness_agrees(
    heard: f64,
    blended: f64,
    (phon, from_sone, quiet_sone): (f64, f64, f64),
) -> bool {
    if !heard.is_finite() || !blended.is_finite() || heard < 0.0 || blended < 0.0 {
        return false;
    }
    if heard.max(blended) < from_sone {
        return (heard - blended).abs() <= quiet_sone;
    }
    heard > 0.0 && blended > 0.0 && (10.0 * (heard / blended).log2()).abs() <= phon
}

/// Whether two Lden energies agree: within `db` where either reaches `from_db`, within `quiet_db`
/// where either reaches `quiet_from_db`, any under it.
fn energy_agrees(
    heard: f64,
    blended: f64,
    (db, from_db, quiet_db, quiet_from_db): (f64, f64, f64, f64),
) -> bool {
    if !heard.is_finite() || !blended.is_finite() || heard < 0.0 || blended < 0.0 {
        return false;
    }
    let level = level_db(heard.max(blended));
    if level < quiet_from_db {
        return true;
    }
    let within = if level >= from_db { db } else { quiet_db };
    heard > 0.0 && blended > 0.0 && (level_db(heard) - level_db(blended)).abs() <= within
}

/// Whether a probe's loudness agrees with a blend's as far as a visitor can see ([`tolerance`]).
pub(crate) fn loudness_visibly_agrees(
    heard: &[f64; LAYERS + 1],
    blended: &[f64; LAYERS + 1],
) -> bool {
    use tolerance::*;
    let total = (TOTAL_PHON, TOTAL_FROM_SONE, QUIET_TOTAL_SONE);
    let layer = (LAYER_PHON, LAYER_FROM_SONE, QUIET_LAYER_SONE);
    loudness_agrees(heard[LAYERS], blended[LAYERS], total)
        && (0..LAYERS).all(|at| loudness_agrees(heard[at], blended[at], layer))
}

/// Whether a probe's paint agrees with its blend as far as a visitor can see ([`tolerance`]).
pub(crate) fn visibly_agrees(heard: &Painted, blended: &Painted) -> bool {
    use tolerance::*;
    let total_energy = (TOTAL_DB, TOTAL_FROM_DB, QUIET_TOTAL_DB, 0.0);
    let layer_energy = (LAYER_DB, LAYER_FROM_DB, QUIET_LAYER_DB, QUIET_LAYER_FROM_DB);
    loudness_visibly_agrees(&heard.1, &blended.1)
        && energy_agrees(heard.0.iter().sum(), blended.0.iter().sum(), total_energy)
        && (0..LAYERS).all(|at| energy_agrees(heard.0[at], blended.0[at], layer_energy))
}

#[cfg(test)]
#[path = "visible_tests.rs"]
mod tests;
