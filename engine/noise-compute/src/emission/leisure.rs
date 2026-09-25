//! Leisure-area (sports / play / open-air hospitality) noise emission. These
//! OSM features carry NO `building=*` (a padel court / playground / café terrace
//! is not a building), so they spill to their own `leisure.arrow` and
//! [`crate::normalize::prepare_leisure_points`] discretises them through the SAME
//! area grid + area-law as buildings — UNIFIED: `lw = settlement::area_lw(..)`
//! over the polygon area (a bigger court / terrace is louder). The ONLY thing
//! that differs from a building is PHYSICAL: the source sits on the open ground
//! (~1.5 m, voices/rackets, not roof plant) with no floors. Per-sport spectrum +
//! day/evening/night pattern below.
//!
//! Per-sport calibration sources are cited inline. All `lw` are radiated dB(A)
//! (`a_weighted_total(bands) == lw`) and bake the duty cycle / seasonality into a single annualized level
//! because the engine has no month axis.
//! PROP-MEAS = no clean measured value found; a conservative placeholder ships
//! flagged and is never presented as measured.

use crate::types::NUM_BANDS;

/// Leisure `sport`/kind class ids written by `osm-extract::spill` into
/// `leisure.arrow`. Stable once shipped: the arrow stores the raw u8, so a new
/// id is a new file contract (`leisure_v5` carries the area classes below
/// plus AGP 12, motorsport 10 and shooting 11). The formula classes subdivide at
/// read time from the retained `sport` / `shooting` tags (see
/// [`MotorsportSubtype`] / [`ShootingSubtype`]) — the extractor keeps one
/// class per activity, not one per vehicle or weapon.
/// 0 is the generic-pitch default so an untyped `leisure=pitch` still emits.
pub const PITCH: u8 = 0;
pub const PADEL: u8 = 1;
pub const TENNIS: u8 = 2;
pub const BASKETBALL: u8 = 3;
pub const PLAYGROUND: u8 = 4;
pub const POOL: u8 = 5;
/// Open-air hospitality seating (`amenity=biergarten`, `outdoor_seating=yes`,
/// `leisure=outdoor_seating`) — patron voices, area-scaled like everything else.
pub const OUTDOOR_SEATING: u8 = 6;
/// Stadium / large sports ground — pitch + crowd + PA; rare match-day events,
/// kept at pitch level because rare match days must not be over-weighted.
pub const STADIUM: u8 = 7;
/// Open car park (`amenity=parking` with no `building` tag) — manoeuvring cars,
/// doors, trolleys on open ground. Nothing stands there, so it never screens;
/// the structure classes (`building=parking|garage`, settlement class 7) keep
/// their vent fans and their walls.
pub const CAR_PARK: u8 = 8;
/// Street-side / lane parking — the same movements on a strip with no aisle of
/// its own, so it holds a space per 13.3 m² where a lot needs 23.8.
pub const CAR_PARK_STREET: u8 = 9;
/// Motorsport: raceway lines, track areas and points. The per-vehicle level
/// comes from the row's `sport` tag ([`motorsport_subtype`]); raceway lines
/// carry the emission and their enclosing polygon goes silent (resolved by
/// the readers, which see the whole square).
pub const MOTORSPORT: u8 = 10;
/// Shooting: outdoor ranges. The per-shot level comes from the row's
/// `shooting` tags and name ([`shooting_subtype`]).
pub const SHOOTING: u8 = 11;
/// Floodlit artificial-turf pitch (`leisure=pitch` + `surface=artificial_turf`).
/// Same player-voice anchor as grass, booked ~40 h/week year-round, so its
/// annual duty runs ~10 dB hotter. Takes the next id after the two formula
/// classes (10 motorsport, 11 shooting).
pub const AGP: u8 = 12;

/// True for the two formula classes (motorsport/shooting): they carry a
/// class-TOTAL annual Lw with industrial reach, not the area law.
pub fn is_formula_class(sport: u8) -> bool {
    matches!(sport, MOTORSPORT | SHOOTING)
}

/// Map an OSM `sport=*` value (lower-cased) to an area-law class id.
/// The canonical map — `osm-extract::ids` transcribes it. Motor and shooting
/// sports are NOT here: the extractor classifies them to 10/11 itself and
/// the readers subdivide them from the raw tags below.
pub fn sport_class(sport: &str) -> Option<u8> {
    Some(match sport {
        "padel" => PADEL,
        "tennis" => TENNIS,
        "basketball" | "netball" | "handball" => BASKETBALL,
        // Ball-sport pitches all share the football/pitch anchor.
        "soccer" | "football" | "american_football" | "rugby" | "rugby_union" | "rugby_league"
        | "field_hockey" | "hockey" | "baseball" | "cricket" | "multi" => PITCH,
        "swimming" => POOL,
        _ => return None,
    })
}

/// Per-leisure-area emission profile — the SAME area-law as a building
/// (`settlement::area_lw`): a low fixed floor plus a per-m² term over the
/// polygon area. Source height is fixed (~1.5 m) in the prep path; no floors.
pub struct LeisureProfile {
    /// Minimum plant floor (dB) — the shared `settlement::area_lw` form. Leisure
    /// has no real fixed plant, so this is a low common floor; the per-m² term
    /// carries the level.
    pub lw_fixed: f64,
    /// Per-m² of leisure AREA (dB/m²) — the SAME area-law as buildings
    /// (`settlement::area_lw`). Replaces the old capacity scaling: the OSM polygon
    /// area IS the source size (a 2-court padel / a 300-seat garden is bigger ⇒
    /// louder), so the whole emission model is now unified on AREA, not capacity.
    pub lw_per_m2: f64,
    /// Typical facility footprint (m²) the anchor below assumes — also the
    /// fallback area for a leisure NODE with no polygon (a court ~200, a café
    /// terrace ~50). `area_lw(lw_fixed, lw_per_m2, ref_area_m2)` == the cited anchor.
    pub ref_area_m2: f64,
    /// Relative dB per 8-band [63..8k]; A-sum-normalized to the radiated lw.
    pub spectrum: [f64; NUM_BANDS],
    pub evening_offset: f64,
    pub night_offset: f64,
    /// Square metres of AREA per parking space, for the one kind of source that
    /// HAS spaces. It carries the Parkplatzlärmstudie's searching-traffic term;
    /// a court or a terrace leaves it `None`.
    pub m2_per_space: Option<f64>,
}

impl LeisureProfile {
    /// Parkplatzlärmstudie Formula 3, the level a lot gains from the traffic
    /// driving through it and hunting for a free space:
    /// `K_D = 2.5·lg(B − 9)` dB(A) above ten spaces, zero at or below them.
    fn searching_traffic_db(&self, area_m2: f64) -> f64 {
        match self.m2_per_space {
            Some(per_space) if area_m2 / per_space > 10.0 => {
                2.5 * (area_m2 / per_space - 9.0).log10()
            }
            _ => 0.0,
        }
    }
}

/// Measured octave shape of car-park noise: the Parkplatzlärmstudie's own
/// 30-minute spectrum of parking movements (Tab. 25, A-weighted band levels
/// −17.7 −17.1 −12.8 −8.7 −5.3 −4.6 −9.4 −19.6 dB(A) below the total), turned
/// back into unweighted band levels by subtracting [`crate::constants::A_WEIGHTING`]
/// — the engine A-weights the spectrum itself. Idling engines carry the 63 Hz
/// band; door slams the 1–2 kHz.
const CAR_PARK_SPECTRUM: [f64; NUM_BANDS] = [8.5, -1.0, -4.2, -5.5, -5.3, -5.8, -10.4, -18.5];

/// Emission profile by leisure class id (see the `pub const`s).
///
/// ANNUALIZATION (the honest weak spot — END/CNOSSOS does NOT model sport; no
/// standard prescribes a yearly Lden for a court). Each `lw_per_m2` below is an
/// ACTIVE peak-use sound power (measured, cited per arm) MINUS a transparent
/// duty cut to a year-average Lden:
///   annual Lden = active Lw  −  seasonal  −  daily-duty
///     seasonal  : outdoor play ~6 months/yr → −3 dB (pool ~4 mo → −5; play ~−2)
///     daily-duty: a court is in active use ~6 of 24 h (day+evening) → −6 dB
///                 (stadium: match days only ~25/yr → −12)
/// Net ≈ −9 dB for outdoor sport except the two pitch classes, whose arms
/// annualize from published weekly hours instead. Assumptions are LISTED on
/// /about (nothing invented). Indoor halls are indistinguishable in OSM →
/// treated as outdoor (a stated limitation). PROP-MEAS = no clean measured
/// Lw; a flagged estimate.
///
/// Active anchors (pre-annualization): padel 90 (racket "pock" on glass,
/// padelcreations + Higgins); tennis 84 (LFmax 58.4/strike, TU München);
/// football pitch 97.9 over a 100×64 m pitch (58 LAeq,1h @10 m, Sport
/// England AGP, read as an area source); basketball tennis−6 at its
/// reference court (UBC/BKL); playground PROP-MEAS; pool PROP-MEAS;
/// outdoor seating 71 dB(A)/guest (Lärmfibel Biergärten).
pub fn leisure_profile(sport: u8) -> LeisureProfile {
    // All leisure shares a low fixed floor; `lw_per_m2` over the polygon area
    // carries the level (shared `settlement::area_lw`). Each anchor in the
    // comment = area_lw(FLOOR, lw_per_m2, ref_area_m2) = the year-average Lden.
    const FLOOR: f64 = 40.0;
    match sport {
        PADEL => LeisureProfile {
            // active 90 (racket "pock" on glass; padelcreations + Higgins) − 9
            // annual (−3 season −6 duty) → year Lden 81 @ ~200 m². The 2024–26
            // complaint class; stays the loudest sport. HF-weighted, impulsive.
            lw_fixed: FLOOR,
            lw_per_m2: 58.0,
            ref_area_m2: 200.0,
            spectrum: [-6.0, -4.0, -2.0, -1.0, 0.0, 1.0, 2.0, 1.0],
            evening_offset: 0.0, // plays 07–23; evening is peak
            night_offset: -15.0,
            m2_per_space: None,
        },
        TENNIS => LeisureProfile {
            // active 84 (LFmax 58.4/strike, TU München) − 9 annual (−3 season
            // −6 duty) → year Lden 74 @ ~260 m². Indoor halls look the same in
            // OSM → treated as outdoor (a stated /about limitation).
            lw_fixed: FLOOR,
            lw_per_m2: 50.0,
            ref_area_m2: 260.0,
            spectrum: [-5.0, -4.0, -2.0, -1.0, 0.0, 1.0, 1.0, 0.0],
            evening_offset: -3.0,
            night_offset: -20.0,
            m2_per_space: None,
        },
        BASKETBALL => LeisureProfile {
            // active tennis−6 (ball bounce on hard court) − 9 annual → year Lden
            // 68 @ ~420 m².
            lw_fixed: FLOOR,
            lw_per_m2: 42.0,
            ref_area_m2: 420.0,
            spectrum: [-4.0, -3.0, -1.0, 0.0, 0.0, 0.0, 0.0, -1.0],
            evening_offset: -3.0,
            night_offset: -20.0,
            m2_per_space: None,
        },
        PLAYGROUND => LeisureProfile {
            // child play (PROP-MEAS — no clean per-child Lw) − 8 annual (−2
            // weather −6 duty; used more of the year than a court) → year Lden
            // 71 @ ~200 m². Day-heavy.
            lw_fixed: FLOOR,
            lw_per_m2: 48.0,
            ref_area_m2: 200.0,
            spectrum: [-3.0, -1.0, 1.0, 2.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -5.0,
            night_offset: -25.0,
            m2_per_space: None,
        },
        POOL => LeisureProfile {
            // outdoor lido splash/voice (PROP-MEAS) − 9 annual (−5 summer-only
            // May–Sep −4 duty) → year Lden 76 @ ~400 m².
            lw_fixed: FLOOR,
            lw_per_m2: 50.0,
            ref_area_m2: 400.0,
            spectrum: [-3.0, -2.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            evening_offset: -5.0,
            night_offset: -25.0,
            m2_per_space: None,
        },
        OUTDOOR_SEATING => LeisureProfile {
            // beer garden / café terrace patron voices (Lärmfibel/VDI 3770 71
            // dB(A)/guest raised speech). Annualized HARD (~−16): warm season only +
            // meal-time hours (not all day) + conversational (not continuous). And
            // these are mostly bare `outdoor_seating=yes` NODES with no size (~84 %
            // of them), so a node assumes a SMALL ~12 m² terrace (a few tables) → ~66
            // dB — not a 50-seat beer garden. A mapped 50 m² terrace → ~72, 200 m² →
            // ~78. (Was 65 / 50 m² → a flat 82 that lit the whole map as loud dots.)
            lw_fixed: FLOOR,
            lw_per_m2: 55.0,
            ref_area_m2: 12.0,
            spectrum: [-2.0, -1.0, 1.0, 2.0, 1.0, 0.0, -3.0, -6.0],
            evening_offset: 0.0,
            night_offset: -15.0,
            m2_per_space: None,
        },
        STADIUM => LeisureProfile {
            // pitch + crowd/PA, match days ONLY (~25/yr) − 12 duty → year Lden 78
            // @ ~7000 m². Do NOT over-weight (rare events, big footprint).
            lw_fixed: FLOOR,
            lw_per_m2: 40.0,
            ref_area_m2: 7000.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -4.0],
            evening_offset: -3.0,
            night_offset: -12.0,
            m2_per_space: None,
        },
        CAR_PARK => LeisureProfile {
            // Parkplatzlärmstudie (Bayerisches Landesamt für Umwelt, 6th ed.
            // 2007): one movement per hour radiates L_W0 = 63 dB(A) per parking
            // space, and an overground lot in a residential area sees N = 0.40
            // movements per space and hour by day (06–22) and 0.05 by night
            // (22–06, Tab. 33). One space costs 23.8 m² of lot — the median of
            // 3,050 OSM `parking=surface` polygons carrying `capacity` in eight
            // cities (p25 18.7, p75 30.6; research/parking-noise-20260919). The
            // area law's per-m² term is then 63 + 10·lg(0.40 / 23.8) = 45.3, and
            // `m2_per_space` adds the study's searching-traffic term on top.
            // The study's two blocks are re-averaged onto this engine's clock
            // (day 07–19, evening 19–23, night 23–07): evening holds 3 h of the
            // day rate and 1 h of the night rate → 10·lg(0.3125/0.40) = −1.1 dB;
            // night holds 7 h of 0.05 and the 06–07 hour of 0.40 →
            // 10·lg(0.09375/0.40) = −6.3 dB. KPA = 0 (Tab. 34: residential,
            // visitor and employee lots); the KI = +4 impulse surcharge is a
            // TA Lärm RATING penalty, not radiated energy, so it stays out.
            // KNOWN LOW: a shopping-centre lot turns over about 2.5× faster
            // (Tab. 33 sales-area rates) → roughly 4 dB under until a lot is
            // typed by the function it serves.
            lw_fixed: FLOOR,
            lw_per_m2: 45.3,
            ref_area_m2: 1000.0, // 42 spaces → day Lw 75.3 + K_D 3.8 = 79.1
            spectrum: CAR_PARK_SPECTRUM,
            evening_offset: -1.1,
            night_offset: -6.3,
            m2_per_space: Some(23.8),
        },
        CAR_PARK_STREET => LeisureProfile {
            // The same movements and the same L_W0, on a strip that borrows the
            // street as its aisle: 13.3 m² per space (median of 6,424 OSM
            // `parking=street_side` polygons with `capacity`; `parking=lane` is
            // 12.5 over 1,026 and rides along 0.3 dB low). So the per-m² term is
            // 63 + 10·lg(0.40 / 13.3) = 47.8; a strip usually holds fewer than
            // the ten spaces the searching-traffic term needs.
            lw_fixed: FLOOR,
            lw_per_m2: 47.8,
            ref_area_m2: 100.0, // ~8 spaces → day Lw 67.8, below the K_D threshold
            spectrum: CAR_PARK_SPECTRUM,
            evening_offset: -1.1,
            night_offset: -6.3,
            m2_per_space: Some(13.3),
        },
        // PITCH (0) — grass (or unknown-surface) ball-sport pitch, the class an
        // untyped `leisure=pitch` gets. Active 97.9 dB(A) over 6400 m²: the
        // Sport England AGP Acoustics DGN (2015) typical free-field 58 dB
        // LAeq,1h at 10 m from the sideline halfway (player voices, in use),
        // back-calculated through the hemispherical incoherent area integral
        // (−1.8 dB for 100×64 m). Duty from published use: natural turf
        // tolerates 3–6 h/week (Sport England Natural Turf DGN) → 5 h over a
        // September–May season plus summer (40 weeks), 4 h by day + 1 h on
        // summer evenings → day −14.4, evening −15.6 rel active; night silent
        // (unlit). Year Lden ~83.5 @ 7000 m².
        PITCH => LeisureProfile {
            lw_fixed: FLOOR,
            lw_per_m2: 45.4,
            ref_area_m2: 7000.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -4.0],
            evening_offset: -1.2,
            night_offset: -25.0,
            m2_per_space: None,
        },
        // AGP (12) — the same voice anchor, booked like an artificial-turf
        // pitch: 40 h/week (Sport England Hybrid Pitch Year-4: 3G AGP modelled
        // use), year-round, on the documented peak pattern (weekday evenings +
        // weekends; the 34 peak hours hold 26 day + 8 evening, scaled to 40)
        // → day −4.4, evening −4.7 rel active; night silent (floodlights off
        // ~22:00). Year Lden ~93.8 @ 7000 m².
        AGP => LeisureProfile {
            lw_fixed: FLOOR,
            lw_per_m2: 55.4,
            ref_area_m2: 7000.0,
            spectrum: [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -4.0],
            evening_offset: -0.3,
            night_offset: -25.0,
            m2_per_space: None,
        },
        // An id this engine does not know (13+, or a formula class passed
        // here instead of through its subtype emission — a new class bumps
        // the contract). We cannot say what it is, so it says nothing: `lw`
        // lands under the `prepare_leisure_points` audibility gate for any
        // area, exactly as `settlement::SILENT` does. Guessing "sports
        // pitch" would put a plausible, wrong level on the map instead.
        _ => LeisureProfile {
            lw_fixed: f64::MIN,
            lw_per_m2: f64::MIN,
            ref_area_m2: 1.0,
            spectrum: [0.0; NUM_BANDS],
            evening_offset: 0.0,
            night_offset: 0.0,
            m2_per_space: None,
        },
    }
}

/// Day-period hours per year (12 h × 365) — the annualizer motorsport duty
/// spreads active hours over, and ×3600 s the shooting annualizer.
pub const DAY_PERIOD_HOURS_PER_YEAR: f64 = 4_380.0;

/// Default motorsport activity without permit data: 100 days × 6 h, stated on
/// /about. Bounded below by the UK permitted-development 14 days/yr (CIEH
/// 2003) and event noise-day counts (Zandvoort 12); replaced from
/// permits/registers where available.
pub const MOTORSPORT_DEFAULT_ACTIVE_HOURS: f64 = 600.0;

/// Default civil-range activity without register data: 20,000 shots/yr,
/// stated on /about (Zürich range register 64-ZH carries per-range shots and
/// half-days as the replacement source). Bounded below by the UK 28-day clay
/// rule (CIEH 2003).
pub const SHOOTING_DEFAULT_SHOTS_PER_YEAR: f64 = 20_000.0;

/// Pink-noise reference spectrum (unweighted, rel 1 kHz) — the octave shape
/// REP-0310 §4.2.3 mandates for motorsport propagation.
const MOTORSPORT_SPECTRUM: [f64; NUM_BANDS] = [12.0, 9.0, 6.0, 3.0, 0.0, -3.0, -6.0, -9.0];

/// Single-shot octave spectra (unweighted, rel 1 kHz): RIVM Defensie emission
/// table 2024-10-10 (Omgevingsregeling bijlage XVIIIc data), energy-summed
/// over the sphere per band — Glock 9 mm BALL (ID 40), Accuracy AW .308 Ball
/// (ID 274), shotgun 12 ga No. 7 (ID 106, the clay shot). The table stops at
/// 4 kHz; the 8 kHz entries continue the 2→4 kHz slope (marked EST). Shots are
/// directional (9 mm reads +4.8 forward / −7 rear of the omni sum) — the
/// engine has no direction axis, so downrange is underestimated and uprange
/// overestimated by that much.
const SHOT_SPECTRUM_RIFLE: [f64; NUM_BANDS] = [-15.0, -7.3, -0.2, 2.4, 0.0, -4.4, -5.7, -7.0]; // 8k EST
const SHOT_SPECTRUM_PISTOL: [f64; NUM_BANDS] = [-25.4, -16.4, -7.5, -1.2, 0.0, -5.8, -9.7, -13.6]; // 8k EST
const SHOT_SPECTRUM_SHOTGUN: [f64; NUM_BANDS] = [-15.6, -7.5, -0.5, 1.4, 0.0, -3.4, -6.1, -8.8]; // 8k EST

/// A formula-class emission: annual day Lw (a TOTAL, not per-area — the prep
/// path spreads it over the row geometry), spectrum, and day-only offsets.
/// Evening/night are effectively silent (−50): racing and shooting are
/// daytime activities; floodlit night races are unmodelled.
#[derive(Debug, Clone, Copy)]
pub struct FormulaEmission {
    pub lw_day: f64,
    pub spectrum: [f64; NUM_BANDS],
    pub evening_offset: f64,
    pub night_offset: f64,
}

/// Motorsport sub-type of a class-10 row, read from its raw `sport` tag.
/// Each arm carries its LW(1) provenance: the per-vehicle energy-equivalent
/// level is UBA Austria REP-0310 Table 6 (after LfU Bayern 1999) except where
/// noted, and n the simultaneously driving vehicles at full occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotorsportSubtype {
    /// Circuit car racing, incl. autocross/rallycross (the touring-car 116
    /// proxy — REP-0310 has no road-racing car class; autocross shares it
    /// until a dedicated value is evidenced). n = 15, the club-race grid.
    Circuit,
    /// Motocross (supermoto rides along — same machines on tarmac). n = 7.
    Motocross,
    /// Kart tracks (Pop-Kart national 118 — the race use; rental-only
    /// operation is ~17 dB lower). n = 8.
    Kart,
    /// Speedway. n = 4, the FIM heat size.
    Speedway,
    /// Motorcycle trials. n = 2. LW(1) 95 is the w7 value without a
    /// published source — the weakest motorsport anchor; it also overstates
    /// the rarer bicycle-trial parks (OSM does not split `sport=trial`).
    Trial,
    /// Untyped motorsport (`sport=motor`, unknown values): the touring-car
    /// proxy at n = 10, the honest middle.
    Other,
    /// Radio-controlled cars (`sport=rc_car` on a raceway): toy machines,
    /// not race vehicles — no formula emission.
    Silent,
}

impl MotorsportSubtype {
    /// Per-vehicle LW(1) [dB(A)] and simultaneous vehicle count.
    fn lw1_n(self) -> Option<(f64, f64)> {
        match self {
            Self::Circuit => Some((116.0, 15.0)),
            Self::Motocross => Some((114.0, 7.0)),
            Self::Kart => Some((118.0, 8.0)),
            Self::Speedway => Some((139.0, 4.0)),
            Self::Trial => Some((95.0, 2.0)),
            Self::Other => Some((116.0, 10.0)),
            Self::Silent => None,
        }
    }
}

/// Sub-type of a class-10 row from its raw `sport` tag value (any case;
/// `-`/space spellings accepted). Multi-values (`karting;motocross`) resolve
/// to the loudest annual Lw — the same argmax the extractor uses for
/// multi-sport area rows — except a purely radio-controlled value, which is
/// [`MotorsportSubtype::Silent`]. Unknown or empty tags are
/// [`MotorsportSubtype::Other`].
pub fn motorsport_subtype(sport_tag: &str) -> MotorsportSubtype {
    let mut best: Option<(MotorsportSubtype, f64)> = None;
    let mut silent = false;
    for token in sport_tag.split(';') {
        let token = token.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        let subtype = match token.as_str() {
            "autocross" | "rallycross" | "car_racing" | "formula_one" | "road_racing"
            | "drifting" | "stockcar" | "auto_racing" | "stock_car_racing" | "drag_racing" => {
                MotorsportSubtype::Circuit
            }
            "motocross" | "supermoto" | "enduro" => MotorsportSubtype::Motocross,
            "karting" | "kart" | "go_kart" => MotorsportSubtype::Kart,
            "speedway" => MotorsportSubtype::Speedway,
            "trial" => MotorsportSubtype::Trial,
            "motor" | "motorsport" | "motor_sports" | "motorcycle" => MotorsportSubtype::Other,
            "rc_car" | "rc_racing" | "radiocontrol" | "radio_control" => MotorsportSubtype::Silent,
            _ => continue,
        };
        // A real vehicle anywhere in the value beats radio-controlled: a
        // mixed site still races.
        let Some((lw1, n)) = subtype.lw1_n() else {
            silent = true;
            continue;
        };
        let loud = lw1 + 10.0 * n.log10();
        if best.is_none_or(|(_, best)| loud > best) {
            best = Some((subtype, loud));
        }
    }
    match best {
        Some((subtype, _)) => subtype,
        None if silent => MotorsportSubtype::Silent,
        None => MotorsportSubtype::Other,
    }
}

/// Formula emission of a motorsport sub-type: annual Lw = LW(1) + 10·lg(n) +
/// 10·lg(active hours / 4,380), day-only, at the class-default 100 days × 6 h.
/// [`MotorsportSubtype::Silent`] has no emission (`None`).
pub fn motorsport_emission(subtype: MotorsportSubtype) -> Option<FormulaEmission> {
    let (lw1, n) = subtype.lw1_n()?;
    Some(FormulaEmission {
        lw_day: lw1 + 10.0 * n.log10()
            + 10.0 * (MOTORSPORT_DEFAULT_ACTIVE_HOURS / DAY_PERIOD_HOURS_PER_YEAR).log10(),
        spectrum: MOTORSPORT_SPECTRUM,
        evening_offset: -50.0,
        night_offset: -50.0,
    })
}

/// Shooting sub-type of a class-11 row, read from its `shooting` tags and
/// name. LE is the RIVM Defensie table sphere sum (see the spectra above),
/// ASSUMED to be the per-shot energy level (the table states no reference
/// quantity; TNO 2014-R10135 would confirm it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShootingSubtype {
    /// Rifle (.308, LE 139.0) — also the default for an untyped outdoor
    /// range (conservative: the loudest common discipline).
    Rifle,
    /// Pistol (9 mm, LE 133.6).
    Pistol,
    /// Shotgun (12 ga clay shot, LE 134.8).
    Shotgun,
    /// A near-silent discipline (archery, paintball, air guns): no firearm
    /// emission, so the row stays out rather than sounding like a rifle.
    Silent,
}

/// Sub-type of a class-11 row from its `shooting=*` tag, its `shooting:*`
/// detail values, and the row name. Tags beat the name; across several
/// firearm disciplines the loudest LE wins (a mixed range still fires guns);
/// an untyped range defaults to [`ShootingSubtype::Rifle`]. Tag values follow
/// the [OSM `shooting` key](https://wiki.openstreetmap.org/wiki/Key:shooting)
/// (`rifle`, `pistol`, `clay_pigeon`, `paintball`, …).
pub fn shooting_subtype(
    shooting: Option<&str>,
    details: &[&str],
    name: &str,
) -> ShootingSubtype {
    fn firearm(token: &str) -> Option<ShootingSubtype> {
        Some(match token {
            "rifle" => ShootingSubtype::Rifle,
            "pistol" | "handgun" | "ipsc" => ShootingSubtype::Pistol,
            "shotgun" | "clay" | "clay_pigeon" | "claypigeon" | "skeet" | "trap" => {
                ShootingSubtype::Shotgun
            }
            _ => return None,
        })
    }
    fn quiet(token: &str) -> bool {
        matches!(
            token,
            "archery" | "crossbow" | "paintball" | "airsoft" | "laser" | "laser_tag"
                | "lasertag" | "air_gun" | "airgun" | "air_rifle" | "air_pistol"
                | "blowgun" | "indoor" | "indoor_range" | "virtual"
        )
    }
    let mut tokens: Vec<String> = Vec::new();
    if let Some(shooting) = shooting {
        tokens.extend(
            shooting
                .split(';')
                .map(|token| token.trim().to_ascii_lowercase().replace(['-', ' '], "_")),
        );
    }
    for detail in details {
        tokens.extend(
            detail
                .split(';')
                .map(|token| token.trim().to_ascii_lowercase().replace(['-', ' '], "_")),
        );
    }
    // Firearm presence dominates: the loudest LE wins (rifle over shotgun
    // over pistol); a purely quiet discipline stays silent.
    let mut best: Option<(ShootingSubtype, f64)> = None;
    for token in &tokens {
        if let Some(subtype) = firearm(token) {
            let le = match subtype {
                ShootingSubtype::Rifle => 139.0,
                ShootingSubtype::Shotgun => 134.8,
                ShootingSubtype::Pistol => 133.6,
                ShootingSubtype::Silent => continue,
            };
            if best.is_none_or(|(_, best)| le > best) {
                best = Some((subtype, le));
            }
        }
    }
    if let Some((subtype, _)) = best {
        return subtype;
    }
    if tokens.iter().any(|token| quiet(token)) {
        return ShootingSubtype::Silent;
    }
    // No tag evidence: the loudest discipline named wins (a "rifle and
    // pistol club" fires rifles); a purely quiet name stays silent; else the
    // conservative rifle default.
    let name = name.to_ascii_lowercase();
    if name.contains("rifle") {
        ShootingSubtype::Rifle
    } else if name.contains("clay")
        || name.contains("skeet")
        || name.contains("trap")
        || name.contains("shotgun")
    {
        ShootingSubtype::Shotgun
    } else if name.contains("pistol") || name.contains("handgun") || name.contains("ipsc") {
        ShootingSubtype::Pistol
    } else if name.contains("archery")
        || name.contains("crossbow")
        || name.contains("paintball")
        || name.contains("airsoft")
        || name.contains("laser")
    {
        ShootingSubtype::Silent
    } else {
        ShootingSubtype::Rifle
    }
}

/// Formula emission of one class-10/11 row — the single composition both
/// loaders call: the motorsport sub-type from the raw `sport` tag, the
/// shooting sub-type from `shooting` / `shooting:*` / name. `None` for
/// area-law classes and silent shooting sub-types.
pub fn formula_for_row(
    sport: u8,
    sport_tag: &str,
    shooting: Option<&str>,
    shooting_details: &[&str],
    name: &str,
) -> Option<FormulaEmission> {
    match sport {
        MOTORSPORT => motorsport_emission(motorsport_subtype(sport_tag)),
        SHOOTING => shooting_emission(shooting_subtype(shooting, shooting_details, name)),
        _ => None,
    }
}

/// Formula emission of a shooting sub-type: annual Lw = LE +
/// 10·lg(shots / 15.77 Ms), day-only — the single-shot energy LE spread over
/// the day period's 15,768,000 s/yr at the default 20,000 shots/yr.
/// [`ShootingSubtype::Silent`] has no emission (`None`).
pub fn shooting_emission(subtype: ShootingSubtype) -> Option<FormulaEmission> {
    let (le, spectrum) = match subtype {
        ShootingSubtype::Rifle => (139.0, SHOT_SPECTRUM_RIFLE),
        ShootingSubtype::Pistol => (133.6, SHOT_SPECTRUM_PISTOL),
        ShootingSubtype::Shotgun => (134.8, SHOT_SPECTRUM_SHOTGUN),
        ShootingSubtype::Silent => return None,
    };
    Some(FormulaEmission {
        lw_day: le
            + 10.0
                * (SHOOTING_DEFAULT_SHOTS_PER_YEAR / (DAY_PERIOD_HOURS_PER_YEAR * 3600.0)).log10(),
        spectrum,
        evening_offset: -50.0,
        night_offset: -50.0,
    })
}

/// Emission bands for a leisure area (day period), normalized so
/// `a_weighted_total(bands) == lw` (same contract as buildings).
pub fn leisure_emission_bands(profile: &LeisureProfile, lw: f64) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(lw, &profile.spectrum)
}

/// Emission bands for a formula class (day period), same normalization.
pub fn leisure_formula_bands(emission: &FormulaEmission) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(emission.lw_day, &emission.spectrum)
}

/// Leisure Lw — the shared [`crate::emission::settlement::area_lw`] over the
/// leisure polygon area (a bigger court / terrace is louder). Unified with
/// buildings; the polygon area replaces the old per-facility capacity scaling.
pub fn leisure_lw(profile: &LeisureProfile, area_m2: f64) -> f64 {
    crate::emission::settlement::area_lw(profile.lw_fixed, profile.lw_per_m2, area_m2)
        + profile.searching_traffic_db(area_m2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn radiated_dba_equals_lw_for_all_leisure_classes() {
        for s in [
            PITCH,
            PADEL,
            TENNIS,
            BASKETBALL,
            PLAYGROUND,
            POOL,
            OUTDOOR_SEATING,
            STADIUM,
            CAR_PARK,
            CAR_PARK_STREET,
        ] {
            let p = leisure_profile(s);
            let lw = leisure_lw(&p, 500.0);
            let aw = a_weighted_total(&leisure_emission_bands(&p, lw));
            assert!(
                (aw - lw).abs() < 1e-6,
                "sport {s}: radiated {aw:.6} != lw {lw:.6}"
            );
        }
        for subtype in [
            MotorsportSubtype::Circuit,
            MotorsportSubtype::Motocross,
            MotorsportSubtype::Kart,
            MotorsportSubtype::Speedway,
            MotorsportSubtype::Trial,
            MotorsportSubtype::Other,
        ] {
            let emission = motorsport_emission(subtype).unwrap();
            let aw = a_weighted_total(&leisure_formula_bands(&emission));
            assert!(
                (aw - emission.lw_day).abs() < 1e-6,
                "{subtype:?}: radiated {aw:.6} != lw {:.6}",
                emission.lw_day
            );
        }
        for subtype in [
            ShootingSubtype::Rifle,
            ShootingSubtype::Pistol,
            ShootingSubtype::Shotgun,
        ] {
            let emission = shooting_emission(subtype).unwrap();
            let aw = a_weighted_total(&leisure_formula_bands(&emission));
            assert!(
                (aw - emission.lw_day).abs() < 1e-6,
                "{subtype:?}: radiated {aw:.6} != lw {:.6}",
                emission.lw_day
            );
        }
    }

    /// The w7-sources evidence pilots, pinned: Most/Brands D100 (circuit 116,
    /// n=15, 600 h) → 119.1; Hodonín MX D100 (114, n≈7) → 113.6±0.2; Tatra
    /// rifle/clay and Hodonice IPSC N20k → 110.0/105.8/104.6.
    #[test]
    fn formula_subtypes_reproduce_the_evidence_pilots() {
        let motor = |s: MotorsportSubtype| motorsport_emission(s).unwrap().lw_day;
        let shot = |s: ShootingSubtype| shooting_emission(s).unwrap().lw_day;
        assert!((motor(MotorsportSubtype::Circuit) - 119.1).abs() < 0.05);
        assert!((motor(MotorsportSubtype::Motocross) - 113.6).abs() < 0.25);
        assert!((shot(ShootingSubtype::Rifle) - 110.0).abs() < 0.05);
        assert!((shot(ShootingSubtype::Shotgun) - 105.8).abs() < 0.05);
        assert!((shot(ShootingSubtype::Pistol) - 104.6).abs() < 0.05);
        // Day-only: evening and night effectively silent on every sub-type.
        for subtype in [
            MotorsportSubtype::Circuit,
            MotorsportSubtype::Motocross,
            MotorsportSubtype::Kart,
            MotorsportSubtype::Speedway,
            MotorsportSubtype::Trial,
            MotorsportSubtype::Other,
        ] {
            let emission = motorsport_emission(subtype).unwrap();
            assert_eq!(emission.evening_offset, -50.0, "{subtype:?} evening");
            assert_eq!(emission.night_offset, -50.0, "{subtype:?} night");
        }
        for subtype in [
            ShootingSubtype::Rifle,
            ShootingSubtype::Pistol,
            ShootingSubtype::Shotgun,
        ] {
            let emission = shooting_emission(subtype).unwrap();
            assert_eq!(emission.evening_offset, -50.0, "{subtype:?} evening");
            assert_eq!(emission.night_offset, -50.0, "{subtype:?} night");
        }
        assert!(shooting_emission(ShootingSubtype::Silent).is_none());
        assert!(motorsport_emission(MotorsportSubtype::Silent).is_none());
        // Only 10/11 are formula classes.
        assert!(is_formula_class(MOTORSPORT));
        assert!(is_formula_class(SHOOTING));
        assert!(!is_formula_class(PITCH));
        assert!(!is_formula_class(CAR_PARK_STREET));
        assert!(!is_formula_class(12));
    }

    #[test]
    fn motorsport_subtype_reads_raw_sport_tags() {
        use MotorsportSubtype::*;
        assert_eq!(motorsport_subtype("karting"), Kart);
        assert_eq!(motorsport_subtype("Kart"), Kart);
        assert_eq!(motorsport_subtype("go-kart"), Kart);
        assert_eq!(motorsport_subtype("motocross"), Motocross);
        assert_eq!(motorsport_subtype("supermoto"), Motocross);
        assert_eq!(motorsport_subtype("speedway"), Speedway);
        assert_eq!(motorsport_subtype("trial"), Trial);
        assert_eq!(motorsport_subtype("autocross"), Circuit);
        assert_eq!(motorsport_subtype("rallycross"), Circuit);
        assert_eq!(motorsport_subtype("car_racing"), Circuit);
        assert_eq!(motorsport_subtype("drag_racing"), Circuit);
        assert_eq!(motorsport_subtype("motor"), Other);
        assert_eq!(motorsport_subtype("motorsport"), Other);
        assert_eq!(motorsport_subtype(""), Other);
        assert_eq!(motorsport_subtype("chess"), Other);
        // Multi-values resolve to the loudest (circuit over kart).
        assert_eq!(motorsport_subtype("karting;car_racing"), Circuit);
        assert_eq!(motorsport_subtype("motocross;karting"), Kart);
        // Radio-controlled cars stay silent — unless a real vehicle shares
        // the value.
        assert_eq!(motorsport_subtype("rc_car"), Silent);
        assert_eq!(motorsport_subtype("RC-Car"), Silent);
        assert_eq!(motorsport_subtype("rc_car;karting"), Kart);
    }

    #[test]
    fn shooting_subtype_reads_tags_then_name() {
        use ShootingSubtype::*;
        assert_eq!(shooting_subtype(Some("rifle"), &[], ""), Rifle);
        assert_eq!(shooting_subtype(Some("pistol"), &[], ""), Pistol);
        assert_eq!(shooting_subtype(Some("clay_pigeon"), &[], ""), Shotgun);
        assert_eq!(shooting_subtype(Some("clay-pigeon"), &[], ""), Shotgun);
        assert_eq!(shooting_subtype(Some("skeet"), &[], ""), Shotgun);
        // Mixed tags: the loudest firearm wins; quiet-only stays silent.
        assert_eq!(shooting_subtype(Some("pistol;rifle"), &[], ""), Rifle);
        assert_eq!(shooting_subtype(Some("pistol;clay_pigeon"), &[], ""), Shotgun);
        assert_eq!(shooting_subtype(Some("archery"), &[], ""), Silent);
        assert_eq!(shooting_subtype(Some("paintball"), &[], ""), Silent);
        assert_eq!(shooting_subtype(Some("indoor_range"), &[], ""), Silent);
        assert_eq!(shooting_subtype(None, &["pistol"], ""), Pistol);
        // No tag evidence: the name decides, else the rifle default.
        assert_eq!(shooting_subtype(None, &[], "Tatra clay"), Shotgun);
        assert_eq!(shooting_subtype(None, &[], "Hodonice IPSC"), Pistol);
        assert_eq!(shooting_subtype(None, &[], "Rifle and pistol club"), Rifle);
        assert_eq!(shooting_subtype(None, &[], "City archery club"), Silent);
        assert_eq!(shooting_subtype(None, &[], "Střelnice"), Rifle);
        assert_eq!(shooting_subtype(None, &[], ""), Rifle);
    }

    /// The plan's loudness ordering must hold (each at its reference court):
    /// padel ≫ tennis ≫ basketball — padel is the 2024–26 complaint class.
    #[test]
    fn padel_louder_than_tennis_louder_than_basketball() {
        let anchor = |s: u8| {
            let p = leisure_profile(s);
            leisure_lw(&p, p.ref_area_m2)
        };
        assert!(anchor(PADEL) > anchor(TENNIS), "padel must beat tennis");
        assert!(
            anchor(TENNIS) > anchor(BASKETBALL),
            "tennis must beat basketball"
        );
    }

    /// Unified area scaling (replaces the old capacity build-up): a leisure
    /// source scales with its polygon AREA, +3 dB per doubling, and the anchor
    /// holds at the profile's reference footprint.
    #[test]
    fn leisure_scales_with_area() {
        let p = leisure_profile(OUTDOOR_SEATING); // node default ~12 m² ≈ 66 dB
        let small = leisure_lw(&p, p.ref_area_m2);
        let big = leisure_lw(&p, p.ref_area_m2 * 2.0);
        assert!(
            (small - 65.8).abs() < 0.2,
            "node-default terrace: {small:.1}"
        );
        assert!((big - small - 3.0103).abs() < 0.05, "doubling area = +3 dB");
    }

    /// The Parkplatzlärmstudie arithmetic, held in place: 63 dB(A) per space and
    /// movement per hour, 0.40 movements by day, the searching-traffic term
    /// `K_D = 2.5·lg(B − 9)` above ten spaces, over a space per 23.8 m² of lot
    /// (13.3 m² on a street-side strip).
    #[test]
    fn car_park_lw_follows_the_parking_study() {
        let study = |spaces: f64| {
            let searching = if spaces > 10.0 {
                2.5 * (spaces - 9.0).log10()
            } else {
                0.0
            };
            63.0 + searching + 10.0 * (spaces * 0.40).log10()
        };
        for (class, m2_per_space, area) in [
            (CAR_PARK, 23.8, 5_000.0),      // a supermarket lot: 210 spaces
            (CAR_PARK, 23.8, 1_000.0),      // 42 spaces
            (CAR_PARK_STREET, 13.3, 200.0), // a long strip: 15 spaces
            (CAR_PARK_STREET, 13.3, 120.0), // 9 spaces, under the K_D threshold
        ] {
            let profile = leisure_profile(class);
            let lw = leisure_lw(&profile, area);
            let expected = study(area / m2_per_space);
            assert!(
                (lw - expected).abs() < 0.1,
                "class {class} at {area} m²: {lw:.2} != {expected:.2}"
            );
        }
        // The engine's clock is not the study's: its 06–22 and 22–06 blocks are
        // re-averaged onto day 07–19, evening 19–23 and night 23–07.
        let profile = leisure_profile(CAR_PARK);
        let evening = 10.0 * ((3.0 * 0.40 + 0.05) / 4.0 / 0.40f64).log10();
        let night = 10.0 * ((0.40 + 7.0 * 0.05) / 8.0 / 0.40f64).log10();
        assert!(
            (profile.evening_offset - evening).abs() < 0.05,
            "evening {evening:.2}"
        );
        assert!(
            (profile.night_offset - night).abs() < 0.05,
            "night {night:.2}"
        );
        // Nothing else in the lane has spaces to search for.
        assert_eq!(leisure_profile(PITCH).m2_per_space, None);
    }

    /// Pitch duty from published use, held in place: the 58 dB LAeq,1h anchor
    /// back-calculates to 59.8 dB/m² active through the hemispherical area
    /// integral (−1.8 dB); grass takes 5 h/week over 40 weeks (4 day + 1
    /// evening), an AGP 40 h/week year-round on the 26-day/8-evening peak
    /// pattern. Night is silent on both (unlit / floodlights off ~22:00).
    #[test]
    fn pitch_duty_follows_published_use() {
        let active_per_m2 = 59.8;
        for (class, day_h, eve_h, weeks) in [
            (PITCH, 4.0f64, 1.0, 40.0),
            (AGP, 26.0 / 34.0 * 40.0, 8.0 / 34.0 * 40.0, 52.0),
        ] {
            let profile = leisure_profile(class);
            let day: f64 = 10.0 * (day_h * weeks / (12.0 * 365.0)).log10();
            let eve: f64 = 10.0 * (eve_h * weeks / (4.0 * 365.0)).log10();
            assert!(
                (profile.lw_per_m2 - (active_per_m2 + day)).abs() < 0.1,
                "class {class} day duty {day:.2}"
            );
            assert!(
                (profile.evening_offset - (eve - day)).abs() < 0.1,
                "class {class} evening offset {:.2}",
                eve - day
            );
            assert_eq!(profile.night_offset, -25.0);
            assert_eq!(profile.m2_per_space, None);
        }
        // The booked pitch runs ~10 dB hotter than grass over the year.
        let grass = leisure_profile(PITCH);
        let agp = leisure_profile(AGP);
        let lden = |p: &LeisureProfile| {
            let day = leisure_lw(p, 7_000.0);
            let eve = day + p.evening_offset + 5.0;
            let night = day + p.night_offset + 10.0;
            10.0 * ((12.0 * 10f64.powf(day / 10.0)
                + 4.0 * 10f64.powf(eve / 10.0)
                + 8.0 * 10f64.powf(night / 10.0))
                / 24.0)
                .log10()
        };
        assert!((lden(&grass) - 83.5).abs() < 0.2, "grass Lden");
        assert!((lden(&agp) - 93.8).abs() < 0.2, "AGP Lden");
    }

    /// A class id outside `leisure_v5` can only come from a file that lies about
    /// its stamp. It must say nothing rather than sound like a football pitch.
    #[test]
    fn an_unknown_class_emits_nothing() {
        let unknown = leisure_profile(AGP + 1);
        for area in [10.0, 1_000.0, 100_000.0] {
            assert!(
                leisure_lw(&unknown, area) < 10.0,
                "an unknown class must stay under the audibility gate at {area} m²"
            );
        }
        let pitch = leisure_profile(PITCH);
        assert!(
            leisure_lw(&pitch, 1_000.0) > 60.0,
            "an untyped pitch still emits"
        );
        let misrouted = leisure_profile(CAR_PARK_STREET + 1);
        assert!(
            leisure_lw(&misrouted, 100_000.0) < 10.0,
            "a formula class id passed here instead of its subtype path stays silent"
        );
        let agp = leisure_profile(AGP);
        assert!(
            leisure_lw(&agp, 1_000.0) > leisure_lw(&pitch, 1_000.0),
            "the booked pitch out-emits grass"
        );
    }

    #[test]
    fn sport_class_maps_known_values() {
        assert_eq!(sport_class("padel"), Some(PADEL));
        assert_eq!(sport_class("tennis"), Some(TENNIS));
        assert_eq!(sport_class("soccer"), Some(PITCH));
        assert_eq!(sport_class("basketball"), Some(BASKETBALL));
        assert_eq!(sport_class("swimming"), Some(POOL));
        // Motor and shooting sports bypass this map: the extractor owns
        // classes 10/11 and the readers subdivide them from the raw tags.
        assert_eq!(sport_class("motocross"), None);
        assert_eq!(sport_class("shooting"), None);
        assert_eq!(sport_class("chess"), None);
    }
}
