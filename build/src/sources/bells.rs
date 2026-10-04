//! Church bells as events (research 2026-10-02, building-plant): the Christian places of worship
//! and bell towers of OpenStreetMap (`fetch/worship.sh`) in Europe, where the research found what
//! they ring. dev4 gave every place of worship of the world a steady 72 dB(A) "bells, fleet
//! average": a steady source where bells ring minutes a day, and far under their energy.
//!
//! What rings: the prayer ringing three times a day (the Angelus at 06-07, 12 and 18-19 h,
//! Württemberg's Gebetläuten three or four times, 1-5 minutes: three minutes taken, the morning one
//! before 07 h in the night period), the Sunday service's peal (7-8 minutes, once a week), and in
//! Germany and Switzerland the clock's strikes, the hours and their quarters (about 25 s of
//! striking an hour; Switzerland's also at night: the Zurich sleep study counted 23 bell events a
//! night). Sound power while ringing: a parish church's peal 114 dB(A) (110-118), a cathedral's
//! 122 (120-125), a clock's strike 110 (LUBW: LAFmax 71 dB at about 30 m), a lone bell tower's
//! single bell 109 (a parish peal's three or four bells, one of them). The bells hang in the tower:
//! three quarters of the church's height, at least 15 m (a cathedral 30, a bell tower 6). Their
//! partials lie between 250 Hz and 2 kHz. Places of worship of other religions or none mapped keep
//! their building's plant.

use super::cells::{Site, Z30Ring, push_site_points};
use super::{Converted, group_key};
use crate::dev4::degrees_to_z30;
use physics::bands::BANDS;
use physics::emission::spectrum::SoundPower;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// One place of worship or bell tower: where, its religion, and what it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorshipSite {
    pub lat: f64,
    pub lon: f64,
    pub religion: Religion,
    pub kind: SiteKind,
    /// The mapped height (m), 0 when none.
    pub height_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Religion {
    Christian,
    /// None mapped (most bell towers).
    Unknown,
    Other,
}

impl WorshipSite {
    /// Whether it rings: a Christian church, cathedral or place of worship (chapels apart: the
    /// research has nothing on their bells), or a bell tower of no other religion.
    pub fn rings(&self) -> bool {
        match self.kind {
            SiteKind::BellTower => self.religion != Religion::Other,
            SiteKind::Chapel => false,
            _ => self.religion == Religion::Christian,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    Cathedral,
    Church,
    Chapel,
    BellTower,
    Other,
}

/// The places of worship and bell towers by z9 square.
pub struct WorshipSites {
    by_square: HashMap<(u32, u32), Vec<WorshipSite>>,
}

/// European countries (UN M49), where the research describes what the bells ring.
const EUROPE: [[u8; 2]; 51] = [
    *b"AD", *b"AL", *b"AT", *b"AX", *b"BA", *b"BE", *b"BG", *b"BY", *b"CH", *b"CZ", *b"DE", *b"DK",
    *b"EE", *b"ES", *b"FI", *b"FO", *b"FR", *b"GB", *b"GG", *b"GI", *b"GR", *b"HR", *b"HU", *b"IE",
    *b"IM", *b"IS", *b"IT", *b"JE", *b"LI", *b"LT", *b"LU", *b"LV", *b"MC", *b"MD", *b"ME", *b"MK",
    *b"MT", *b"NL", *b"NO", *b"PL", *b"PT", *b"RO", *b"RS", *b"RU", *b"SE", *b"SI", *b"SK", *b"SM",
    *b"UA", *b"VA", *b"XK",
];
/// Countries whose church clocks strike the hours and quarters by day (the research's Germany and
/// Switzerland), and those that strike at night too.
const STRIKING: [[u8; 2]; 2] = [*b"CH", *b"DE"];
const STRIKING_AT_NIGHT: [[u8; 2]; 1] = [*b"CH"];

/// A place of worship's bells are those of the Christian sites within this distance of its
/// building's centre (m).
pub const SITE_REACH_M: f64 = 40.0;

const PRAYER_RINGING_S: f64 = 180.0;
const SUNDAY_PEAL_S: f64 = 450.0;
/// Striking an hour (the hour's strokes and the quarters', about two seconds a stroke) and the
/// strikes heard an hour.
const STRIKING_S_PER_HOUR: f64 = 25.0;
const STRIKES_PER_HOUR: f64 = 4.0;
/// The Zurich study's 23 bell events a night: the strikes heard an hour at night.
const NIGHT_STRIKES_PER_HOUR: f64 = 23.0 / 8.0;
const PEAL_LW_DBA: f64 = 114.0;
const CATHEDRAL_PEAL_LW_DBA: f64 = 122.0;
const LONE_BELL_LW_DBA: f64 = 109.0;
const STRIKE_LW_DBA: f64 = 110.0;
/// Partials between 250 Hz and 2 kHz (63 Hz to 8 kHz octaves).
const BELL_SPECTRUM: [f64; BANDS] = [-25.0, -15.0, -6.0, -1.0, 0.0, -3.0, -9.0, -16.0];
const PERIOD_SECONDS: [f64; 3] = [12.0 * 3600.0, 4.0 * 3600.0, 8.0 * 3600.0];

impl WorshipSites {
    /// Reads `worship.txt`: `lat lon religion kind height_m` per line (tab-separated).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut by_square: HashMap<(u32, u32), Vec<WorshipSite>> = HashMap::new();
        for (number, line) in text.lines().enumerate() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [lat, lon, religion, kind, height] = fields[..] else {
                return Err(format!("line {}: expected five fields", number + 1));
            };
            let bad = |error: &dyn std::fmt::Display| format!("line {}: {error}", number + 1);
            let site = WorshipSite {
                lat: lat.parse().map_err(|e| bad(&e))?,
                lon: lon.parse().map_err(|e| bad(&e))?,
                religion: match religion {
                    "christian" => Religion::Christian,
                    "unknown" | "" => Religion::Unknown,
                    _ => Religion::Other,
                },
                kind: match kind {
                    "cathedral" => SiteKind::Cathedral,
                    "church" => SiteKind::Church,
                    "chapel" => SiteKind::Chapel,
                    "bell_tower" => SiteKind::BellTower,
                    _ => SiteKind::Other,
                },
                height_m: height.parse().map_err(|e| bad(&e))?,
            };
            let (gx, gy) = degrees_to_z30(site.lat, site.lon);
            let square = crate::dev4::Square::of_z30(gx, gy);
            by_square
                .entry((square.x, square.y))
                .or_default()
                .push(site);
        }
        Ok(WorshipSites { by_square })
    }

    /// The square's sites.
    pub fn in_square(&self, x: u32, y: u32) -> &[WorshipSite] {
        self.by_square.get(&(x, y)).map_or(&[], Vec::as_slice)
    }
}

/// What a church's bells ring in a country: events a day and seconds of ringing a day per period
/// (day, evening, night), and the energy of a day per period (seconds times 10^(LW/10)).
pub struct BellSchedule {
    pub events_per_day: [f64; 3],
    pub seconds: [f64; 3],
    pub energy: [f64; 3],
}

/// The schedule of bells at `peal_lw_dba`, the clock striking where the country's do; `None`
/// outside Europe.
pub fn schedule(country_iso: u16, peal_lw_dba: f64) -> Option<BellSchedule> {
    let code = country_iso.to_le_bytes();
    if !EUROPE.contains(&code) {
        return None;
    }
    let power = |lw: f64| 10f64.powf(lw / 10.0);
    // Prayer ringing at noon and in the evening by day, in the morning before 07 h by night; the
    // Sunday peal by day, a seventh of the days.
    let mut events = [2.0 + 1.0 / 7.0, 0.0, 1.0];
    let mut seconds = [
        2.0 * PRAYER_RINGING_S + SUNDAY_PEAL_S / 7.0,
        0.0,
        PRAYER_RINGING_S,
    ];
    let mut energy = seconds.map(|s| s * power(peal_lw_dba));
    if STRIKING.contains(&code) {
        let hours = [12.0, 4.0, 8.0];
        for period in 0..3 {
            let (strikes, striking) = if period == 2 {
                if !STRIKING_AT_NIGHT.contains(&code) {
                    continue;
                }
                (
                    NIGHT_STRIKES_PER_HOUR,
                    STRIKING_S_PER_HOUR * NIGHT_STRIKES_PER_HOUR / STRIKES_PER_HOUR,
                )
            } else {
                (STRIKES_PER_HOUR, STRIKING_S_PER_HOUR)
            };
            events[period] += strikes * hours[period];
            seconds[period] += striking * hours[period];
            energy[period] += striking * hours[period] * power(STRIKE_LW_DBA);
        }
    }
    Some(BellSchedule {
        events_per_day: events,
        seconds,
        energy,
    })
}

/// The bells' mean sound power per period (dB(A); day, evening and night offsets, silence minus
/// infinity), their share of each period ringing and their sound power while ringing.
pub fn bell_emission(schedule: &BellSchedule) -> (SoundPower, [f64; 3], f64) {
    let mean = std::array::from_fn::<f64, 3, _>(|p| schedule.energy[p] / PERIOD_SECONDS[p]);
    let duty = std::array::from_fn(|p| schedule.seconds[p] / PERIOD_SECONDS[p]);
    let level = |energy: f64| {
        if energy > 0.0 {
            10.0 * energy.log10()
        } else {
            f64::NEG_INFINITY
        }
    };
    let day = level(mean[0]);
    let ringing: f64 = schedule.seconds.iter().sum();
    let while_ringing = level(schedule.energy.iter().sum::<f64>() / ringing);
    (
        SoundPower {
            day_dba: day,
            spectrum_db: BELL_SPECTRUM,
            evening_offset_db: level(mean[1]) - day,
            night_offset_db: level(mean[2]) - day,
        },
        duty,
        while_ringing,
    )
}

/// The bells of a church or bell tower as one point source `height_m` above the ground at
/// `centroid` (lat, lon), screened by everything but `footprint_id`.
pub fn push_bells(
    (centroid, height_m, footprint_id): ((f64, f64), f64, u64),
    (country_iso, kind): (u16, SiteKind),
    key: &str,
    out: &mut Vec<Converted>,
) -> bool {
    let peal = match kind {
        SiteKind::Cathedral => CATHEDRAL_PEAL_LW_DBA,
        SiteKind::BellTower => LONE_BELL_LW_DBA,
        _ => PEAL_LW_DBA,
    };
    let Some(plan) = schedule(country_iso, peal) else {
        return false;
    };
    let (sound, duty, while_ringing) = bell_emission(&plan);
    let decibels = |level: f64| (level * 10.0).round() / 10.0;
    let ring: Z30Ring = Vec::new();
    let site = Site {
        centroid,
        ring: &ring,
        area_m2: 1.0,
        single_point_up_to_m2: 1.0,
        cell_m: 1.0,
    };
    let attribute = Attribute {
        layer: Layer::Building,
        height_m,
        ground_percent: GROUND_FROM_TERRAIN,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id,
        group_key: group_key(&["bells", key]),
        emission: sound.band_levels_db(),
        display: json!([
            "",
            "church_bells",
            decibels(height_m),
            0,
            0,
            "",
            decibels(while_ringing),
            null,
            plan.events_per_day.map(|e| (e * 100.0).round() / 100.0),
            duty.map(|d| (d * 1e5).round() / 1e5),
        ])
        .to_string(),
    };
    push_site_points(&super::cells::site_points(&site), 1.0, &attribute, out);
    true
}

/// The bells' height: three quarters of the church's, at least 15 m (a cathedral 30, a bell tower
/// three quarters of its mapped height, else 6 m).
pub fn bell_height_m(kind: SiteKind, building_height_m: f64) -> f64 {
    match kind {
        SiteKind::Cathedral => (0.75 * building_height_m).max(30.0),
        SiteKind::BellTower if building_height_m > 0.0 => 0.75 * building_height_m,
        SiteKind::BellTower => 6.0,
        _ => (0.75 * building_height_m).max(15.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Czech parish church: prayer ringing at noon and in the evening, in the morning before 07
    /// h, the Sunday peal; a German one strikes the quarters by day and evening, a Swiss one at
    /// night too; outside Europe nothing is known.
    #[test]
    fn bells_ring_their_countrys_schedule() {
        let iso = |code: &[u8; 2]| u16::from_le_bytes(*code);
        let czech = schedule(iso(b"CZ"), PEAL_LW_DBA).unwrap();
        assert!((czech.events_per_day[0] - (2.0 + 1.0 / 7.0)).abs() < 1e-12);
        assert_eq!(czech.events_per_day[1..], [0.0, 1.0]);
        let (sound, duty, ringing) = bell_emission(&czech);
        assert!((duty[0] - (360.0 + 450.0 / 7.0) / 43_200.0).abs() < 1e-12);
        assert!((ringing - PEAL_LW_DBA).abs() < 1e-9);
        // 424 s of a 114 dB(A) peal in 12 hours: a mean of 114 + 10 lg(424.3 / 43,200) = 93.9.
        assert!((sound.day_dba - 93.92).abs() < 0.01, "{}", sound.day_dba);
        assert_eq!(sound.evening_offset_db, f64::NEG_INFINITY);
        let german = schedule(iso(b"DE"), PEAL_LW_DBA).unwrap();
        assert!((german.events_per_day[1] - 16.0).abs() < 1e-12 && german.events_per_day[2] == 1.0);
        let swiss = schedule(iso(b"CH"), PEAL_LW_DBA).unwrap();
        assert!((swiss.events_per_day[2] - 24.0).abs() < 1e-12);
        assert!(schedule(iso(b"US"), PEAL_LW_DBA).is_none());
        assert_eq!(bell_height_m(SiteKind::Church, 8.0), 15.0);
        assert_eq!(bell_height_m(SiteKind::BellTower, 0.0), 6.0);
    }

    #[test]
    fn sites_are_read_and_filed_by_square() {
        let sites = WorshipSites::parse(
            "50.08\t14.42\tchristian\tchurch\t0\n13.75\t100.5\tbuddhist\tother\t0\n",
        )
        .unwrap();
        let (gx, gy) = degrees_to_z30(50.08, 14.42);
        let square = crate::dev4::Square::of_z30(gx, gy);
        let prague = sites.in_square(square.x, square.y);
        assert_eq!(prague.len(), 1);
        assert!(prague[0].religion == Religion::Christian && prague[0].kind == SiteKind::Church);
        assert!(prague[0].rings());
        let tower = WorshipSite {
            religion: Religion::Unknown,
            kind: SiteKind::BellTower,
            ..prague[0]
        };
        let chapel = WorshipSite {
            kind: SiteKind::Chapel,
            ..prague[0]
        };
        let mosque = WorshipSite {
            religion: Religion::Other,
            kind: SiteKind::Other,
            ..prague[0]
        };
        assert!(tower.rings() && !chapel.rings() && !mosque.rings());
        assert!(WorshipSites::parse("1\t2\tx\n").is_err());
    }
}
