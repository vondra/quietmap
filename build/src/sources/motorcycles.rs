//! Motorcycle shares from local counted roads, then the year’s national fleet table.
use crate::source_tables::SourceTables;

/// The share of a class against a tertiary road's: [leisure, daily]. Thailand's counts read
/// secondary roads at three quarters of the tertiary share and primary roads at about half;
/// motorways and expressways closed to them in most daily-riding countries; leisure riders use
/// every class alike, motorways at half.
fn class_ratio(class: usize) -> [f64; 2] {
    match class {
        0 | 10 => [0.5, 0.1],
        1 | 2 | 11 | 12 => [1.0, 0.5],
        3 => [1.0, 0.75],
        _ => [1.0, 1.0],
    }
}

/// The motorcycle share of a road of `class` in a country by its fleet.
pub fn country_share(tables: &SourceTables, country_iso: u16, class: usize) -> f64 {
    let iso = country_iso.to_le_bytes();
    let [share, daily] = tables
        .motorcycle_shares
        .get(iso)
        .copied()
        .unwrap_or([0.01, 0.0]);
    let [leisure, daily_ratio] = class_ratio(class);
    share * (leisure + daily * (daily_ratio - leisure))
}

/// Sources whose counts hold motorcycles though dev4 left the rows' estimated bits set:
/// Thailand's DRR rural-road table (its `MC` column).
const COUNTED_MOTORCYCLE_SOURCES: [u16; 1] = [1113];
/// dev4's `traffic_estimated` bits of light vehicles and motorcycles.
const LIGHT_ESTIMATED: u8 = 1;
const MOTORCYCLES_ESTIMATED: u8 = 8;
/// Class groups of the square's counted shares: motorways, trunk and primary roads, secondary,
/// tertiary and unclassified, local streets.
const GROUPS: usize = 5;
/// The least counted rows of a group (or of the square) whose share stands for the group.
const LEAST_GROUP_ROWS: u32 = 10;
const LEAST_SQUARE_ROWS: u32 = 20;

fn group(class: usize) -> usize {
    match class {
        0 | 10 => 0,
        1 | 2 | 11 | 12 => 1,
        3 => 2,
        4 | 9 => 3,
        _ => 4,
    }
}

/// Whether a row's motorcycles were counted.
pub fn counted(source_id: u16, estimated: u8) -> bool {
    estimated & (LIGHT_ESTIMATED | MOTORCYCLES_ESTIMATED) == 0
        || COUNTED_MOTORCYCLE_SOURCES.contains(&source_id)
}

/// The motorcycles the counted rows of one square carry, by class group.
#[derive(Default)]
pub struct LocalMotorcycles {
    groups: [(f64, f64, u32); GROUPS],
    observations: std::collections::HashSet<(u16, String, usize)>,
}

impl LocalMotorcycles {
    pub fn add_observation(&mut self, source: u16, identity: &str, class: usize, flows: [f64; 4]) {
        if !identity.is_empty()
            && self
                .observations
                .insert((source, identity.into(), group(class)))
        {
            self.add(class, flows);
        }
    }

    /// Adds a counted row's daily flows (light, medium, heavy, motorcycles).
    pub fn add(&mut self, class: usize, flows: [f64; 4]) {
        let total: f64 = flows.iter().sum();
        if total > 0.0 {
            let entry = &mut self.groups[group(class)];
            entry.0 += flows[3];
            entry.1 += total;
            entry.2 += 1;
        }
    }

    /// The flow-weighted motorcycle share of the counted rows of the class's group, or of the
    /// whole square scaled by the class against a tertiary road, when enough rows were counted.
    pub fn share(&self, tables: &SourceTables, class: usize, country_iso: u16) -> Option<f64> {
        let (moto, total, rows) = self.groups[group(class)];
        if rows >= LEAST_GROUP_ROWS {
            return Some(moto / total);
        }
        let (moto, total, rows) = self.groups.iter().fold((0.0, 0.0, 0), |sum, entry| {
            (sum.0 + entry.0, sum.1 + entry.1, sum.2 + entry.2)
        });
        (rows >= LEAST_SQUARE_ROWS).then(|| {
            moto / total * country_share(tables, country_iso, class)
                / country_share(tables, country_iso, 4).max(1e-6)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(code: &[u8; 2]) -> u16 {
        u16::from_le_bytes(*code)
    }

    /// Thailand rides motorcycles daily: about 39 % of a tertiary road's vehicles, three quarters
    /// of it on secondary roads and almost none on motorways; Germany a little over 1 % on any
    /// road; a country without a table entry 1 %.
    #[test]
    fn a_country_rides_motorcycles_as_its_fleet_says() {
        let tables = SourceTables::test_fixture();
        let thai = country_share(tables, iso(b"TH"), 4);
        assert!((0.35..0.43).contains(&thai), "{thai}");
        assert!((country_share(tables, iso(b"TH"), 3) / thai - 0.75).abs() < 0.03);
        assert!(country_share(tables, iso(b"TH"), 0) < 0.06);
        let german = country_share(tables, iso(b"DE"), 2);
        assert!((0.008..0.02).contains(&german), "{german}");
        assert_eq!(country_share(tables, iso(b"ZZ"), 4), 0.01);
    }

    /// The square's counted rows decide where enough of them count motorcycles: their own group
    /// first, else the whole square scaled to the class.
    #[test]
    fn counted_rows_of_the_square_decide() {
        let tables = SourceTables::test_fixture();
        let mut local = LocalMotorcycles::default();
        assert_eq!(local.share(tables, 4, iso(b"TH")), None);
        for _ in 0..12 {
            local.add(4, [300.0, 50.0, 30.0, 620.0]);
        }
        assert!((local.share(tables, 9, iso(b"TH")).unwrap() - 0.62).abs() < 1e-9);
        assert_eq!(local.share(tables, 3, iso(b"TH")), None);
        for _ in 0..8 {
            local.add(2, [600.0, 50.0, 50.0, 300.0]);
        }
        let secondary = local.share(tables, 3, iso(b"TH")).unwrap();
        assert!(secondary > 0.3 && secondary < 0.5, "{secondary}");
        assert!(counted(1113, 15));
        assert!(counted(20, 0));
        assert!(!counted(0, 15));
    }

    /// Each year's committed table reads as the run will read it: no repeated country, no
    /// overlapping region, shares and daily use between zero and one.
    #[test]
    fn every_year_motorcycle_table_reads_as_a_country_table() {
        for year in ["2024", "2026"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../years/tables/{year}/motorcycle-share.json"));
            let table: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let shares =
                crate::source_tables::CountryTable::<[f64; 2]>::fixture(&table["rows"].to_string());
            assert!(
                shares
                    .values()
                    .flatten()
                    .all(|value| (0.0..=1.0).contains(value)),
                "{year}"
            );
            assert!(shares.get(*b"TH").unwrap()[0] > 0.3, "{year}");
        }
    }
}

#[cfg(test)]
mod observation_tests {
    #[test]
    fn one_station_split_into_many_pieces_is_one_sample() {
        let mut local = super::LocalMotorcycles::default();
        for _ in 0..30 {
            local.add_observation(1, "one station", 4, [90.0, 0.0, 0.0, 10.0]);
        }
        assert_eq!(
            local.share(
                crate::source_tables::SourceTables::test_fixture(),
                4,
                u16::from_le_bytes(*b"DE")
            ),
            None
        );
        for i in 1..10 {
            local.add_observation(1, &format!("station {i}"), 4, [90.0, 0.0, 0.0, 10.0]);
        }
        assert_eq!(
            local.share(
                crate::source_tables::SourceTables::test_fixture(),
                4,
                u16::from_le_bytes(*b"DE")
            ),
            Some(0.1)
        );
    }
}
