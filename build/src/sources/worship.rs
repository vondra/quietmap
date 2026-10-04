//! Places of worship, bell towers and minarets of OpenStreetMap (`fetch/worship.sh`) by z9 square,
//! and what they form on the ground: the sites within [`SITE_REACH_M`] of each other are one place
//! (a node and its building, a church and its bell tower, a mosque and its minaret), sounding from
//! the building nearest them.

use crate::dev4::degrees_to_z30;
use std::collections::HashMap;
use std::path::Path;

/// One place of worship, bell tower or minaret.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorshipSite {
    pub lat: f64,
    pub lon: f64,
    pub religion: Religion,
    pub kind: SiteKind,
    /// The mapped height (m), 0 when none.
    pub height_m: f64,
    pub denomination: Denomination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Religion {
    Christian,
    Muslim,
    /// None mapped (most bell towers).
    Unknown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    Cathedral,
    Church,
    Chapel,
    BellTower,
    Minaret,
    Other,
}

/// A church's denomination as far as its bells go: Orthodox churches ring before their services,
/// not the Angelus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denomination {
    Orthodox,
    Other,
    Untagged,
}

/// Sites within this distance (m) of a group's first site belong to it.
pub const SITE_REACH_M: f64 = 40.0;

/// The places of worship, bell towers and minarets by z9 square.
pub struct WorshipSites {
    by_square: HashMap<(u32, u32), Vec<WorshipSite>>,
}

impl WorshipSites {
    /// Reads `worship.txt`: `lat lon religion kind height_m denomination` per line (tab-separated).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut by_square: HashMap<(u32, u32), Vec<WorshipSite>> = HashMap::new();
        for (number, line) in text.lines().enumerate() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [lat, lon, religion, kind, height, denomination] = fields[..] else {
                return Err(format!("line {}: expected six fields", number + 1));
            };
            let bad = |error: &dyn std::fmt::Display| format!("line {}: {error}", number + 1);
            let site = WorshipSite {
                lat: lat.parse().map_err(|e| bad(&e))?,
                lon: lon.parse().map_err(|e| bad(&e))?,
                religion: match religion {
                    "christian" => Religion::Christian,
                    "muslim" => Religion::Muslim,
                    "unknown" | "" => Religion::Unknown,
                    _ => Religion::Other,
                },
                kind: match kind {
                    "cathedral" => SiteKind::Cathedral,
                    "church" => SiteKind::Church,
                    "chapel" => SiteKind::Chapel,
                    "bell_tower" => SiteKind::BellTower,
                    "minaret" => SiteKind::Minaret,
                    _ => SiteKind::Other,
                },
                height_m: height.parse().map_err(|e| bad(&e))?,
                denomination: if denomination.is_empty() {
                    Denomination::Untagged
                } else if denomination.contains("orthodox") {
                    Denomination::Orthodox
                } else {
                    Denomination::Other
                },
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

/// Horizontal distance (m) between two points (lat, lon) a few hundred metres apart.
pub fn metres(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dy = (a.0 - b.0) * 111_320.0;
    let dx = (a.1 - b.1) * 111_320.0 * a.0.to_radians().cos();
    dx.hypot(dy)
}

/// The sites in groups: each group the sites not yet taken within [`SITE_REACH_M`] of its first,
/// in their order.
pub fn groups(sites: &[&WorshipSite]) -> Vec<Vec<usize>> {
    let mut taken = vec![false; sites.len()];
    let mut groups = Vec::new();
    for first in 0..sites.len() {
        if taken[first] {
            continue;
        }
        let at = (sites[first].lat, sites[first].lon);
        let group: Vec<usize> = (first..sites.len())
            .filter(|&other| {
                !taken[other] && metres(at, (sites[other].lat, sites[other].lon)) <= SITE_REACH_M
            })
            .collect();
        for &member in &group {
            taken[member] = true;
        }
        groups.push(group);
    }
    groups
}

/// A building bells or loudspeakers may hang from: its centre (lat, lon), height, screening
/// footprint and name, and whether it is a place of worship.
pub struct Host {
    pub centre: (f64, f64),
    pub height_m: f64,
    pub footprint_id: u64,
    pub name: String,
    pub worship: bool,
}

impl Host {
    /// Its name when it is a place of worship: another building's name (a school, a shop) names
    /// something else.
    pub fn worship_name(&self) -> &str {
        if self.worship { &self.name } else { "" }
    }
}

/// The building nearest `point` within [`SITE_REACH_M`], a place of worship before any other.
pub fn nearest_host(point: (f64, f64), hosts: &[Host]) -> Option<&Host> {
    hosts
        .iter()
        .map(|host| (host, metres(point, host.centre)))
        .filter(|(_, distance)| *distance <= SITE_REACH_M)
        .min_by(|(a, da), (b, db)| {
            (!a.worship, *da)
                .partial_cmp(&(!b.worship, *db))
                .expect("finite")
        })
        .map(|(host, _)| host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_are_read_and_filed_by_square() {
        let sites = WorshipSites::parse(
            "50.08\t14.42\tchristian\tchurch\t0\troman_catholic\n\
             50.0801\t14.4201\tunknown\tbell_tower\t32\t\n\
             13.75\t100.5\tbuddhist\tother\t0\t\n\
             41.0\t29.0\tmuslim\tminaret\t40\t\n\
             44.8\t20.46\tchristian\tchurch\t0\tserbian_orthodox\n",
        )
        .unwrap();
        let (gx, gy) = degrees_to_z30(50.08, 14.42);
        let square = crate::dev4::Square::of_z30(gx, gy);
        let prague = sites.in_square(square.x, square.y);
        assert_eq!(prague.len(), 2);
        assert_eq!(prague[0].religion, Religion::Christian);
        assert_eq!(prague[0].denomination, Denomination::Other);
        assert_eq!(prague[1].kind, SiteKind::BellTower);
        assert_eq!(prague[1].height_m, 32.0);
        let (gx, gy) = degrees_to_z30(44.8, 20.46);
        let belgrade = crate::dev4::Square::of_z30(gx, gy);
        let belgrade = sites.in_square(belgrade.x, belgrade.y);
        assert_eq!(belgrade[0].denomination, Denomination::Orthodox);
        let (gx, gy) = degrees_to_z30(41.0, 29.0);
        let istanbul = crate::dev4::Square::of_z30(gx, gy);
        let minaret = sites.in_square(istanbul.x, istanbul.y)[0];
        assert!(minaret.religion == Religion::Muslim && minaret.kind == SiteKind::Minaret);
        assert!(WorshipSites::parse("1\t2\tx\t\t0\n").is_err());
    }

    /// A church's node, its building's centroid and its bell tower 30 m away are one place; a
    /// second church 200 m away is another.
    #[test]
    fn sites_within_reach_group() {
        let site = |lat: f64, lon: f64| WorshipSite {
            lat,
            lon,
            religion: Religion::Christian,
            kind: SiteKind::Church,
            height_m: 0.0,
            denomination: Denomination::Untagged,
        };
        let sites = [
            site(50.0, 14.0),
            site(50.00005, 14.0),
            site(50.0, 14.0004),
            site(50.0018, 14.0),
        ];
        let refs: Vec<&WorshipSite> = sites.iter().collect();
        assert_eq!(groups(&refs), vec![vec![0, 1, 2], vec![3]]);
        let host = |lat: f64, worship: bool| Host {
            centre: (lat, 14.0),
            height_m: 10.0,
            footprint_id: lat.to_bits(),
            name: String::new(),
            worship,
        };
        let hosts = [
            host(50.0001, false),
            host(50.0003, true),
            host(50.001, true),
        ];
        let chosen = nearest_host((50.0, 14.0), &hosts).unwrap();
        assert!(
            chosen.worship && chosen.centre.0 == 50.0003,
            "a church before a nearer house"
        );
        assert!(nearest_host((50.01, 14.0), &hosts).is_none());
    }
}
