//! Motorcycles in the traffic of roads nobody counted them on: the counted rows of the same
//! square where they count motorcycles, else the country's share from its fleet. Where
//! motorcycles are a minority of the fleet they are ridden for leisure and carry a small part of
//! the traffic (Czechia, Germany, Great Britain, Poland, Ireland, Slovenia count 0.5-1.5 % of the
//! vehicles on 1.8 M rows where 3-20 % of the fleet are motorcycles: a traffic share of 0.18 of
//! the fleet share in the median); where they are most of it they are the daily vehicle (Thailand's
//! rural-road counts, 35,260 ways of DRR, motorcycles counted as their own class: 42 % of the
//! vehicles on tertiary roads in the median where they are 52 % of the fleet, 0.77). The share
//! passes between the two over a fleet share of 0.35 +- 0.05 (no counts in between were found;
//! evidence 2026-10-02, motorcycles).

/// Per country (ISO 3166-1 alpha-2, sorted): the motorcycle share of the traffic on a tertiary
/// road and how far the country rides them daily (0 leisure, 1 daily vehicle). The fleet share is
/// the WHO Global status report on road safety 2023 country profile's powered two- and
/// three-wheelers over its registered vehicles (95 countries whose total and vehicle types agree);
/// without it, dev4's traffic share of the country where it set one (Cambodia, Laos, Taiwan,
/// Japan, Nigeria, Kenya), else the median fleet share of the country's WHO region.
const COUNTRY_MOTORCYCLES: [([u8; 2], f64, f64); 249] = [
    (*b"AD", 0.0151, 0.005),
    (*b"AE", 0.0048, 0.002),
    (*b"AF", 0.0033, 0.001),
    (*b"AG", 0.0034, 0.001),
    (*b"AI", 0.0151, 0.005),
    (*b"AL", 0.0110, 0.003),
    (*b"AM", 0.0151, 0.005),
    (*b"AO", 0.0151, 0.005),
    (*b"AQ", 0.0151, 0.005),
    (*b"AR", 0.1633, 0.488),
    (*b"AS", 0.0151, 0.005),
    (*b"AT", 0.0272, 0.016),
    (*b"AU", 0.0082, 0.002),
    (*b"AW", 0.0151, 0.005),
    (*b"AX", 0.0151, 0.005),
    (*b"AZ", 0.0006, 0.001),
    (*b"BA", 0.0030, 0.001),
    (*b"BB", 0.0920, 0.231),
    (*b"BD", 0.5900, 1.000),
    (*b"BE", 0.0123, 0.004),
    (*b"BF", 0.0300, 0.020),
    (*b"BG", 0.0109, 0.003),
    (*b"BH", 0.0058, 0.002),
    (*b"BI", 0.0300, 0.020),
    (*b"BJ", 0.0300, 0.020),
    (*b"BL", 0.0151, 0.005),
    (*b"BM", 0.0151, 0.005),
    (*b"BN", 0.0151, 0.005),
    (*b"BO", 0.0973, 0.251),
    (*b"BQ", 0.0151, 0.005),
    (*b"BR", 0.0766, 0.172),
    (*b"BS", 0.0151, 0.005),
    (*b"BT", 0.6062, 1.000),
    (*b"BV", 0.0151, 0.005),
    (*b"BW", 0.0010, 0.001),
    (*b"BY", 0.0112, 0.003),
    (*b"BZ", 0.0920, 0.231),
    (*b"CA", 0.0056, 0.002),
    (*b"CC", 0.0151, 0.005),
    (*b"CD", 0.0151, 0.005),
    (*b"CF", 0.0300, 0.020),
    (*b"CG", 0.0151, 0.005),
    (*b"CH", 0.0227, 0.010),
    (*b"CI", 0.0300, 0.020),
    (*b"CK", 0.3353, 0.910),
    (*b"CL", 0.0071, 0.002),
    (*b"CM", 0.0300, 0.020),
    (*b"CN", 0.0426, 0.050),
    (*b"CO", 0.4699, 0.994),
    (*b"CR", 0.0897, 0.222),
    (*b"CU", 0.2235, 0.672),
    (*b"CV", 0.0300, 0.020),
    (*b"CW", 0.0151, 0.005),
    (*b"CX", 0.0151, 0.005),
    (*b"CY", 0.0112, 0.003),
    (*b"CZ", 0.0416, 0.047),
    (*b"DE", 0.0141, 0.004),
    (*b"DJ", 0.0151, 0.005),
    (*b"DK", 0.0112, 0.003),
    (*b"DM", 0.0920, 0.231),
    (*b"DO", 0.4267, 0.985),
    (*b"DZ", 0.0077, 0.002),
    (*b"EC", 0.0875, 0.214),
    (*b"EE", 0.0112, 0.003),
    (*b"EG", 0.1331, 0.384),
    (*b"EH", 0.0151, 0.005),
    (*b"ER", 0.0024, 0.001),
    (*b"ES", 0.0302, 0.021),
    (*b"ET", 0.0300, 0.020),
    (*b"FI", 0.0114, 0.003),
    (*b"FJ", 0.0341, 0.028),
    (*b"FK", 0.0151, 0.005),
    (*b"FM", 0.0341, 0.028),
    (*b"FO", 0.0151, 0.005),
    (*b"FR", 0.0115, 0.003),
    (*b"GA", 0.0229, 0.011),
    (*b"GB", 0.0061, 0.002),
    (*b"GD", 0.0920, 0.231),
    (*b"GE", 0.0112, 0.003),
    (*b"GF", 0.0151, 0.005),
    (*b"GG", 0.0151, 0.005),
    (*b"GH", 0.0300, 0.020),
    (*b"GI", 0.0151, 0.005),
    (*b"GL", 0.0151, 0.005),
    (*b"GM", 0.0300, 0.020),
    (*b"GN", 0.0300, 0.020),
    (*b"GP", 0.0151, 0.005),
    (*b"GQ", 0.0151, 0.005),
    (*b"GR", 0.0388, 0.040),
    (*b"GS", 0.0151, 0.005),
    (*b"GT", 0.3016, 0.855),
    (*b"GU", 0.0151, 0.005),
    (*b"GW", 0.0300, 0.020),
    (*b"GY", 0.0920, 0.231),
    (*b"HK", 0.0151, 0.005),
    (*b"HM", 0.0151, 0.005),
    (*b"HN", 0.3426, 0.920),
    (*b"HR", 0.0111, 0.003),
    (*b"HT", 0.0151, 0.005),
    (*b"HU", 0.0075, 0.002),
    (*b"ID", 0.6398, 1.000),
    (*b"IE", 0.0112, 0.003),
    (*b"IL", 0.0074, 0.002),
    (*b"IM", 0.0151, 0.005),
    (*b"IN", 0.6313, 1.000),
    (*b"IO", 0.0151, 0.005),
    (*b"IQ", 0.0033, 0.001),
    (*b"IR", 0.0033, 0.001),
    (*b"IS", 0.0081, 0.002),
    (*b"IT", 0.0379, 0.037),
    (*b"JE", 0.0151, 0.005),
    (*b"JM", 0.0050, 0.002),
    (*b"JO", 0.0014, 0.001),
    (*b"JP", 0.0200, 0.008),
    (*b"KE", 0.2000, 0.608),
    (*b"KG", 0.0012, 0.001),
    (*b"KH", 0.3000, 0.853),
    (*b"KI", 0.0341, 0.028),
    (*b"KM", 0.0300, 0.020),
    (*b"KN", 0.0151, 0.005),
    (*b"KP", 0.0151, 0.005),
    (*b"KR", 0.0149, 0.005),
    (*b"KW", 0.0033, 0.001),
    (*b"KY", 0.0151, 0.005),
    (*b"KZ", 0.0112, 0.003),
    (*b"LA", 0.3000, 0.853),
    (*b"LB", 0.0147, 0.005),
    (*b"LC", 0.0920, 0.231),
    (*b"LI", 0.0151, 0.005),
    (*b"LK", 0.5583, 0.999),
    (*b"LR", 0.0300, 0.020),
    (*b"LS", 0.0151, 0.005),
    (*b"LT", 0.0112, 0.003),
    (*b"LU", 0.0217, 0.009),
    (*b"LV", 0.0059, 0.002),
    (*b"LY", 0.0005, 0.001),
    (*b"MA", 0.1272, 0.363),
    (*b"MC", 0.0151, 0.005),
    (*b"MD", 0.0112, 0.003),
    (*b"ME", 0.0044, 0.001),
    (*b"MF", 0.0151, 0.005),
    (*b"MG", 0.0300, 0.020),
    (*b"MH", 0.0151, 0.005),
    (*b"MK", 0.0052, 0.002),
    (*b"ML", 0.0300, 0.020),
    (*b"MM", 0.6389, 1.000),
    (*b"MN", 0.0106, 0.003),
    (*b"MO", 0.0151, 0.005),
    (*b"MP", 0.0151, 0.005),
    (*b"MQ", 0.0151, 0.005),
    (*b"MR", 0.0151, 0.005),
    (*b"MS", 0.0151, 0.005),
    (*b"MT", 0.0112, 0.003),
    (*b"MU", 0.1958, 0.591),
    (*b"MV", 0.6748, 1.000),
    (*b"MW", 0.0151, 0.005),
    (*b"MX", 0.0207, 0.008),
    (*b"MY", 0.0341, 0.028),
    (*b"MZ", 0.0300, 0.020),
    (*b"NA", 0.0300, 0.020),
    (*b"NC", 0.0151, 0.005),
    (*b"NE", 0.0300, 0.020),
    (*b"NF", 0.0151, 0.005),
    (*b"NG", 0.3000, 0.853),
    (*b"NI", 0.0151, 0.005),
    (*b"NL", 0.0112, 0.003),
    (*b"NO", 0.0112, 0.003),
    (*b"NP", 0.6135, 1.000),
    (*b"NR", 0.0151, 0.005),
    (*b"NU", 0.0151, 0.005),
    (*b"NZ", 0.0341, 0.028),
    (*b"OM", 0.0008, 0.001),
    (*b"PA", 0.0920, 0.231),
    (*b"PE", 0.4501, 0.991),
    (*b"PF", 0.0151, 0.005),
    (*b"PG", 0.0341, 0.028),
    (*b"PH", 0.4805, 0.996),
    (*b"PK", 0.6158, 1.000),
    (*b"PL", 0.0171, 0.006),
    (*b"PM", 0.0151, 0.005),
    (*b"PN", 0.0151, 0.005),
    (*b"PR", 0.0151, 0.005),
    (*b"PS", 0.0017, 0.001),
    (*b"PT", 0.0129, 0.004),
    (*b"PW", 0.0151, 0.005),
    (*b"PY", 0.1855, 0.560),
    (*b"QA", 0.0035, 0.001),
    (*b"RE", 0.0151, 0.005),
    (*b"RO", 0.0112, 0.003),
    (*b"RS", 0.0065, 0.002),
    (*b"RU", 0.0070, 0.002),
    (*b"RW", 0.0300, 0.020),
    (*b"SA", 0.0013, 0.001),
    (*b"SB", 0.0151, 0.005),
    (*b"SC", 0.0093, 0.003),
    (*b"SD", 0.0031, 0.001),
    (*b"SE", 0.0115, 0.003),
    (*b"SG", 0.0272, 0.016),
    (*b"SH", 0.0151, 0.005),
    (*b"SI", 0.0151, 0.005),
    (*b"SJ", 0.0151, 0.005),
    (*b"SK", 0.0112, 0.003),
    (*b"SL", 0.0151, 0.005),
    (*b"SM", 0.0112, 0.003),
    (*b"SN", 0.0300, 0.020),
    (*b"SO", 0.0033, 0.001),
    (*b"SR", 0.0292, 0.019),
    (*b"SS", 0.0300, 0.020),
    (*b"ST", 0.0300, 0.020),
    (*b"SV", 0.0944, 0.240),
    (*b"SX", 0.0151, 0.005),
    (*b"SY", 0.0471, 0.064),
    (*b"SZ", 0.0300, 0.020),
    (*b"TC", 0.0151, 0.005),
    (*b"TD", 0.5604, 0.999),
    (*b"TF", 0.0151, 0.005),
    (*b"TG", 0.5381, 0.999),
    (*b"TH", 0.3894, 0.966),
    (*b"TJ", 0.0012, 0.001),
    (*b"TK", 0.0151, 0.005),
    (*b"TL", 0.5989, 1.000),
    (*b"TM", 0.0151, 0.005),
    (*b"TN", 0.0018, 0.001),
    (*b"TO", 0.0341, 0.028),
    (*b"TR", 0.0282, 0.017),
    (*b"TT", 0.0920, 0.231),
    (*b"TV", 0.0151, 0.005),
    (*b"TW", 0.3500, 0.930),
    (*b"TZ", 0.0300, 0.020),
    (*b"UA", 0.0112, 0.003),
    (*b"UG", 0.0300, 0.020),
    (*b"UM", 0.0151, 0.005),
    (*b"US", 0.0055, 0.002),
    (*b"UY", 0.0920, 0.231),
    (*b"UZ", 0.0151, 0.005),
    (*b"VA", 0.0151, 0.005),
    (*b"VC", 0.0151, 0.005),
    (*b"VE", 0.0920, 0.231),
    (*b"VG", 0.0041, 0.001),
    (*b"VI", 0.0151, 0.005),
    (*b"VN", 0.7202, 1.000),
    (*b"VU", 0.0151, 0.005),
    (*b"WF", 0.0151, 0.005),
    (*b"WS", 0.0004, 0.001),
    (*b"YE", 0.0151, 0.005),
    (*b"YT", 0.0151, 0.005),
    (*b"ZA", 0.0151, 0.005),
    (*b"ZM", 0.0151, 0.005),
    (*b"ZW", 0.0300, 0.020),
];

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
pub fn country_share(country_iso: u16, class: usize) -> f64 {
    let iso = country_iso.to_le_bytes();
    let (share, daily) = COUNTRY_MOTORCYCLES
        .binary_search_by(|(code, _, _)| code[..].cmp(&iso[..]))
        .map_or((0.01, 0.0), |index| {
            (COUNTRY_MOTORCYCLES[index].1, COUNTRY_MOTORCYCLES[index].2)
        });
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
}

impl LocalMotorcycles {
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
    pub fn share(&self, class: usize, country_iso: u16) -> Option<f64> {
        let (moto, total, rows) = self.groups[group(class)];
        if rows >= LEAST_GROUP_ROWS {
            return Some(moto / total);
        }
        let (moto, total, rows) = self.groups.iter().fold((0.0, 0.0, 0), |sum, entry| {
            (sum.0 + entry.0, sum.1 + entry.1, sum.2 + entry.2)
        });
        (rows >= LEAST_SQUARE_ROWS).then(|| {
            moto / total * country_share(country_iso, class)
                / country_share(country_iso, 4).max(1e-6)
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
        let thai = country_share(iso(b"TH"), 4);
        assert!((0.35..0.43).contains(&thai), "{thai}");
        assert!((country_share(iso(b"TH"), 3) / thai - 0.75).abs() < 0.03);
        assert!(country_share(iso(b"TH"), 0) < 0.06);
        let german = country_share(iso(b"DE"), 2);
        assert!((0.008..0.02).contains(&german), "{german}");
        assert_eq!(country_share(iso(b"ZZ"), 4), 0.01);
    }

    /// The square's counted rows decide where enough of them count motorcycles: their own group
    /// first, else the whole square scaled to the class.
    #[test]
    fn counted_rows_of_the_square_decide() {
        let mut local = LocalMotorcycles::default();
        assert_eq!(local.share(4, iso(b"TH")), None);
        for _ in 0..12 {
            local.add(4, [300.0, 50.0, 30.0, 620.0]);
        }
        assert!((local.share(9, iso(b"TH")).unwrap() - 0.62).abs() < 1e-9);
        assert_eq!(local.share(3, iso(b"TH")), None);
        for _ in 0..8 {
            local.add(2, [600.0, 50.0, 50.0, 300.0]);
        }
        let secondary = local.share(3, iso(b"TH")).unwrap();
        assert!(secondary > 0.3 && secondary < 0.5, "{secondary}");
        assert!(counted(1113, 15));
        assert!(counted(20, 0));
        assert!(!counted(0, 15));
    }
}
