//! Bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food places of OpenStreetMap
//! (`fetch/venues.sh`) by z9 square, with what the people converter needs of them (research
//! 2026-10-04, people): the seats outside, the hours (the mapped `opening_hours`, else the
//! country's customs) and the weight of a place in a nightlife cluster.

use super::opening_hours::{WeekHours, parse};
use crate::dev4::degrees_to_z30;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueKind {
    Bar,
    Pub,
    Nightclub,
    Biergarten,
    Restaurant,
    Cafe,
    FastFood,
}

/// The `outdoor_seating` tag: a terrace (`yes`, `terrace`, `sidewalk`, `garden`, ...), none, or
/// not mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seating {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VenueSite {
    pub lat: f64,
    pub lon: f64,
    pub kind: VenueKind,
    pub seating: Seating,
    /// A mapped outline's area (m2), 0 for a node.
    pub area_m2: f64,
    /// The mapped opening hours when readable.
    pub hours: Option<WeekHours>,
    pub name: String,
}

/// The places by z9 square.
pub struct Venues {
    by_square: HashMap<(u32, u32), Vec<VenueSite>>,
}

impl Venues {
    /// Reads `venues.txt`: `lat lon kind outdoor_seating area_m2 opening_hours name` per line
    /// (tab-separated); food courts are indoors and left out.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut by_square: HashMap<(u32, u32), Vec<VenueSite>> = HashMap::new();
        for (number, line) in text.lines().enumerate() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [lat, lon, kind, seating, area, hours, name] = fields[..] else {
                return Err(format!("line {}: expected seven fields", number + 1));
            };
            let bad = |error: &dyn std::fmt::Display| format!("line {}: {error}", number + 1);
            let kind = match kind {
                "bar" => VenueKind::Bar,
                "pub" => VenueKind::Pub,
                "nightclub" => VenueKind::Nightclub,
                "biergarten" => VenueKind::Biergarten,
                "restaurant" => VenueKind::Restaurant,
                "cafe" => VenueKind::Cafe,
                "fast_food" => VenueKind::FastFood,
                _ => continue,
            };
            let site = VenueSite {
                lat: lat.parse().map_err(|e| bad(&e))?,
                lon: lon.parse().map_err(|e| bad(&e))?,
                kind,
                seating: match seating {
                    "no" | "none" => Seating::No,
                    "unknown" | "" => Seating::Unknown,
                    _ => Seating::Yes,
                },
                area_m2: area.parse().map_err(|e| bad(&e))?,
                hours: parse(hours),
                name: name.to_string(),
            };
            let (gx, gy) = degrees_to_z30(site.lat, site.lon);
            let square = crate::dev4::Square::of_z30(gx, gy);
            by_square
                .entry((square.x, square.y))
                .or_default()
                .push(site);
        }
        for sites in by_square.values_mut() {
            merge_nodes_into_outlines(sites);
        }
        Ok(Venues { by_square })
    }

    /// The square's places.
    pub fn in_square(&self, x: u32, y: u32) -> &[VenueSite] {
        self.by_square.get(&(x, y)).map_or(&[], Vec::as_slice)
    }
}

/// Within this of an outline's own radius a node of the same kind stands on it (m).
const ON_OUTLINE_M: f64 = 5.0;

/// One place mapped twice, as a node and as its outline (Grok, review of 2026-10-05: two crowds,
/// +3 dB): a node of the same kind and no other name within the outline's radius (that of a circle
/// of its area) and [`ON_OUTLINE_M`] of its centre joins the nearest such outline, which takes from
/// the node the terrace, hours and name it lacks. Two names are two places (a café next door).
fn merge_nodes_into_outlines(sites: &mut Vec<VenueSite>) {
    let mut merged = vec![false; sites.len()];
    for node in 0..sites.len() {
        if sites[node].area_m2 > 0.0 {
            continue;
        }
        let (lat, lon, kind) = (sites[node].lat, sites[node].lon, sites[node].kind);
        let name = sites[node].name.to_lowercase();
        let metres_per_degree = 111_195.0;
        let nearest = (0..sites.len())
            .filter(|&outline| sites[outline].area_m2 > 0.0 && sites[outline].kind == kind)
            .filter(|&outline| {
                let other = sites[outline].name.to_lowercase();
                name.is_empty() || other.is_empty() || name == other
            })
            .map(|outline| {
                let site = &sites[outline];
                let (dy, dx) = (
                    (site.lat - lat) * metres_per_degree,
                    (site.lon - lon) * metres_per_degree * lat.to_radians().cos(),
                );
                let reach = (site.area_m2 / std::f64::consts::PI).sqrt() + ON_OUTLINE_M;
                (outline, dx.hypot(dy), reach)
            })
            .filter(|&(_, distance, reach)| distance <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((outline, _, _)) = nearest else {
            continue;
        };
        let taken = sites[node].clone();
        let site = &mut sites[outline];
        if site.seating == Seating::Unknown {
            site.seating = taken.seating;
        }
        if site.hours.is_none() {
            site.hours = taken.hours;
        }
        if site.name.is_empty() {
            site.name = taken.name;
        }
        merged[node] = true;
    }
    let mut keep = merged.iter().map(|merged| !merged);
    sites.retain(|_| keep.next().unwrap_or(true));
}

/// The median mapped beer garden's outline (m2; 4,767 outlines in the planet of 2026-09-21).
const BEER_GARDEN_M2: f64 = 222.0;
/// Seats a m2 of an outdoor area (Austria's experience value, Forum Schall 2008).
const SEATS_PER_M2: f64 = 0.7;

/// Seats outside: a tagged terrace's median (Madrid's census 2026: bars 28, restaurants and cafés
/// 24, fast food 23, nightclubs 38; Melbourne's pubs 40), an untagged place the share with a
/// terrace times its mean (Madrid: bars 13.5, restaurants 9.0, cafés 8.8, fast food 8.4,
/// nightclubs 1.9; Melbourne's pubs 0.56 x 40), a beer garden 0.7 a m2 of its outline (else of the
/// median mapped one).
pub fn seats(site: &VenueSite) -> f64 {
    use VenueKind::*;
    let (tagged, expected) = match site.kind {
        Bar => (28.0, 13.5),
        Pub => (40.0, 0.56 * 40.0),
        Nightclub => (38.0, 1.9),
        Restaurant => (24.0, 9.0),
        Cafe => (24.0, 8.8),
        FastFood => (23.0, 8.4),
        Biergarten => {
            let area = if site.area_m2 > 0.0 {
                site.area_m2
            } else {
                BEER_GARDEN_M2
            };
            return SEATS_PER_M2 * area;
        }
    };
    match site.seating {
        Seating::Yes => tagged,
        Seating::No => 0.0,
        Seating::Unknown => expected,
    }
}

/// A place's weight in a nightlife cluster (Ballesteros 2014's emission weights: bars 0.8, pubs and
/// beer gardens 1, nightclubs 2, restaurants 0.6); cafés and fast food none.
pub fn cluster_weight(kind: VenueKind) -> f64 {
    match kind {
        VenueKind::Bar => 0.8,
        VenueKind::Pub | VenueKind::Biergarten => 1.0,
        VenueKind::Nightclub => 2.0,
        VenueKind::Restaurant => 0.6,
        VenueKind::Cafe | VenueKind::FastFood => 0.0,
    }
}

/// A country's customs where a place maps no hours (hours; past 24 after midnight; Sunday to
/// Thursday nights, Friday and Saturday nights): when bars close and terraces empty, and when
/// dinner is eaten. Legal hours where the research found them (Catalonia's bars 03:00 in summer,
/// Madrid's 02:00/02:30 and terraces 01:00/01:30, Barcelona's terraces 24:00/01:00, Paris 02:00,
/// Ireland 23:30/00:30, the UK's 23:00, Norway's and Sweden's 01:00, the US's 02:00 last call,
/// Bavaria's beer gardens 23:00); dinner at Eurostat HETUS's evening peaks (Spain 21:20, Italy
/// 20:20, France 20:00, Germany 18:30, the UK 18:20). Elsewhere judgments: bars to 01:00 (02:00 at
/// weekends), terraces to 23:00, dinner 19-23 h; open-fronted places of the warm south-east of
/// Asia keep their terraces as long as they are open.
pub struct Customs {
    pub bar_close: (f64, f64),
    pub terrace_close: Option<(f64, f64)>,
    pub dinner: (f64, f64),
    pub late: bool,
}

const LATE: &str = "ES PT IT GR CY MT";
const EARLY_DINNER: &str = "DE AT CH NL BE LU DK NO SE FI IS GB IE PL CZ SK HU US CA AU NZ";
const OPEN_FRONTED: &str = "TH VN KH LA MM ID MY PH LK BN TL SG";

fn member(list: &str, code: [u8; 2]) -> bool {
    list.split_ascii_whitespace().any(|m| m.as_bytes() == code)
}

pub fn customs(country_iso: u16) -> Customs {
    let code = country_iso.to_le_bytes();
    let late = member(LATE, code);
    let bar_close = match &code {
        _ if late => (26.0, 27.0),
        b"FR" | b"US" | b"CA" => (26.0, 26.0),
        b"GB" => (23.0, 24.0),
        b"IE" => (23.5, 24.5),
        _ => (25.0, 26.0),
    };
    let terrace_close = if member(OPEN_FRONTED, code) {
        None
    } else if late {
        Some((24.0, 25.0))
    } else {
        Some((23.0, 23.0))
    };
    let dinner = if late {
        (20.0, 24.0)
    } else if member(EARLY_DINNER, code) {
        (18.0, 22.0)
    } else {
        (19.0, 23.0)
    };
    Customs {
        bar_close,
        terrace_close,
        dinner,
        late,
    }
}

fn is_weekend_night(day: usize) -> bool {
    day == 4 || day == 5
}

/// Open from `from` to `to` (weeknights, weekend nights) on the `days` (Monday first), past 24 into
/// the next day.
fn spans(spans: &[(f64, (f64, f64))], days: [bool; 7]) -> WeekHours {
    let mut week = [[false; 24]; 7];
    for day in (0..7).filter(|&day| days[day]) {
        for &(from, (weeknight, weekend)) in spans {
            let to = if is_weekend_night(day) {
                weekend
            } else {
                weeknight
            };
            for hour in 0..48usize {
                let middle = hour as f64 + 0.5;
                if middle >= from && middle < to {
                    week[(day + hour / 24) % 7][hour % 24] = true;
                }
            }
        }
    }
    week
}

/// A place's open hours and its terrace's: the mapped hours, else its kind's in its country; the
/// terrace closes at the country's terrace hour at the latest.
pub fn hours(site: &VenueSite, customs: &Customs) -> (WeekHours, WeekHours) {
    use VenueKind::*;
    let every_day = [true; 7];
    let open = site.hours.unwrap_or_else(|| match site.kind {
        Bar | Pub => spans(&[(12.0, customs.bar_close)], every_day),
        // Thursday, Friday and Saturday nights (Catalonia's discos to 05:45/06:45, Madrid's 05:30/06:00).
        Nightclub => {
            let (from, to) = if customs.late {
                (24.0, 30.0)
            } else {
                (23.0, 29.0)
            };
            spans(
                &[(from, (to, to))],
                [false, false, false, true, true, true, false],
            )
        }
        Biergarten => spans(&[(11.0, (23.0, 23.0))], every_day),
        Restaurant => {
            let (from, to) = customs.dinner;
            let lunch = if customs.late {
                (13.0, (16.0, 16.0))
            } else {
                (12.0, (15.0, 15.0))
            };
            spans(&[lunch, (from, (to, to + 1.0))], every_day)
        }
        Cafe => spans(
            &[(
                8.0,
                if customs.late {
                    (22.0, 22.0)
                } else {
                    (20.0, 20.0)
                },
            )],
            every_day,
        ),
        FastFood => spans(&[(11.0, (23.0, 24.0))], every_day),
    });
    let terrace = match customs.terrace_close {
        None => open,
        Some(close) => {
            let allowed = spans(&[(7.0, close)], every_day);
            std::array::from_fn(|day| {
                std::array::from_fn(|hour| open[day][hour] && allowed[day][hour])
            })
        }
    };
    (open, terrace)
}

#[cfg(test)]
#[path = "venues_tests.rs"]
mod tests;
