//! Church bells as events (research 2026-10-02, building-plant, and 2026-10-04, calls-to-prayer):
//! the Christian places of worship and bell towers of OpenStreetMap in Europe, where the research
//! found what they ring. dev4 gave every place of worship of the world a steady 72 dB(A) "bells,
//! fleet average": a steady source where bells ring minutes a day, and far under their energy.
//!
//! What rings: the prayer ringing three times a day (the Angelus at 06-07, 12 and 18-19 h,
//! Württemberg's Gebetläuten three or four times, 1-5 minutes: three minutes taken, the morning one
//! before 07 h in the night period), the Sunday service's peal (7-8 minutes, once a week), and in
//! Germany and Switzerland the clock's strikes, the hours and their quarters (about 25 s of
//! striking an hour; Switzerland's also at night: the Zurich sleep study counted 23 bell events a
//! night). Orthodox churches ring no Angelus: they ring before their services, Saturday's vespers
//! and Sunday's liturgy (five minutes each, a judgment: the Russian call to service lasts a psalm's
//! reading, no count or level was found); a church without a mapped denomination takes its
//! country's. Sound power while ringing: a parish church's peal 114 dB(A) (110-118), a cathedral's
//! 122 (120-125), a clock's strike 110 (LUBW: LAFmax 71 dB at about 30 m), a lone bell tower's
//! single bell 109 (a parish peal's three or four bells, one of them). The bells hang in the tower:
//! three quarters of the church's height, at least 15 m (a cathedral 30, a bell tower 6). Their
//! partials lie between 250 Hz and 2 kHz. Places of worship of other religions or none mapped keep
//! their building's plant.

use super::Converted;
use super::events::{DAY, EVENING, EventSchedule, NIGHT, push_event_source};
use super::worship::{
    Denomination, Host, Religion, SiteKind, WorshipSite, first, groups, nearest_host,
};
use crate::dev4::Square;
use physics::bands::BANDS;

/// European countries (UN M49) and Cyprus, where the research describes what the bells ring.
const EUROPE: [[u8; 2]; 52] = [
    *b"AD", *b"AL", *b"AT", *b"AX", *b"BA", *b"BE", *b"BG", *b"BY", *b"CH", *b"CY", *b"CZ", *b"DE",
    *b"DK", *b"EE", *b"ES", *b"FI", *b"FO", *b"FR", *b"GB", *b"GG", *b"GI", *b"GR", *b"HR", *b"HU",
    *b"IE", *b"IM", *b"IS", *b"IT", *b"JE", *b"LI", *b"LT", *b"LU", *b"LV", *b"MC", *b"MD", *b"ME",
    *b"MK", *b"MT", *b"NL", *b"NO", *b"PL", *b"PT", *b"RO", *b"RS", *b"RU", *b"SE", *b"SI", *b"SK",
    *b"SM", *b"UA", *b"VA", *b"XK",
];
/// Countries whose church clocks strike the hours and quarters by day (the research's Germany and
/// Switzerland), and those that strike at night too.
const STRIKING: [[u8; 2]; 2] = [*b"CH", *b"DE"];
const STRIKING_AT_NIGHT: [[u8; 2]; 1] = [*b"CH"];
/// European countries whose churches are Orthodox where no denomination is mapped.
const ORTHODOX_COUNTRIES: [[u8; 2]; 11] = [
    *b"BG", *b"BY", *b"CY", *b"GR", *b"MD", *b"ME", *b"MK", *b"RO", *b"RS", *b"RU", *b"UA",
];

const PRAYER_RINGING_S: f64 = 180.0;
const SUNDAY_PEAL_S: f64 = 450.0;
const ORTHODOX_RINGING_S: f64 = 300.0;
/// One strike of the hour or a quarter (25 s of striking an hour, four strikes heard an hour).
const STRIKE_S: f64 = 25.0 / 4.0;
const STRIKES_PER_HOUR: f64 = 4.0;
/// The Zurich study's 23 bell events a night.
const NIGHT_STRIKES: f64 = 23.0;
const PEAL_LW_DBA: f64 = 114.0;
const CATHEDRAL_PEAL_LW_DBA: f64 = 122.0;
const LONE_BELL_LW_DBA: f64 = 109.0;
const STRIKE_LW_DBA: f64 = 110.0;
/// Partials between 250 Hz and 2 kHz (63 Hz to 8 kHz octaves).
const BELL_SPECTRUM: [f64; BANDS] = [-25.0, -15.0, -6.0, -1.0, 0.0, -3.0, -9.0, -16.0];

impl WorshipSite {
    /// Whether it rings: a church, cathedral or bell tower of no other religion (a church building
    /// names its religion), a Christian place of worship; chapels never (the research has nothing
    /// on their bells).
    pub fn rings(&self) -> bool {
        match self.kind {
            SiteKind::BellTower | SiteKind::Church | SiteKind::Cathedral => {
                matches!(self.religion, Religion::Christian | Religion::Unknown)
            }
            SiteKind::Chapel | SiteKind::Minaret => false,
            SiteKind::Other => self.religion == Religion::Christian,
        }
    }
}

/// What a church's bells ring in a country at `peal_lw_dba`, the Orthodox ones before their
/// services and the others the Angelus, the Sunday peal and the clock where the country's strike;
/// `None` outside Europe.
pub fn schedule(country_iso: u16, peal_lw_dba: f64, orthodox: bool) -> Option<EventSchedule> {
    let code = country_iso.to_le_bytes();
    if !EUROPE.contains(&code) {
        return None;
    }
    let mut plan = EventSchedule::default();
    if orthodox {
        plan.add(DAY, 2.0 / 7.0, ORTHODOX_RINGING_S, peal_lw_dba);
        return Some(plan);
    }
    // Prayer ringing at noon and in the evening by day, in the morning before 07 h by night; the
    // Sunday peal by day, a seventh of the days.
    plan.add(DAY, 2.0, PRAYER_RINGING_S, peal_lw_dba);
    plan.add(DAY, 1.0 / 7.0, SUNDAY_PEAL_S, peal_lw_dba);
    plan.add(NIGHT, 1.0, PRAYER_RINGING_S, peal_lw_dba);
    if STRIKING.contains(&code) {
        plan.add(DAY, 12.0 * STRIKES_PER_HOUR, STRIKE_S, STRIKE_LW_DBA);
        plan.add(EVENING, 4.0 * STRIKES_PER_HOUR, STRIKE_S, STRIKE_LW_DBA);
        if STRIKING_AT_NIGHT.contains(&code) {
            plan.add(NIGHT, NIGHT_STRIKES, STRIKE_S, STRIKE_LW_DBA);
        }
    }
    Some(plan)
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

/// The bells of the square's Christian churches, cathedrals and bell towers: the sites within
/// reach of each other ring as one (from the bell tower where one is mapped), from the building
/// nearest them (a place of worship first), screened by everything but that building. Returns how
/// many ring.
pub fn convert_bells(
    (sites, square): (&[&WorshipSite], Square),
    hosts: &[Host],
    country_iso: u16,
    out: &mut Vec<Converted>,
) -> usize {
    let ringing: Vec<&WorshipSite> = sites.iter().copied().filter(|site| site.rings()).collect();
    let mut rung = 0;
    for group in groups(&ringing) {
        let members = || group.iter().map(|&m| ringing[m]);
        let kind = if members().any(|site| site.kind == SiteKind::Cathedral) {
            SiteKind::Cathedral
        } else if members().all(|site| site.kind == SiteKind::BellTower) {
            SiteKind::BellTower
        } else {
            SiteKind::Church
        };
        let orthodox = members().any(|site| site.denomination == Denomination::Orthodox)
            || (members().all(|site| site.denomination == Denomination::Untagged)
                && ORTHODOX_COUNTRIES.contains(&country_iso.to_le_bytes()));
        let peal = match kind {
            SiteKind::Cathedral => CATHEDRAL_PEAL_LW_DBA,
            SiteKind::BellTower => LONE_BELL_LW_DBA,
            _ => PEAL_LW_DBA,
        };
        let Some(plan) = schedule(country_iso, peal, orthodox) else {
            continue;
        };
        // The bells hang in a mapped tower where there is one, else in the church; the square of
        // that site rings them (a group across a square's edge rings once).
        let tower = first(members().filter(|site| site.kind == SiteKind::BellTower));
        let Some(at) = tower.or_else(|| first(members())) else {
            continue;
        };
        if at.square() != square {
            continue;
        }
        let point = (at.lat, at.lon);
        let host = nearest_host(point, hosts);
        let (centre, height_m) = match (tower, host) {
            (Some(tower), _) if tower.height_m > 0.0 => (point, 0.75 * tower.height_m),
            (Some(_), host) => (point, bell_height_m(kind, host.map_or(0.0, |h| h.height_m))),
            (None, Some(host)) => (host.centre, bell_height_m(kind, host.height_m)),
            (None, None) => (point, bell_height_m(kind, at.height_m)),
        };
        let (footprint_id, name) = host.map_or((0, ""), |h| (h.footprint_id, h.worship_name()));
        let key = format!("{:.6},{:.6}", point.0, point.1);
        push_event_source(
            (centre, height_m, footprint_id),
            ("church_bells", name),
            (&plan, BELL_SPECTRUM),
            (&key, host.and_then(Host::object)),
            out,
        );
        rung += 1;
    }
    rung
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::events::event_emission;

    /// A Czech parish church: prayer ringing at noon and in the evening, in the morning before 07
    /// h, the Sunday peal; a German one strikes the quarters by day and evening, a Swiss one at
    /// night too; a Serbian (Orthodox) one rings before Saturday's and Sunday's services only;
    /// outside Europe nothing is known.
    #[test]
    fn bells_ring_their_countrys_schedule() {
        let iso = |code: &[u8; 2]| u16::from_le_bytes(*code);
        let czech = schedule(iso(b"CZ"), PEAL_LW_DBA, false).unwrap();
        assert!((czech.events_per_day[0] - (2.0 + 1.0 / 7.0)).abs() < 1e-12);
        assert_eq!(czech.events_per_day[1..], [0.0, 1.0]);
        let (sound, duty, ringing) = event_emission(&czech, BELL_SPECTRUM);
        assert!((duty[0] - (360.0 + 450.0 / 7.0) / 43_200.0).abs() < 1e-12);
        assert!((ringing - PEAL_LW_DBA).abs() < 1e-9);
        // 424 s of a 114 dB(A) peal in 12 hours: a mean of 114 + 10 lg(424.3 / 43,200) = 93.9.
        assert!((sound.day_dba - 93.92).abs() < 0.01, "{}", sound.day_dba);
        assert_eq!(sound.evening_offset_db, f64::NEG_INFINITY);
        let german = schedule(iso(b"DE"), PEAL_LW_DBA, false).unwrap();
        assert!((german.events_per_day[1] - 16.0).abs() < 1e-12 && german.events_per_day[2] == 1.0);
        let swiss = schedule(iso(b"CH"), PEAL_LW_DBA, false).unwrap();
        assert!((swiss.events_per_day[2] - 24.0).abs() < 1e-12);
        let serbian = schedule(iso(b"RS"), PEAL_LW_DBA, true).unwrap();
        assert_eq!(serbian.events_per_day, [2.0 / 7.0, 0.0, 0.0]);
        let cypriot = schedule(iso(b"CY"), PEAL_LW_DBA, true).unwrap();
        assert_eq!(
            cypriot.events_per_day, serbian.events_per_day,
            "Cyprus rings, Orthodox"
        );
        assert!(schedule(iso(b"US"), PEAL_LW_DBA, false).is_none());
        assert_eq!(bell_height_m(SiteKind::Church, 8.0), 15.0);
        assert_eq!(bell_height_m(SiteKind::BellTower, 0.0), 6.0);
    }

    /// A church node and its bell tower 30 m away ring once, from the tower's host building; an
    /// untagged church in Greece rings as an Orthodox one; a chapel and a mosque do not ring.
    #[test]
    fn a_church_and_its_tower_ring_once_by_their_countrys_rite() {
        let site = |lon: f64, kind: SiteKind, religion: Religion| WorshipSite {
            lat: 50.0,
            lon,
            religion,
            kind,
            height_m: 0.0,
            denomination: Denomination::Untagged,
        };
        let sites = [
            site(14.0, SiteKind::Church, Religion::Christian),
            site(14.0004, SiteKind::BellTower, Religion::Unknown),
            site(14.01, SiteKind::Chapel, Religion::Christian),
            site(14.02, SiteKind::Other, Religion::Muslim),
        ];
        let hosts = [Host {
            centre: (50.0, 14.0001),
            height_m: 20.0,
            footprint_id: 7,
            name: "St Nicholas".into(),
            worship: true,
            group_key: 70,
        }];
        let iso = |code: &[u8; 2]| u16::from_le_bytes(*code);
        let refs: Vec<&WorshipSite> = sites.iter().collect();
        let here = (&refs[..], sites[0].square());
        let mut out = Vec::new();
        assert_eq!(convert_bells(here, &hosts, iso(b"CZ"), &mut out), 1);
        let display = &out[0].attribute.display;
        assert!(
            display.starts_with(r#"["St Nicholas","church_bells",15.0"#),
            "{display}"
        );
        assert!(display.contains("[2.14,0.0,1.0]"), "{display}");
        out.clear();
        assert_eq!(convert_bells(here, &hosts, iso(b"GR"), &mut out), 1);
        assert!(out[0].attribute.display.contains("[0.29,0.0,0.0]"));
        // A church and its tower 1.4 m apart across a square's edge (14.0625 E) ring once, from
        // the tower's square.
        let edge = [
            site(14.06249, SiteKind::Church, Religion::Christian),
            site(14.06251, SiteKind::BellTower, Religion::Unknown),
        ];
        let refs: Vec<&WorshipSite> = edge.iter().collect();
        let (west, east) = (edge[0].square(), edge[1].square());
        assert_ne!(west, east);
        out.clear();
        assert_eq!(convert_bells((&refs, west), &[], iso(b"CZ"), &mut out), 0);
        assert_eq!(convert_bells((&refs, east), &[], iso(b"CZ"), &mut out), 1);
    }
}
