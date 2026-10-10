//! Calls to prayer as events (research 2026-10-04, calls-to-prayer): the mosques of OpenStreetMap
//! call through horn loudspeakers five times a day at their place's prayer times
//! (`prayer_times`), each call in the Lden period its clock time falls in over the year: Fajr's
//! 204 s, the others' 180 s (Bekasi's timed 3-4 minutes, four recordings 2:35-3:17), at 118 dB(A)
//! while calling (Yogyakarta at 100 m 106-125 dB(A) by amplifier setting, Port Harcourt 103-114,
//! Riyadh 114-123, a 2-4 horn system on chant 122-129 by the datasheets). By country, as their
//! rules and practice say:
//!
//! - Indonesia (SE 05/2022): the calls (210 s, Subuh's 240) after recitation, 10 minutes before
//!   Subuh and Friday's Zuhur, 5 before the others;
//! - Saudi Arabia (2021): the adhan and the iqama (60 s, a judgment) at a third of the power;
//!   Egypt: the adhan and the iqama;
//! - Turkey and Cyprus (Diyanet): the calls and the sala, Friday's by day and Thursday's in the
//!   evening (120 s, a judgment);
//! - Rwanda: no dawn call since 2022; China, Tajikistan and Singapore: none outside;
//! - western, central and northern Europe, the Americas, Australia and East Asia: Friday's noon
//!   call only, 240 s at 100 dB(A) (Cologne's 60 dB at the footpath, Oer-Erkenschwick's 33 dB(A)
//!   at 890 m), and only at mosques with a mapped minaret (7-8 % of Dutch mosques call, 0.5-1 % of
//!   German ones);
//! - everywhere else (the Middle East, Africa, South and Central Asia, the Balkans, Russia, South-East
//!   Asia): the five calls (India's 22-06 h loudspeaker ban is not the practice, the research found).
//!
//! The horns hang three quarters up a mapped minaret within reach, else 10 m up in Indonesia,
//! Malaysia and Brunei, 6 m where only Friday's call sounds, 15 m elsewhere, or 2 m above a taller
//! mosque. Their spectrum is a shouted voice through a horn (nothing under 200 Hz).

use super::Converted;
use super::events::{DAY, EVENING, EventSchedule, push_event_source};
use super::metres;
use super::prayer_times::{Isha, Method, WORLD_LEAGUE, call_periods};
use super::worship::{
    Host, Religion, SITE_REACH_M, SiteKind, WorshipSite, first, groups, nearest_host,
};
use crate::dev4::Square;
use crate::period::time_zone;
use physics::bands::BANDS;

/// What a country's mosques broadcast outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Calls,
    Indonesia,
    SaudiArabia,
    Egypt,
    Turkey,
    NoDawn,
    FridayOnly,
    Silent,
}

const RULES: [(&str, Rule); 7] = [
    ("CN TJ SG", Rule::Silent),
    ("ID", Rule::Indonesia),
    ("SA", Rule::SaudiArabia),
    ("EG", Rule::Egypt),
    ("TR CY", Rule::Turkey),
    ("RW", Rule::NoDawn),
    (
        "AD AT AX BE CH CZ DE DK EE ES FI FO FR GB GG GI HR HU IE IM IS IT JE LI LT LU LV MC MT NL \
         NO PL PT SE SI SK SM VA AI AG AW BS BB BQ VG KY CU CW DM DO GD GP HT JM MQ MS PR BL KN LC \
         MF VC SX TT TC VI BZ CR SV GT HN MX NI PA AR BO BV BR CL CO EC FK GF GY PY PE GS SR UY VE \
         US CA BM GL PM AU NZ NF JP KR TW HK MO UA BY MD",
        Rule::FridayOnly,
    ),
];

fn rule(code: [u8; 2]) -> Rule {
    RULES
        .iter()
        .find(|(members, _)| {
            members
                .split_ascii_whitespace()
                .any(|m| m.as_bytes() == code)
        })
        .map_or(Rule::Calls, |&(_, rule)| rule)
}

/// The authority's angles (Aladhan's compilation; JAKIM's own tables fit 18 degrees) and the
/// margins its tables add (Diyanet's, JAKIM's and Kemenag's against the computed times over the
/// year, research 2026-10-04; elsewhere computed within a minute).
fn method(code: [u8; 2]) -> Method {
    let angles = |fajr_angle: f64, isha: Isha| Method {
        fajr_angle,
        isha,
        ..WORLD_LEAGUE
    };
    match &code {
        b"EG" => angles(19.5, Isha::Angle(17.5)),
        b"SA" => angles(18.5, Isha::MinutesAfterMaghrib(90.0)),
        b"AE" | b"BH" | b"OM" | b"QA" => angles(19.5, Isha::MinutesAfterMaghrib(90.0)),
        b"KW" => angles(18.0, Isha::Angle(17.5)),
        b"MA" => angles(19.0, Isha::Angle(17.0)),
        b"IR" => angles(17.7, Isha::Angle(14.0)),
        b"TR" | b"CY" => Method {
            margins_min: [0.0, 5.0, 4.0, 7.0, 0.0],
            ..WORLD_LEAGUE
        },
        b"ID" => Method {
            margins_min: [2.5, 3.5, 2.4, 3.1, 2.4],
            ..angles(20.0, Isha::Angle(18.0))
        },
        b"MY" | b"BN" => Method {
            margins_min: [1.9, 2.8, 1.8, 1.9, 2.0],
            ..angles(18.0, Isha::Angle(18.0))
        },
        b"PK" | b"BD" | b"IN" | b"AF" => Method {
            asr_factor: 2.0,
            ..angles(18.0, Isha::Angle(18.0))
        },
        b"US" | b"CA" => angles(15.0, Isha::Angle(15.0)),
        _ => WORLD_LEAGUE,
    }
}

const CALL_LW_DBA: f64 = 118.0;
/// A third of the maximum volume (Saudi Arabia 2021): a third of the power.
const SAUDI_LW_DBA: f64 = 113.0;
const FRIDAY_CALL_LW_DBA: f64 = 100.0;
const FAJR_S: f64 = 204.0;
const CALL_S: f64 = 180.0;
const IQAMA_S: f64 = 60.0;
const SALA_S: f64 = 120.0;
const FRIDAY_CALL_S: f64 = 240.0;
/// Indonesia's calls and the recitation before them (SE 05/2022's caps): ten minutes before Subuh
/// and Friday's Zuhur, five before the others.
const SUBUH_CALL_S: f64 = 240.0;
const SUBUH_RECITATION_S: f64 = 600.0;
const INDONESIAN_CALL_S: f64 = 210.0;
const RECITATION_S: f64 = 300.0;
const FRIDAY_RECITATION_EXTRA_S: f64 = 300.0;
/// A shouted voice's spectrum through a horn loudspeaker (63 Hz to 8 kHz, Z-weighted).
const HORN_SPECTRUM: [f64; BANDS] = [-60.0, -53.0, -14.0, -4.0, 0.0, -4.0, -25.0, -56.0];

/// The calls of a mosque at (`lat`, `lon`) in a country; `None` where none sound outside.
pub fn schedule(country_iso: u16, lat: f64, lon: f64) -> Option<EventSchedule> {
    if country_iso == 0 {
        return None;
    }
    let code = country_iso.to_le_bytes();
    let rule = rule(code);
    let mut plan = EventSchedule::default();
    match rule {
        Rule::Silent => return None,
        Rule::FridayOnly => {
            plan.add(DAY, 1.0 / 7.0, FRIDAY_CALL_S, FRIDAY_CALL_LW_DBA);
            return Some(plan);
        }
        _ => {}
    }
    // Each call's sound: the recitation before the prayer time and the call after it.
    let sounding: [(f64, f64); 5] = std::array::from_fn(|call| match (rule, call) {
        (Rule::Indonesia, 0) => (SUBUH_RECITATION_S, SUBUH_RECITATION_S + SUBUH_CALL_S),
        (Rule::Indonesia, 1) => {
            let recitation = RECITATION_S + FRIDAY_RECITATION_EXTRA_S / 7.0;
            (recitation, recitation + INDONESIAN_CALL_S)
        }
        (Rule::Indonesia, _) => (RECITATION_S, RECITATION_S + INDONESIAN_CALL_S),
        (_, 0) => (0.0, FAJR_S),
        _ => (0.0, CALL_S),
    });
    let periods = call_periods((lat, lon), &time_zone(lat, lon), &method(code), sounding);
    let lw = if rule == Rule::SaudiArabia {
        SAUDI_LW_DBA
    } else {
        CALL_LW_DBA
    };
    for (call, (events, seconds)) in periods.iter().enumerate() {
        if call == 0 && rule == Rule::NoDawn {
            continue;
        }
        plan.add_split(*events, *seconds, lw);
        if matches!(rule, Rule::SaudiArabia | Rule::Egypt) {
            // The iqama follows by the prayer's own delay: in the call's period.
            plan.add_split(*events, events.map(|e| e * IQAMA_S), lw);
        }
    }
    if rule == Rule::Turkey {
        plan.add(DAY, 1.0 / 7.0, SALA_S, lw);
        plan.add(EVENING, 1.0 / 7.0, SALA_S, lw);
    }
    // No call at all where the sun never sets or rises on any sampled day.
    (plan.seconds.iter().sum::<f64>() > 0.0).then_some(plan)
}

/// The horns' height: three quarters of a mapped minaret, else the country's usual mounting, or
/// 2 m above a taller mosque.
fn horn_height_m(code: [u8; 2], minaret_m: f64, mosque_m: f64) -> f64 {
    if minaret_m > 0.0 {
        return 0.75 * minaret_m;
    }
    let usual: f64 = match (rule(code), &code) {
        (_, b"ID" | b"MY" | b"BN") => 10.0,
        (Rule::FridayOnly, _) => 6.0,
        _ => 15.0,
    };
    usual.max(mosque_m + 2.0)
}

/// The calls of the square's mosques: the mosque sites within reach of each other call as one,
/// from a minaret within reach where one is mapped, else from the mosque's building, screened by
/// everything but the building nearest the horns. Where only Friday's call sounds, a mosque without
/// a minaret is silent. Returns how many call.
pub fn convert_calls(
    (sites, square): (&[&WorshipSite], Square),
    hosts: &[Host],
    country_iso: u16,
    out: &mut Vec<Converted>,
) -> usize {
    let code = country_iso.to_le_bytes();
    let muslim = |site: &&WorshipSite| site.religion == Religion::Muslim;
    let mosques: Vec<&WorshipSite> = sites
        .iter()
        .copied()
        .filter(muslim)
        .filter(|site| site.kind != SiteKind::Minaret)
        .collect();
    let minarets: Vec<&WorshipSite> = sites
        .iter()
        .copied()
        .filter(muslim)
        .filter(|site| site.kind == SiteKind::Minaret)
        .collect();
    let mut calling = 0;
    for group in groups(&mosques) {
        // The square of the group's first mosque calls (a group across a square's edge once).
        let Some(first) = first(group.iter().map(|&m| mosques[m])) else {
            continue;
        };
        if first.square() != square {
            continue;
        }
        let point = (first.lat, first.lon);
        let minaret = minarets
            .iter()
            .filter(|minaret| {
                group.iter().any(|&m| {
                    metres((mosques[m].lat, mosques[m].lon), (minaret.lat, minaret.lon))
                        <= SITE_REACH_M
                })
            })
            .max_by(|a, b| a.height_m.total_cmp(&b.height_m));
        if minaret.is_none() && rule(code) == Rule::FridayOnly {
            continue;
        }
        let Some(plan) = schedule(country_iso, first.lat, first.lon) else {
            continue;
        };
        let mosque = nearest_host(point, hosts);
        let horns = minaret.map_or_else(
            || mosque.map_or(point, |host| host.centre),
            |minaret| (minaret.lat, minaret.lon),
        );
        let screen = nearest_host(horns, hosts).map_or(0, |host| host.footprint_id);
        let height = horn_height_m(
            code,
            minaret.map_or(0.0, |minaret| minaret.height_m),
            mosque.map_or(0.0, |host| host.height_m),
        );
        let name = mosque.map_or("", Host::worship_name);
        let key = format!("{:.6},{:.6}", point.0, point.1);
        push_event_source(
            (horns, height, screen),
            ("call_to_prayer", name),
            (&plan, HORN_SPECTRUM),
            (&key, mosque.and_then(Host::object)),
            out,
        );
        calling += 1;
    }
    calling
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::events::{NIGHT, event_emission};
    use crate::sources::worship::Denomination;

    fn iso(code: &[u8; 2]) -> u16 {
        u16::from_le_bytes(*code)
    }

    /// Istanbul's mosque calls five times a day, Fajr at night, plus Friday's and Thursday's sala;
    /// Riyadh's adds the iqamas at a third of the power; Jakarta's recites before its calls;
    /// Kigali's has no dawn call; Beijing's and Cologne's (no minaret) are silent outside.
    #[test]
    fn calls_follow_their_countrys_rules() {
        let istanbul = schedule(iso(b"TR"), 41.0082, 28.9784).unwrap();
        let total: f64 = istanbul.events_per_day.iter().sum();
        assert!((total - (5.0 + 2.0 / 7.0)).abs() < 1e-9, "{total}");
        assert!((istanbul.events_per_day[NIGHT] - 1.0).abs() < 1e-9);
        let (sound, duty, calling) = event_emission(&istanbul, HORN_SPECTRUM);
        assert!((calling - CALL_LW_DBA).abs() < 1e-9);
        // Fajr's 204 s in the 8 night hours: 118 + 10 lg(204 / 28,800) = 96.5 dB(A) on average.
        assert!((sound.day_dba + sound.night_offset_db - 96.5).abs() < 0.05);
        assert!((duty[NIGHT] - 204.0 / 28_800.0).abs() < 1e-9);
        let riyadh = schedule(iso(b"SA"), 24.7136, 46.6753).unwrap();
        assert!((riyadh.events_per_day.iter().sum::<f64>() - 10.0).abs() < 1e-9);
        assert!((event_emission(&riyadh, HORN_SPECTRUM).2 - SAUDI_LW_DBA).abs() < 1e-9);
        let jakarta = schedule(iso(b"ID"), -6.2, 106.85).unwrap();
        assert!((jakarta.seconds[NIGHT] - (SUBUH_RECITATION_S + SUBUH_CALL_S)).abs() < 1e-9);
        let kigali = schedule(iso(b"RW"), -1.95, 30.06).unwrap();
        assert_eq!(kigali.events_per_day[NIGHT], 0.0);
        assert!(schedule(iso(b"CN"), 39.9, 116.4).is_none());
        let cologne = schedule(iso(b"DE"), 50.94, 6.96).unwrap();
        assert_eq!(cologne.events_per_day, [1.0 / 7.0, 0.0, 0.0]);
    }

    /// A mosque's node and building call once from its minaret, three quarters up its 40 m; in
    /// Germany a mosque without a minaret is silent.
    #[test]
    fn a_mosque_calls_once_from_its_minaret() {
        let site = |lon: f64, kind: SiteKind, height_m: f64| WorshipSite {
            lat: 41.0,
            lon,
            religion: Religion::Muslim,
            kind,
            height_m,
            denomination: Denomination::Untagged,
        };
        let sites = [
            site(29.0, SiteKind::Other, 0.0),
            site(29.0001, SiteKind::Other, 0.0),
            site(29.0003, SiteKind::Minaret, 40.0),
        ];
        let hosts = [Host {
            centre: (41.0, 29.0001),
            height_m: 12.0,
            footprint_id: 9,
            name: "Yeni Cami".into(),
            worship: true,
            group_key: 90,
        }];
        let refs: Vec<&WorshipSite> = sites.iter().collect();
        let square = sites[0].square();
        let mut out = Vec::new();
        assert_eq!(
            convert_calls((&refs, square), &hosts, iso(b"TR"), &mut out),
            1
        );
        let display = &out[0].attribute.display;
        assert!(
            display.starts_with(r#"["Yeni Cami","call_to_prayer",30.0"#),
            "{display}"
        );
        out.clear();
        assert_eq!(
            convert_calls((&refs[..2], square), &hosts, iso(b"DE"), &mut out),
            0
        );
        assert_eq!(horn_height_m(*b"ID", 0.0, 4.0), 10.0);
        assert_eq!(horn_height_m(*b"EG", 0.0, 20.0), 22.0);
    }
}
