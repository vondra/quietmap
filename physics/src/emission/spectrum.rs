//! The sound power of a point or area source: an A-weighted day level, a relative band shape and
//! flat evening and night offsets. The shape carries no energy of its own: the bands are scaled so
//! their A-weighted sum is the stated level (dev4 audit 2026-06, where unscaled shapes had added
//! +4.9..+6.4 dB(A)).

use crate::bands::{BANDS, PERIODS, a_weighted_energy, level_db};

/// A source's emission as dev4's point models state it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundPower {
    /// A-weighted sound power by day, dB(A).
    pub day_dba: f64,
    /// Relative band levels (dB, Z-weighted) of the spectrum shape.
    pub spectrum_db: [f64; BANDS],
    pub evening_offset_db: f64,
    pub night_offset_db: f64,
}

impl SoundPower {
    /// Z-weighted band levels per period whose day A-weighted sum is `day_dba`.
    pub fn band_levels_db(&self) -> [[f64; BANDS]; PERIODS] {
        let day = bands_of_a_weighted_level(self.day_dba, &self.spectrum_db);
        let offsets = [0.0, self.evening_offset_db, self.night_offset_db];
        offsets.map(|offset| day.map(|level| level + offset))
    }
}

/// The A-weighted sum (dB(A)) of Z-weighted band levels.
pub fn a_weighted_level_db(bands_db: &[f64; BANDS]) -> f64 {
    level_db(a_weighted_energy(bands_db))
}

/// Band levels of the shape `spectrum_db` whose A-weighted sum is `level_dba`.
pub fn bands_of_a_weighted_level(level_dba: f64, spectrum_db: &[f64; BANDS]) -> [f64; BANDS] {
    let shape_dba = a_weighted_level_db(spectrum_db);
    spectrum_db.map(|relative| level_dba + relative - shape_dba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emission::{industrial, leisure, settlement, ships, wind};

    #[track_caller]
    fn assert_day_sum_is_the_level(sound: SoundPower, what: &str) {
        let day = a_weighted_level_db(&sound.band_levels_db()[0]);
        assert!(
            (day - sound.day_dba).abs() < 1e-9,
            "{what}: {day} != {}",
            sound.day_dba
        );
    }

    /// dev4's forever-invariant: every emission shape of the atlas radiates exactly its stated
    /// A-weighted level, whatever the level.
    #[test]
    fn every_shape_radiates_its_stated_level() {
        let profiles = (0..=u8::MAX)
            .map(|source_type| Some(industrial::source_type_profile(source_type)))
            .chain((0..=9_999).map(industrial::nace_profile))
            .chain((0..=u8::MAX).map(industrial::subtype_profile))
            .flatten();
        for profile in profiles {
            for area_m2 in [100.0, 10_000.0, 500_000.0, 3_000_000.0] {
                let cap = industrial::INDUSTRIAL_AREA_CAP_HEAVY_M2;
                let sound = industrial::area_law_sound_power(&profile, area_m2, cap);
                assert_day_sum_is_the_level(sound, "industrial profile");
            }
        }
        for sound in [
            industrial::solar_farm_sound_power(Some(24.0), 1.0),
            industrial::substation_sound_power(100.0),
            wind::turbine_sound_power(None),
            wind::turbine_sound_power(Some(3_000.0)),
            ships::ship_cell_sound_power([60.0, 1.0, 2.0]).unwrap().0,
        ] {
            assert_day_sum_is_the_level(sound, "power, wind or ship");
        }
        for class in 0..=u8::MAX {
            for (area_m2, floors) in [(100.0, 1), (200.0, 3), (5_000.0, 10)] {
                if let Some(sound) = settlement::building_sound_power(class, area_m2, floors) {
                    assert_day_sum_is_the_level(sound, "building class");
                }
            }
            if let Some(sound) = leisure::leisure_sound_power(class, 500.0) {
                assert_day_sum_is_the_level(sound, "leisure class");
            }
        }
        for subtype in [
            leisure::ShootingSubtype::Rifle,
            leisure::ShootingSubtype::Pistol,
            leisure::ShootingSubtype::Shotgun,
        ] {
            assert_day_sum_is_the_level(leisure::shooting_sound_power(subtype).unwrap(), "shots");
        }
    }

    #[test]
    fn evening_and_night_are_flat_offsets_of_the_day() {
        let sound = SoundPower {
            day_dba: 90.0,
            spectrum_db: [-5.0, -3.0, -1.0, 0.0, 0.0, -1.0, -3.0, -6.0],
            evening_offset_db: -3.0,
            night_offset_db: -10.0,
        };
        let [day, evening, night] = sound.band_levels_db();
        for band in 0..BANDS {
            assert!((evening[band] - day[band] + 3.0).abs() < 1e-12);
            assert!((night[band] - day[band] + 10.0).abs() < 1e-12);
        }
    }
}
