//! The outdoor units of homes: the air-source heat pumps that heat them and the air conditioners
//! that cool them, by country and climate (research 2026-10-02, building-plant); and the share of
//! shops' and offices' floor that cools (research 2026-10-18, building-plant-hours). A dwelling
//! carries its country's units per household, each running the share of the hours its climate
//! asks for: dev4 gave every dwelling of the world one heat pump at 57 dB(A) all year, where 2 %
//! of British and 60 % of Norwegian households own one, and 88.5 % of Japanese households own 2.37
//! air conditioners.
//!
//! Heating: heat pumps with an outdoor unit per household (EHPA 2024 stock times EurObserv'ER's
//! air share; US RECS 2020 main heating 13.9 %, Canada 13 %; Australia's 49 % reverse-cycle units
//! heat too). They run in proportion to the heating degree days below 16 C (EN 14825): 0.41 of
//! the hours at the UK's 2,200 (heatpumpmonitor, 447 units, compressor 3,590 h a year), as much
//! by night as by day (night power 1.09-1.67 times the day's, night modes 6 dB down), at 60 dB(A)
//! running (labels 55-58 for the common sizes, 1-8 dB more in situ).
//!
//! Cooling: air-conditioning units with an outdoor part per household (US RECS 2020: 66.9 %
//! central, 1.4 % mini-split, 17.3 % window or wall; Canada 41 % central, 4 % mini-split; Japan
//! 2.37, China 1.46 units a household; Korea 86 %, India, Brazil and Mexico 16 %, Indonesia 9 %,
//! South-East Asia 29 %, Saudi Arabia 63 %, the other Gulf states all, South Africa 6 %,
//! Australia 59 %; Europe's households with units, coolinggap.eu). They run hours a day that
//! saturate with the cooling degree days above 18 C, 6 (1 - e^(-CDD/1300)): 1.6 at Chicago's 390
//! (656 full-load hours a year measured in the Midwest), 3.5 at Shanghai's 1,120 (China 3.3), 5.7
//! at Bangkok's 3,830 (Thailand 4.9, Indonesia 6.2, India 6.0); the Gulf's households 12.8 hours
//! whatever the degree days. The day's shape is the measured American one (peak at 17-19 h, night
//! 0.47-0.64 of the day): the day 1.1, the evening 1.5, the night 0.6 times the daily mean. A
//! split or window unit runs at 62 dB(A) (59-65 cooling), a central condenser at 68 (58 variable
//! speed to 72-76 single stage).
//!
//! A country without its own numbers takes its UN M49 region's median of the measured countries
//! (South-East Asia the research's regional 29 %, `REGIONS`);
//! a region without measured countries the 16 % that Brazil, Mexico and India measure, and no
//! heat pumps (outside Europe, North America, Australia and New Zealand the research found no
//! stock of heating heat pumps).

use crate::climate::DegreeDays;
use physics::emission::settlement::PlantUse;

/// Heating degree days (base 16 C) at which a heat pump runs 0.41 of the hours (the UK's 2,200).
const HEATING_DEGREE_DAYS_PER_RUNNING: f64 = 5_370.0;
const HEATING_RUNNING_MAX: f64 = 0.9;
const HEAT_PUMP_LW_DBA: f64 = 60.0;
/// Cooling hours a day of an owned unit: `COOLING_HOURS_MAX` (1 - e^(-CDD / COOLING_DEGREE_DAYS)).
const COOLING_HOURS_MAX: f64 = 6.0;
const COOLING_DEGREE_DAYS: f64 = 1_300.0;
/// The Gulf's households run theirs most of the day whatever the degree days.
const GULF_COOLING_HOURS: f64 = 12.8;
/// The day, evening and night against the daily mean (US metered air conditioning).
const COOLING_SHAPE: [f64; 3] = [1.1, 1.5, 0.6];
const SPLIT_LW_DBA: f64 = 62.0;
const CENTRAL_LW_DBA: f64 = 68.0;
/// Units per household where a region has no measured country: Brazil's, Mexico's and India's.
const WORLD_COOLING_UNITS: f64 = 0.16;

/// A country's heat pumps heating per household, air-conditioning units per household, the share
/// of those that are central condensers, and its households' cooling hours a day when fixed.
struct Plant {
    heating_units: f64,
    cooling_units: f64,
    central_share: f64,
    cooling_hours: Option<f64>,
}

const fn plant(heating_units: f64, cooling_units: f64) -> Plant {
    Plant {
        heating_units,
        cooling_units,
        central_share: 0.0,
        cooling_hours: None,
    }
}

const GULF: Plant = Plant {
    heating_units: 0.0,
    cooling_units: 1.0,
    central_share: 0.0,
    cooling_hours: Some(GULF_COOLING_HOURS),
};

/// Measured countries: heat pumps and cooling units per household, `None` where the research has
/// only the other (the United States, Canada and the Gulf apart).
const COUNTRIES: [([u8; 2], Option<f64>, Option<f64>); 35] = [
    (*b"AT", Some(0.09), Some(0.08)),
    (*b"AU", Some(0.49), Some(0.59)),
    (*b"BE", Some(0.06), Some(0.12)),
    (*b"BG", None, Some(0.61)),
    (*b"BR", None, Some(0.16)),
    (*b"CH", None, Some(0.10)),
    (*b"CN", None, Some(1.46)),
    (*b"CZ", Some(0.06), Some(0.16)),
    (*b"DE", Some(0.04), Some(0.06)),
    (*b"DK", Some(0.19), None),
    (*b"EE", Some(0.32), None),
    (*b"ES", Some(0.09), Some(0.41)),
    (*b"FI", Some(0.47), None),
    (*b"FR", Some(0.20), Some(0.245)),
    (*b"GB", Some(0.02), Some(0.04)),
    (*b"GR", None, Some(0.76)),
    (*b"HR", None, Some(0.55)),
    (*b"HU", None, Some(0.28)),
    (*b"ID", None, Some(0.09)),
    (*b"IE", Some(0.07), None),
    (*b"IN", None, Some(0.16)),
    (*b"IT", Some(0.16), Some(0.56)),
    (*b"JP", None, Some(2.37)),
    (*b"KR", None, Some(0.86)),
    (*b"MT", None, Some(0.84)),
    (*b"MX", None, Some(0.16)),
    (*b"NL", Some(0.07), Some(0.12)),
    (*b"NO", Some(0.60), None),
    (*b"PL", Some(0.04), Some(0.02)),
    (*b"PT", Some(0.08), Some(0.17)),
    (*b"RO", None, Some(0.16)),
    (*b"SE", Some(0.38), None),
    (*b"SI", None, Some(0.44)),
    (*b"SK", Some(0.04), None),
    (*b"ZA", None, Some(0.06)),
];

/// Of the floor of shops, offices, restaurants and bars, the share cooled in 2015 (Heat Roadmap
/// Europe D3.2 section 5.3, from the EU28's sales and stocks of units).
const SERVICE_COOLED: [([u8; 2], f64); 28] = [
    (*b"AT", 0.083),
    (*b"BE", 0.238),
    (*b"BG", 0.283),
    (*b"CY", 0.847),
    (*b"CZ", 0.099),
    (*b"DE", 0.089),
    (*b"DK", 0.074),
    (*b"EE", 0.068),
    (*b"ES", 0.727),
    (*b"FI", 0.113),
    (*b"FR", 0.225),
    (*b"GB", 0.225),
    (*b"GR", 0.871),
    (*b"HR", 0.294),
    (*b"HU", 0.124),
    (*b"IE", 0.052),
    (*b"IT", 0.735),
    (*b"LT", 0.089),
    (*b"LU", 0.112),
    (*b"LV", 0.073),
    (*b"MT", 0.847),
    (*b"NL", 0.101),
    (*b"PL", 0.104),
    (*b"PT", 0.222),
    (*b"RO", 0.293),
    (*b"SE", 0.196),
    (*b"SI", 0.208),
    (*b"SK", 0.145),
];
/// Outside Europe, HRE's saturation: the United States' share where it is hot (reached by Cyprus
/// and Malta), as HRE takes the US share at the same climate for where cooling saturates.
const WORLD_SERVICE_COOLED: f64 = 0.847;

/// UN M49 regions: members, heat pumps and cooling units per household (the medians of the
/// measured members; South-East Asia's regional figure; Sub-Saharan Africa South Africa's), and in
/// Europe the median service floor cooled of [`SERVICE_COOLED`]'s members.
const REGIONS: [(&str, f64, f64, Option<f64>); 11] = [
    // GB 0.02, IE 0.07, DK 0.19, EE 0.32, SE 0.38, FI 0.47, NO 0.60; cooling GB alone; service
    // IE .052 EE .068 LV .073 DK .074 LT .089 FI .113 SE .196 GB .225.
    (
        "AX DK EE FO FI GG IS IE IM JE LV LT NO SJ SE GB",
        0.32,
        0.04,
        Some(0.0815),
    ),
    // AT 0.09, BE 0.06, FR 0.20, DE 0.04, NL 0.07; AT .08 BE .12 FR .245 DE .06 NL .12 CH .10;
    // service AT .083 DE .089 NL .101 LU .112 FR .225 BE .238.
    ("AT BE FR DE LI LU MC NL CH", 0.07, 0.11, Some(0.1065)),
    // PT 0.08, ES 0.09, IT 0.16; MT .84 GR .76 IT .56 HR .55 SI .44 ES .41 PT .17; service SI .208
    // PT .222 HR .294 ES .727 IT .735 MT .847 GR .871.
    (
        "AL AD BA HR GI GR VA IT MT ME MK PT SM RS SI ES XK",
        0.09,
        0.55,
        Some(0.727),
    ),
    // PL 0.04, SK 0.04, CZ 0.06; BG .61 HU .28 CZ .16 RO .16 PL .02; service CZ .099 PL .104 HU
    // .124 SK .145 BG .283 RO .293.
    ("BY BG CZ HU PL MD RO RU SK UA", 0.04, 0.16, Some(0.1345)),
    // Japan 2.37, China 1.46, Korea 0.86.
    ("CN HK MO KP JP MN KR TW", 0.0, 1.46, None),
    ("BN KH ID LA MY MM PH SG TH TL VN", 0.0, 0.29, None),
    // India.
    ("AF BD BT IN IR MV NP PK LK", 0.0, 0.16, None),
    // Mexico and Brazil.
    (
        "AI AG AW BS BB BQ VG KY CU CW DM DO GD GP HT JM MQ MS PR BL KN LC MF VC SX TT TC VI BZ CR \
         SV GT HN MX NI PA AR BO BV BR CL CO EC FK GF GY PY PE GS SR UY VE",
        0.0,
        0.16,
        None,
    ),
    // South Africa, the region's only measured country.
    (
        "IO BI KM DJ ER ET TF KE MG MW MU YT MZ RE RW SC SO SS UG TZ ZM ZW AO CM CF TD CG CD GQ GA \
         ST BW SZ LS NA ZA BJ BF CV CI GM GH GN GW LR ML MR NE NG SH SN SL TG",
        0.0,
        0.06,
        None,
    ),
    // Australia's units heat and cool in New Zealand too.
    ("AU NZ NF", 0.49, 0.59, None),
    // Canada's for its neighbours.
    ("BM GL PM", 0.13, 0.45, None),
];

fn is_member(members: &str, code: [u8; 2]) -> bool {
    members
        .split_ascii_whitespace()
        .any(|member| member.as_bytes() == code)
}

/// The share of the floor of shops, offices, restaurants and bars cooled in `country_iso` (ISO 3166
/// alpha-2, little-endian): its own, else its European region's median, else HRE's saturation.
pub fn cooled_service_share(country_iso: u16) -> f64 {
    let code = country_iso.to_le_bytes();
    SERVICE_COOLED
        .iter()
        .find(|(iso, _)| *iso == code)
        .map(|&(_, share)| share)
        .or_else(|| {
            REGIONS
                .iter()
                .find(|(members, ..)| is_member(members, code))
                .and_then(|region| region.3)
        })
        .unwrap_or(WORLD_SERVICE_COOLED)
}

fn country_plant(country_iso: u16) -> Plant {
    let code = country_iso.to_le_bytes();
    match &code {
        b"US" => {
            return Plant {
                heating_units: 0.139,
                cooling_units: 0.856,
                central_share: 0.669 / 0.856,
                cooling_hours: None,
            };
        }
        b"CA" => {
            return Plant {
                heating_units: 0.13,
                cooling_units: 0.45,
                central_share: 0.41 / 0.45,
                cooling_hours: None,
            };
        }
        b"AE" | b"BH" | b"KW" | b"OM" | b"QA" => return GULF,
        b"SA" => {
            return Plant {
                cooling_units: 0.63,
                ..GULF
            };
        }
        _ => {}
    }
    let region = REGIONS
        .iter()
        .find(|(members, ..)| is_member(members, code));
    let (region_heating, region_cooling) = region
        .map_or((0.0, WORLD_COOLING_UNITS), |&(_, heating, cooling, _)| {
            (heating, cooling)
        });
    let measured = COUNTRIES.iter().find(|(iso, _, _)| *iso == code);
    plant(
        measured.and_then(|row| row.1).unwrap_or(region_heating),
        measured.and_then(|row| row.2).unwrap_or(region_cooling),
    )
}

/// The outdoor units of one dwelling in `country_iso` (ISO 3166 alpha-2, little-endian) under
/// `climate`'s degree days: heating, cooling by split or window units, cooling by central units.
pub fn dwelling_plant(country_iso: u16, climate: DegreeDays) -> [PlantUse; 3] {
    let plant = country_plant(country_iso);
    let heating = (climate.heating / HEATING_DEGREE_DAYS_PER_RUNNING).min(HEATING_RUNNING_MAX);
    let hours = plant.cooling_hours.unwrap_or_else(|| {
        COOLING_HOURS_MAX * (1.0 - (-climate.cooling / COOLING_DEGREE_DAYS).exp())
    });
    let cooling = COOLING_SHAPE.map(|shape| (hours / 24.0 * shape).min(1.0));
    [
        PlantUse {
            units_per_dwelling: plant.heating_units,
            running: [heating; 3],
            lw_dba: HEAT_PUMP_LW_DBA,
        },
        PlantUse {
            units_per_dwelling: plant.cooling_units * (1.0 - plant.central_share),
            running: cooling,
            lw_dba: SPLIT_LW_DBA,
        },
        PlantUse {
            units_per_dwelling: plant.cooling_units * plant.central_share,
            running: cooling,
            lw_dba: CENTRAL_LW_DBA,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics::emission::settlement::plant_sound_power;

    fn iso(code: &[u8; 2]) -> u16 {
        u16::from_le_bytes(*code)
    }

    /// A Prague home (2,640 heating degree days, 40 cooling): 6 % of heat pumps at 0.49 of the
    /// hours, 16 % of air conditioners an hour a month; 12 dB under dev4's 57 dB(A) by day.
    /// Bangkok's (3,830 cooling): 29 % of units 5.7 hours a day, louder in the evening.
    #[test]
    fn homes_carry_their_countrys_units_at_their_climates_hours() {
        let prague = plant_sound_power(
            &dwelling_plant(
                iso(b"CZ"),
                DegreeDays {
                    heating: 2_640.0,
                    cooling: 40.0,
                },
            ),
            1.0,
        )
        .unwrap();
        assert!((prague.day_dba - 45.0).abs() < 0.1, "{prague:?}");
        assert!(prague.night_offset_db.abs() < 0.5);
        let bangkok = plant_sound_power(
            &dwelling_plant(
                iso(b"TH"),
                DegreeDays {
                    heating: 0.0,
                    cooling: 3_830.0,
                },
            ),
            1.0,
        )
        .unwrap();
        assert!((bangkok.day_dba - 50.8).abs() < 0.1, "{bangkok:?}");
        assert!(bangkok.evening_offset_db > 1.0 && bangkok.night_offset_db < -2.0);
        // Dubai runs its units 12.8 hours a day; a Houston home's central condenser rules.
        let dubai = dwelling_plant(
            iso(b"AE"),
            DegreeDays {
                heating: 0.0,
                cooling: 3_430.0,
            },
        );
        assert!((dubai[1].running[0] - 12.8 / 24.0 * 1.1).abs() < 1e-9);
        let houston = dwelling_plant(
            iso(b"US"),
            DegreeDays {
                heating: 700.0,
                cooling: 2_100.0,
            },
        );
        assert!(houston[2].units_per_dwelling > 0.6 && houston[2].lw_dba == 68.0);
    }

    /// Countries without numbers take their region's (Slovenia measured cooling only: heating
    /// Southern Europe's 0.09); regions without measured countries 16 % of cooling units.
    #[test]
    fn countries_fall_back_to_their_region_then_the_world() {
        let slovenia = country_plant(iso(b"SI"));
        assert_eq!(
            (slovenia.heating_units, slovenia.cooling_units),
            (0.09, 0.44)
        );
        let latvia = country_plant(iso(b"LV"));
        assert_eq!((latvia.heating_units, latvia.cooling_units), (0.32, 0.04));
        let egypt = country_plant(iso(b"EG"));
        assert_eq!((egypt.heating_units, egypt.cooling_units), (0.0, 0.16));
        assert_eq!(country_plant(0).cooling_units, WORLD_COOLING_UNITS);
        for (iso, _, _) in COUNTRIES {
            assert!(
                REGIONS.iter().any(|(members, ..)| is_member(members, iso))
                    || [*b"US", *b"CA"].contains(&iso),
                "{}",
                String::from_utf8_lossy(&iso)
            );
        }
    }

    /// The EU's countries cool their service floor as HRE models it, Europe's others as their
    /// region's median, the rest of the world at HRE's saturation; the regional medians are the
    /// table's.
    #[test]
    fn service_floor_cools_by_country_then_region_then_saturation() {
        assert_eq!(cooled_service_share(iso(b"CZ")), 0.099);
        assert_eq!(cooled_service_share(iso(b"ES")), 0.727);
        assert_eq!(cooled_service_share(iso(b"NO")), 0.0815);
        assert_eq!(cooled_service_share(iso(b"UA")), 0.1345);
        for code in [b"US", b"JP", b"IN", b"BR", b"AE"] {
            assert_eq!(cooled_service_share(iso(code)), WORLD_SERVICE_COOLED);
        }
        for (members, _, _, median) in REGIONS {
            let mut shares: Vec<f64> = SERVICE_COOLED
                .iter()
                .filter(|(code, _)| is_member(members, *code))
                .map(|&(_, share)| share)
                .collect();
            shares.sort_by(f64::total_cmp);
            let middle = shares.len() / 2;
            let expected = match shares.len() {
                0 => None,
                n if n % 2 == 1 => Some(shares[middle]),
                _ => Some((shares[middle - 1] + shares[middle]) / 2.0),
            };
            assert_eq!(
                median.map(|m| (m * 1e4).round()),
                expected.map(|e| (e * 1e4).round()),
                "{members}"
            );
        }
    }
}
