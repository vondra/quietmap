//! Outdoor calls use the year's country policies and astronomical prayer times.

use super::Converted;
use super::events::{DAY, EVENING, EventSchedule, push_event_source};
use super::metres;
use super::prayer_times::call_periods;
use super::worship::{
    Host, Religion, SITE_REACH_M, SiteKind, WorshipSite, first, groups, nearest_host,
};
use crate::dev4::Square;
use crate::period::time_zone;
use crate::source_tables::SourceTables;
use physics::bands::BANDS;

/// A shouted voice's spectrum through a horn loudspeaker (63 Hz to 8 kHz, Z-weighted).
const HORN_SPECTRUM: [f64; BANDS] = [-60.0, -53.0, -14.0, -4.0, 0.0, -4.0, -25.0, -56.0];

/// The calls of a mosque at (`lat`, `lon`) in a country; `None` where none sound outside.
pub fn schedule(
    tables: &SourceTables,
    country_iso: u16,
    lat: f64,
    lon: f64,
) -> Option<EventSchedule> {
    if country_iso == 0 {
        return None;
    }
    let policy = tables.worship_calls.get(country_iso.to_le_bytes())?;
    if policy.silent {
        return None;
    }
    let mut plan = EventSchedule::default();
    if policy.friday_only {
        plan.add(DAY, 1.0 / 7.0, policy.sounding[1].1, policy.sound_power_dba);
        return Some(plan);
    }
    let periods = call_periods(
        &tables.solar_days,
        (lat, lon),
        &time_zone(lat, lon),
        &policy.method,
        policy.sounding,
    );
    let lw = policy.sound_power_dba;
    for (call, (events, seconds)) in periods.iter().enumerate() {
        if call == 0 && policy.skip_dawn {
            continue;
        }
        plan.add_split(*events, *seconds, lw);
        if policy.iqama_seconds > 0.0 {
            plan.add_split(
                *events,
                events.map(|event| event * policy.iqama_seconds),
                lw,
            );
        }
    }
    if policy.sala_seconds > 0.0 {
        plan.add(DAY, 1.0 / 7.0, policy.sala_seconds, lw);
        plan.add(EVENING, 1.0 / 7.0, policy.sala_seconds, lw);
    }
    // No call at all where the sun never sets or rises on any sampled day.
    (plan.seconds.iter().sum::<f64>() > 0.0).then_some(plan)
}

/// The horns' height: three quarters of a mapped minaret, else the country's usual mounting, or
/// 2 m above a taller mosque.
fn horn_height_m(tables: &SourceTables, code: [u8; 2], minaret_m: f64, mosque_m: f64) -> f64 {
    if minaret_m > 0.0 {
        return 0.75 * minaret_m;
    }
    let usual = tables
        .worship_calls
        .get(code)
        .map_or(15.0, |policy| policy.horn_height_m);
    usual.max(mosque_m + 2.0)
}

/// The calls of the square's mosques: the mosque sites within reach of each other call as one,
/// from a minaret within reach where one is mapped, else from the mosque's building, screened by
/// everything but the building nearest the horns. Where only Friday's call sounds, a mosque without
/// a minaret is silent. Returns how many call.
pub fn convert_calls(
    tables: &SourceTables,
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
        if minaret.is_none()
            && tables
                .worship_calls
                .get(code)
                .is_some_and(|policy| policy.friday_only)
        {
            continue;
        }
        let Some(plan) = schedule(tables, country_iso, first.lat, first.lon) else {
            continue;
        };
        let mosque = nearest_host(point, hosts);
        let horns = minaret.map_or_else(
            || mosque.map_or(point, |host| host.centre),
            |minaret| (minaret.lat, minaret.lon),
        );
        let screen = nearest_host(horns, hosts).map_or(0, |host| host.footprint_id);
        let height = horn_height_m(
            tables,
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
        let tables = SourceTables::test_fixture();
        let istanbul = schedule(tables, iso(b"TR"), 41.0082, 28.9784).unwrap();
        let total: f64 = istanbul.events_per_day.iter().sum();
        assert!((total - (5.0 + 2.0 / 7.0)).abs() < 1e-9, "{total}");
        assert!((istanbul.events_per_day[NIGHT] - 1.0).abs() < 1e-9);
        let (sound, duty, calling) = event_emission(&istanbul, HORN_SPECTRUM);
        assert!((calling - 118.0).abs() < 1e-9);
        // Fajr's 204 s in the 8 night hours: 118 + 10 lg(204 / 28,800) = 96.5 dB(A) on average.
        assert!((sound.day_dba + sound.night_offset_db - 96.5).abs() < 0.05);
        assert!((duty[NIGHT] - 204.0 / 28_800.0).abs() < 1e-9);
        let riyadh = schedule(tables, iso(b"SA"), 24.7136, 46.6753).unwrap();
        assert!((riyadh.events_per_day.iter().sum::<f64>() - 10.0).abs() < 1e-9);
        assert!((event_emission(&riyadh, HORN_SPECTRUM).2 - 113.0).abs() < 1e-9);
        let jakarta = schedule(tables, iso(b"ID"), -6.2, 106.85).unwrap();
        assert!((jakarta.seconds[NIGHT] - (840.0)).abs() < 1e-9);
        let kigali = schedule(tables, iso(b"RW"), -1.95, 30.06).unwrap();
        assert_eq!(kigali.events_per_day[NIGHT], 0.0);
        assert!(schedule(tables, iso(b"CN"), 39.9, 116.4).is_none());
        let cologne = schedule(tables, iso(b"DE"), 50.94, 6.96).unwrap();
        assert_eq!(cologne.events_per_day, [1.0 / 7.0, 0.0, 0.0]);
    }

    /// A mosque's node and building call once from its minaret, three quarters up its 40 m; in
    /// Germany a mosque without a minaret is silent.
    #[test]
    fn a_mosque_calls_once_from_its_minaret() {
        let tables = SourceTables::test_fixture();
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
            convert_calls(tables, (&refs, square), &hosts, iso(b"TR"), &mut out),
            1
        );
        let display = serde_json::Value::from(out[0].attribute.display.clone()).to_string();
        assert!(
            display.starts_with(r#"["Yeni Cami","call_to_prayer",30.0"#),
            "{display}"
        );
        out.clear();
        assert_eq!(
            convert_calls(tables, (&refs[..2], square), &hosts, iso(b"DE"), &mut out),
            0
        );
        assert_eq!(horn_height_m(tables, *b"ID", 0.0, 4.0), 10.0);
        assert_eq!(horn_height_m(tables, *b"EG", 0.0, 20.0), 22.0);
    }
}
