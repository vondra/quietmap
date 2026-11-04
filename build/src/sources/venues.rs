//! Bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food places of OpenStreetMap
//! (`qm-build venues`) by z9 square, with what the people converter needs of them (research
//! 2026-10-04, people): the seats outside, the hours (the mapped `opening_hours`, else the
//! country's customs) and the weight of a place in a nightlife cluster.

use super::opening_hours::parse;
use crate::dev4::degrees_to_z30;
use crate::source_tables::SourceTables;
use physics::bands::WeekHours;
use physics::emission::plant::kitchen_hours;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
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

/// Seats a m2 of an outdoor area (Austria's experience value, Forum Schall 2008).
const SEATS_PER_M2: f64 = 0.7;

/// Expected outdoor seats from the admitted seating survey; a mapped beer garden's area
/// overrides its survey median, and an explicit no-terrace tag gives zero.
pub fn seats(tables: &SourceTables, site: &VenueSite) -> f64 {
    use VenueKind::*;
    let model = &tables.venue_seating;
    let index = match site.kind {
        Bar => 0,
        Pub => 1,
        Nightclub => 2,
        Restaurant => 3,
        Cafe => 4,
        FastFood => 5,
        Biergarten => {
            let area = if site.area_m2 > 0.0 {
                site.area_m2
            } else {
                model.beer_garden_m2
            };
            return SEATS_PER_M2 * area;
        }
    };
    let [tagged, expected] = model.seats[index];
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

/// Opening, terrace and meal times where a place maps no hours, admitted by their validity.
/// Pairs are Sunday–Thursday and Friday–Saturday; hours past midnight exceed 24.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Customs {
    pub bar_close: (f64, f64),
    pub terrace_close: Option<(f64, f64)>,
    pub dinner: (f64, f64),
    pub late: bool,
}

pub fn customs(tables: &SourceTables, country_iso: u16) -> Customs {
    tables
        .venue_customs
        .get(country_iso.to_le_bytes())
        .copied()
        .unwrap_or(Customs {
            bar_close: (25.0, 26.0),
            terrace_close: Some((23.0, 23.0)),
            dinner: (19.0, 23.0),
            late: false,
        })
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

/// The hours of the places mapped in one building: when any is open, and when its kitchen runs if a
/// restaurant or a fast-food place cooks there.
#[derive(Debug, PartialEq)]
pub struct PlaceHours {
    pub open: WeekHours,
    pub kitchen: Option<WeekHours>,
}

/// The hours of the places `sites` mapped in one building, each place's own or its kind's in its
/// country.
pub fn place_hours(sites: &[&VenueSite], customs: &Customs) -> PlaceHours {
    let mut open = [[false; 24]; 7];
    let mut cooking = None;
    for site in sites {
        let (opening, _) = hours(site, customs);
        add(&mut open, &opening);
        if matches!(site.kind, VenueKind::Restaurant | VenueKind::FastFood) {
            add(cooking.get_or_insert([[false; 24]; 7]), &opening);
        }
    }
    PlaceHours {
        open,
        kitchen: cooking.as_ref().map(kitchen_hours),
    }
}

/// The hours of a building of bars and restaurants with no place mapped in it: a restaurant's in its
/// country, using the predominant mapped use recorded by the seating survey.
pub fn unmapped_place_hours(tables: &SourceTables, customs: &Customs) -> PlaceHours {
    let restaurant = VenueSite {
        lat: 0.0,
        lon: 0.0,
        kind: tables.venue_seating.unmapped_kind,
        seating: Seating::Unknown,
        area_m2: 0.0,
        hours: None,
        name: String::new(),
    };
    place_hours(&[&restaurant], customs)
}

/// `hours` and `more` together.
fn add(hours: &mut WeekHours, more: &WeekHours) {
    for (day, more_day) in hours.iter_mut().zip(more) {
        for (hour, &on) in day.iter_mut().zip(more_day) {
            *hour |= on;
        }
    }
}

#[cfg(test)]
#[path = "venues_tests.rs"]
mod tests;
