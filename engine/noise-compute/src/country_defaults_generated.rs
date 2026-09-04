//! Per-country and per-continent AADT scale factors relative to
//! WORLD_DEFAULT. Generated from `scripts/wb-country-2022.json`
//! and `scripts/wiki-roads-fleet.json` by
//! `scripts/gen-country-defaults-rs.mjs`.
//!
//! Sources:
//!   - Wikipedia *List of countries and territories by motor vehicles per
//!     capita* + *List of countries by road network size* → direct
//!     fleet / paved_km ratio (`vehicles_per_km`, ~160 countries).
//!   - World Bank `EN.POP.DNST` (people per km², 2022) — fallback
//!     for countries Wikipedia misses (~60 more).
//!
//! Method (plan v5 §I.3 refined):
//!   If wiki has vehicles_per_km:
//!     scale = clamp(0.7, 1.3,
//!                   1.0 + 0.3 × tanh(log₂(vpk / 63.5)))
//!   Else (density fallback):
//!     scale = clamp(0.7, 1.3,
//!                   1.0 + 0.3 × tanh(log₂(density / 60)))
//!
//! Why vehicles_per_km (plan v5 §I.3 refined, was density-only):
//!   AADT on an existing road is vehicle fleet / road km. Wikipedia gives
//!   both signals directly for ~160 countries. Density was a proxy when
//!   only WB API was available (WB discontinued road_km + fleet
//!   indicators in 2011). With Wikipedia the proxy becomes direct.
//!
//!   ±30% clamp is intentional — Wikipedia `road_paved_km` is noisy:
//!   BR reports only federal/state paved (under-counts ×10), some
//!   countries report zero paved (CR, TH, NO) forcing fallback to total,
//!   and fleet columns in developing countries often exclude 2-wheelers
//!   (IN, VN) — we override these per `source_override` annotation in
//!   wiki-roads-fleet.json where a better value is known.
//!
//! Applied to motorway/trunk/primary/link classes (0/1/2/10/11/12) in
//! `defaults.rs::country_default`. Local road classes (3-9) stay at
//! WORLD_DEFAULT — motorway-scale doesn't predict residential AADT.
//!
//! Continent scales are population-weighted averages of their countries'
//! factors (reaches only hexes whose country assignment fell outside every
//! CGAZ polygon, e.g. disputed areas, micro-ocean cells).
//!
//! To refresh:
//!   node scripts/fetch-wb-country-data.mjs 2022
//!   node scripts/fetch-wiki-roads-fleet.mjs
//!   node scripts/gen-country-defaults-rs.mjs
//!   # commit all three JSONs + this file

use crate::admin::Continent;

/// Per-country scale factor vs WORLD_DEFAULT motorway. 242 entries
/// (160 from Wikipedia vehicles_per_km, 82 from WB density fallback).
pub const COUNTRY_SCALES: &[(&[u8; 2], f64)] = &[
    (b"AD", 1.294), // wiki vpk=306.8
    (b"AE", 1.300), // wiki vpk=831.2
    (b"AF", 1.234), // wiki vpk=130.8
    (b"AG", 1.247), // wiki vpk=142.2
    (b"AL", 1.284), // wiki vpk=220.0
    (b"AM", 1.287), // wiki vpk=238.3
    (b"AO", 0.763), // density 28.6/km²
    (b"AR", 1.291), // wiki vpk=265.9
    (b"AS", 1.289), // density 241.7/km²
    (b"AT", 0.832), // wiki vpk=41.0
    (b"AU", 0.900), // wiki vpk=50.0
    (b"AW", 1.299), // density 596.2/km²
    (b"AZ", 1.118), // wiki vpk=84.7
    (b"BA", 0.904), // wiki vpk=50.4
    (b"BB", 1.046), // wiki vpk=70.6
    (b"BD", 0.933), // wiki vpk=54.2
    (b"BE", 0.968), // wiki vpk=58.9
    (b"BF", 1.299), // wiki vpk=578.3
    (b"BG", 1.258), // wiki vpk=156.3
    (b"BH", 1.284), // wiki vpk=220.3
    (b"BI", 1.066), // wiki vpk=74.2
    (b"BJ", 1.295), // wiki vpk=335.5
    (b"BM", 1.300), // density 1199.1/km²
    (b"BN", 1.274), // wiki vpk=185.6
    (b"BO", 0.705), // density 11.1/km²
    (b"BR", 1.298), // wiki vpk=438.6
    (b"BS", 1.261), // wiki vpk=160.5
    (b"BT", 1.293), // wiki vpk=289.5
    (b"BW", 1.021), // wiki vpk=66.6
    (b"BY", 0.886), // density 45.5/km²
    (b"BZ", 1.151), // wiki vpk=93.3
    (b"CA", 0.999), // wiki vpk=63.3
    (b"CD", 0.884), // density 45.2/km²
    (b"CF", 0.927), // wiki vpk=53.5
    (b"CG", 0.717), // density 17.7/km²
    (b"CH", 0.997), // wiki vpk=63.0
    (b"CI", 1.176), // density 95.6/km²
    (b"CL", 1.292), // wiki vpk=286.5
    (b"CM", 0.989), // density 58.5/km²
    (b"CN", 1.017), // wiki vpk=66.0
    (b"CO", 1.297), // wiki vpk=417.5
    (b"CR", 1.299), // wiki vpk=516.4
    (b"CU", 1.204), // density 106.5/km²
    (b"CV", 1.040), // wiki vpk=69.7
    (b"CW", 1.296), // density 345.3/km²
    (b"CY", 0.948), // wiki vpk=56.2
    (b"CZ", 1.229), // wiki vpk=127.1
    (b"DE", 1.000), // wiki vpk=63.5
    (b"DJ", 0.707), // wiki vpk=13.8
    (b"DK", 0.853), // wiki vpk=43.8
    (b"DM", 0.734), // wiki vpk=24.0
    (b"DO", 1.299), // wiki vpk=588.6
    (b"DZ", 1.017), // wiki vpk=66.1
    (b"EC", 1.294), // wiki vpk=310.7
    (b"EE", 0.781), // density 31.6/km²
    (b"EG", 0.836), // wiki vpk=41.5
    (b"ER", 0.864), // wiki vpk=45.3
    (b"ES", 0.862), // wiki vpk=45.0
    (b"ET", 0.703), // wiki vpk=10.0
    (b"EU", 1.215), // density 111.9/km²
    (b"FI", 0.932), // wiki vpk=54.1
    (b"FJ", 1.082), // wiki vpk=77.1
    (b"FM", 1.267), // density 160.2/km²
    (b"FO", 0.954), // wiki vpk=57.1
    (b"FR", 0.841), // wiki vpk=42.2
    (b"GA", 0.703), // density 9.4/km²
    (b"GB", 1.168), // wiki vpk=98.3
    (b"GD", 0.763), // wiki vpk=30.2
    (b"GE", 1.110), // wiki vpk=82.8
    (b"GH", 1.254), // wiki vpk=149.9
    (b"GI", 1.300), // density 3760.9/km²
    (b"GL", 0.708), // wiki vpk=14.1
    (b"GM", 1.297), // wiki vpk=386.1
    (b"GN", 0.979), // density 57.2/km²
    (b"GQ", 1.030), // density 64.3/km²
    (b"GR", 0.896), // wiki vpk=49.4
    (b"GT", 1.298), // wiki vpk=434.0
    (b"GU", 1.218), // wiki vpk=120.1
    (b"GW", 1.242), // wiki vpk=137.4
    (b"GY", 1.261), // wiki vpk=160.2
    (b"HK", 1.298), // wiki vpk=439.1
    (b"HN", 1.298), // wiki vpk=503.3
    (b"HR", 1.080), // wiki vpk=76.8
    (b"HT", 1.298), // density 417.4/km²
    (b"HU", 0.988), // wiki vpk=61.8
    (b"ID", 1.042), // wiki vpk=70.0
    (b"IE", 0.749), // wiki vpk=27.4
    (b"IL", 1.278), // wiki vpk=196.2
    (b"IM", 1.258), // density 147.6/km²
    (b"IN", 1.042), // wiki vpk=70.0 [national_in]
    (b"IQ", 1.192), // density 101.5/km²
    (b"IR", 1.105), // wiki vpk=81.7
    (b"IS", 0.933), // wiki vpk=54.3
    (b"IT", 1.147), // wiki vpk=92.0
    (b"JE", 1.284), // wiki vpk=222.1
    (b"JM", 0.795), // wiki vpk=35.6
    (b"JO", 1.292), // wiki vpk=277.7
    (b"JP", 1.113), // wiki vpk=83.5
    (b"KE", 1.281), // wiki vpk=206.7
    (b"KG", 0.812), // wiki vpk=38.2
    (b"KH", 0.763), // wiki vpk=30.2
    (b"KI", 0.701), // wiki vpk=5.5
    (b"KM", 0.915), // wiki vpk=51.9
    (b"KN", 1.258), // wiki vpk=155.7
    (b"KP", 0.835), // wiki vpk=41.4
    (b"KR", 1.292), // wiki vpk=279.6
    (b"KW", 1.299), // wiki vpk=516.3
    (b"KY", 1.294), // density 298.3/km²
    (b"KZ", 1.296), // wiki vpk=368.9
    (b"LA", 1.299), // wiki vpk=575.5
    (b"LB", 1.123), // wiki vpk=86.0
    (b"LC", 0.840), // wiki vpk=42.1
    (b"LI", 0.898), // wiki vpk=49.7
    (b"LK", 1.298), // wiki vpk=492.0
    (b"LR", 1.254), // wiki vpk=150.0 [ceic_lr]
    (b"LS", 1.095), // density 75.3/km²
    (b"LT", 0.739), // wiki vpk=25.1
    (b"LU", 1.276), // wiki vpk=191.5
    (b"LV", 0.773), // density 30.2/km²
    (b"LY", 1.160), // wiki vpk=95.9
    (b"MA", 1.143), // wiki vpk=91.1
    (b"MC", 1.300), // density 18680.9/km²
    (b"MD", 1.201), // wiki vpk=111.2
    (b"ME", 0.785), // wiki vpk=34.0
    (b"MF", 1.299), // density 577.4/km²
    (b"MG", 0.701), // wiki vpk=7.5
    (b"MH", 1.287), // density 222.7/km²
    (b"MK", 0.902), // wiki vpk=50.2
    (b"ML", 0.700), // wiki vpk=2.5
    (b"MM", 1.280), // wiki vpk=205.7
    (b"MN", 1.203), // wiki vpk=112.5
    (b"MO", 1.299), // wiki vpk=588.5
    (b"MP", 1.189), // density 100.2/km²
    (b"MR", 0.700), // density 4.7/km²
    (b"MT", 1.284), // wiki vpk=222.3
    (b"MU", 1.299), // density 632.2/km²
    (b"MV", 1.300), // wiki vpk=1408.6
    (b"MW", 1.286), // density 218.2/km²
    (b"MX", 1.286), // wiki vpk=230.9
    (b"MY", 1.277), // wiki vpk=194.5
    (b"MZ", 1.157), // wiki vpk=94.9
    (b"NA", 0.700), // density 3.5/km²
    (b"NC", 0.712), // density 15.7/km²
    (b"NE", 1.201), // wiki vpk=111.6
    (b"NG", 1.285), // wiki vpk=225.0
    (b"NI", 0.970), // density 55.9/km²
    (b"NL", 1.048), // wiki vpk=71.0
    (b"NO", 0.794), // wiki vpk=35.5
    (b"NP", 1.221), // wiki vpk=122.1
    (b"NR", 1.281), // wiki vpk=208.3
    (b"NZ", 1.051), // wiki vpk=71.5 [national_nz]
    (b"OE", 0.835), // density 39.0/km²
    (b"OM", 0.711), // density 15.3/km²
    (b"PA", 0.995), // density 59.3/km²
    (b"PE", 1.275), // wiki vpk=189.5
    (b"PF", 1.121), // density 80.8/km²
    (b"PG", 0.783), // wiki vpk=33.7
    (b"PH", 1.295), // wiki vpk=335.0
    (b"PK", 1.262), // wiki vpk=161.8
    (b"PL", 1.130), // wiki vpk=87.7
    (b"PR", 1.297), // density 363.0/km²
    (b"PS", 1.300), // density 837.1/km²
    (b"PT", 1.175), // wiki vpk=101.0 [national_pt]
    (b"PW", 0.831), // density 38.6/km²
    (b"PY", 0.716), // density 17.1/km²
    (b"QA", 1.275), // wiki vpk=189.0
    (b"RO", 1.244), // wiki vpk=140.1
    (b"RS", 0.920), // wiki vpk=52.6
    (b"RU", 0.878), // wiki vpk=47.1
    (b"RW", 1.268), // wiki vpk=172.4
    (b"SA", 1.249), // wiki vpk=145.1
    (b"SB", 1.295), // wiki vpk=323.5
    (b"SC", 0.861), // wiki vpk=44.9
    (b"SD", 1.259), // wiki vpk=156.6
    (b"SE", 0.829), // wiki vpk=40.6
    (b"SG", 1.292), // wiki vpk=284.8
    (b"SI", 0.770), // wiki vpk=31.5
    (b"SK", 0.959), // wiki vpk=57.7
    (b"SL", 1.292), // wiki vpk=285.4
    (b"SM", 1.274), // wiki vpk=184.7
    (b"SN", 1.078), // wiki vpk=76.4
    (b"SO", 0.700), // wiki vpk=4.0
    (b"SR", 1.283), // wiki vpk=215.7
    (b"SS", 0.717), // density 17.4/km²
    (b"ST", 1.252), // wiki vpk=148.0
    (b"SV", 1.275), // wiki vpk=188.7
    (b"SX", 1.300), // density 1239.4/km²
    (b"SY", 1.232), // density 122.3/km²
    (b"SZ", 1.071), // density 70.9/km²
    (b"TC", 0.909), // density 48.3/km²
    (b"TD", 0.862), // wiki vpk=45.0
    (b"TG", 0.796), // wiki vpk=35.7
    (b"TH", 1.252), // density 140.4/km²
    (b"TJ", 0.709), // wiki vpk=14.7
    (b"TL", 1.165), // density 92.1/km²
    (b"TM", 0.712), // density 15.4/km²
    (b"TN", 1.260), // wiki vpk=158.1
    (b"TO", 1.143), // wiki vpk=90.9
    (b"TR", 1.131), // wiki vpk=87.9
    (b"TT", 1.292), // density 266.2/km²
    (b"TV", 1.296), // density 333.1/km²
    (b"TW", 1.299), // wiki vpk=545.7
    (b"TZ", 1.277), // wiki vpk=193.2
    (b"UA", 0.998), // wiki vpk=63.2
    (b"UG", 1.296), // wiki vpk=374.7
    (b"US", 0.977), // wiki vpk=60.2
    (b"UY", 0.722), // density 19.4/km²
    (b"UZ", 0.740), // wiki vpk=25.4
    (b"VC", 1.292), // density 261.7/km²
    (b"VE", 0.855), // wiki vpk=44.0
    (b"VG", 1.291), // density 255.5/km²
    (b"VI", 1.294), // density 301.2/km²
    (b"VN", 1.293), // wiki vpk=289.0 [national_vn]
    (b"VU", 0.748), // density 25.7/km²
    (b"WS", 0.727), // wiki vpk=21.9
    (b"XC", 1.238), // density 127.3/km²
    (b"XD", 0.755), // density 27.0/km²
    (b"XE", 0.891), // density 46.1/km²
    (b"XF", 1.063), // density 69.6/km²
    (b"XG", 1.087), // density 73.8/km²
    (b"XH", 1.245), // density 133.0/km²
    (b"XI", 0.997), // density 59.6/km²
    (b"XJ", 0.789), // density 32.7/km²
    (b"XL", 0.968), // density 55.7/km²
    (b"XM", 0.869), // density 43.4/km²
    (b"XN", 1.268), // density 162.8/km²
    (b"XO", 1.138), // density 84.8/km²
    (b"XP", 1.170), // density 93.8/km²
    (b"XQ", 1.073), // density 71.3/km²
    (b"XT", 1.029), // density 64.2/km²
    (b"XU", 0.727), // density 20.8/km²
    (b"YE", 1.079), // density 72.4/km²
    (b"ZA", 1.115), // wiki vpk=84.1
    (b"ZF", 0.911), // density 48.5/km²
    (b"ZG", 0.935), // density 51.5/km²
    (b"ZH", 0.917), // density 49.3/km²
    (b"ZI", 0.962), // density 55.0/km²
    (b"ZJ", 0.788), // density 32.6/km²
    (b"ZM", 0.957), // wiki vpk=57.4
    (b"ZQ", 1.014), // density 62.0/km²
    (b"ZT", 1.070), // density 70.7/km²
    (b"ZW", 1.093), // wiki vpk=79.4
];

/// Binary-search lookup. O(log N), N = 242.
pub fn country_scale(iso: &[u8; 2]) -> Option<f64> {
    COUNTRY_SCALES
        .binary_search_by_key(iso, |(k, _)| **k)
        .ok()
        .map(|idx| COUNTRY_SCALES[idx].1)
}

/// Continent scale factor (population-weighted mean of member countries).
/// Returned when a hex has no country arm (e.g. oceanic cells near coasts).
pub fn continent_scale(continent: Continent) -> Option<f64> {
    match continent {
        Continent::Africa => Some(1.057),
        Continent::Asia => Some(1.074),
        Continent::Europe => Some(1.004),
        Continent::NorthAmerica => Some(1.085),
        Continent::Oceania => Some(0.903),
        Continent::SouthAmerica => Some(1.235),
        Continent::Unknown => None,
    }
}
