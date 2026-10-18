//! Open-air leisure areas and shooting ranges of dev4's `leisure.arrow` as area sources 1.5 m
//! above the ground, in the Building layer as dev4 shows them: the area law over the polygon (a
//! node takes its class's reference footprint; a line is no area and emits from its centroid),
//! ranges by shots a year unless roofed. Motorsport (PLAN-z13 DROP) and unknown classes are silent.
//! Parking serves the buildings around it: no more cars park and leave in a day in the lots
//! around than the trip ends of those buildings, nor in the street strips than the trip ends made
//! by cars that park on the street (houses' cars mostly park on their plots, flats' cars on the
//! street), each kind's shared by its spaces; the car-park rate of every space taken is the most a
//! space carries, and all a space carries where the square's buildings are not mapped (at that
//! rate a mountain hotel's 127-space lot took 865 movements a day, where the roads to it carry
//! 303 and 353 vehicles). Driving to and from a space is the road traffic, not the parking's. A
//! car park is heard in its movements, a range in its shots and every other area in use, in their
//! share of each period as bells are: heard steady, 69 movements a day on a Prague villa street's
//! strip read 1.96 sone, more than the flights over it (1.16) and four times the 65 cars a day
//! passing on the street (0.50, 2026-10-18).

use super::cells::{
    AUDIBILITY_FLOOR_DBA, Site, Z30Ring, push_site_points, resolve_area_m2, ring_cell, site_points,
};
use super::facilities::{Tags, parse_tags};
use super::venues::{Seating, Venues};
use super::{Converted, displayed_duty, fields, group_key};
use crate::dev4::{Dev4, Square, column, positive, require_stamp, text, z30_corner_degrees};
use crate::traffic::{GRID_SIDE, grid_cell, street_parking_shares};
use arrow_array::{BinaryArray, Float32Array, Int32Array, Int64Array, StringArray, UInt8Array};
use physics::bands::PERIOD_HOURS;
use physics::emission::leisure::*;
use physics::emission::spectrum::SoundPower;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// Voices and rackets on open ground.
const SOURCE_HEIGHT_M: f64 = 1.5;
/// Areas up to this emit from one point, larger ones from cells of this side (dev4).
const SINGLE_POINT_UP_TO_M2: f64 = 5_000.0;
const CELL_M: f64 = 75.0;
/// The area of a range with neither a stored area nor a ring.
const DEFAULT_RANGE_AREA_M2: f64 = 10_000.0;
/// dev4 `geometry_kind` of a line row (raceways and tracks).
const LINE_GEOMETRY: u8 = 2;
/// Street parking shares the trip ends of the cells within this many cells of its own (cells of
/// about 200 m: 3 x 3 of them) with the street parking spaces there.
const STREET_PARKING_REACH_CELLS: usize = 1;

/// One `leisure.arrow` row as the conversion reads it; tags are kept for shooting ranges only.
pub struct LeisureRow {
    pub osm_kind: String,
    pub osm_id: i64,
    pub centroid: (i32, i32),
    pub class: u8,
    pub name: String,
    pub ring: Z30Ring,
    pub area_m2: Option<f64>,
    pub is_line: bool,
    pub tags: Tags,
}

impl LeisureRow {
    /// The polygon the row emits over: none for a line.
    fn area_ring(&self) -> &[(i32, i32)] {
        if self.is_line { &[] } else { &self.ring }
    }
}

/// A row's sound power and area; `None` when silent.
pub fn row_emission(row: &LeisureRow) -> Option<(SoundPower, f64)> {
    let (sound, area_m2) = if row.class == SHOOTING {
        let roofed = |key: &str| row.tags.get(key).is_some_and(|value| value != "no");
        if roofed("building") || roofed("indoor") {
            return None;
        }
        let details: Vec<&str> = row
            .tags
            .iter()
            .filter(|(key, _)| key.starts_with("shooting:"))
            .map(|(_, value)| value.as_str())
            .collect();
        let shooting = row.tags.get("shooting").map(String::as_str);
        let sound = shooting_sound_power(shooting_subtype(shooting, &details, &row.name))?;
        (
            sound,
            resolve_area_m2(row.area_m2, row.area_ring(), DEFAULT_RANGE_AREA_M2),
        )
    } else {
        let reference_m2 = leisure_profile(row.class)?.reference_area_m2;
        let area_m2 = resolve_area_m2(row.area_m2, row.area_ring(), reference_m2);
        (leisure_sound_power(row.class, area_m2)?, area_m2)
    };
    (sound.day_dba >= AUDIBILITY_FLOOR_DBA).then_some((sound, area_m2))
}

fn class_label(class: u8) -> &'static str {
    match class {
        PITCH => "sports_pitch",
        PADEL => "padel_court",
        TENNIS => "tennis_court",
        BASKETBALL => "ball_court",
        PLAYGROUND => "playground",
        POOL => "swimming_pool",
        OUTDOOR_SEATING => "outdoor_seating",
        STADIUM => "stadium",
        CAR_PARK => "car_park",
        CAR_PARK_STREET => "street_parking",
        SHOOTING => "shooting",
        ARTIFICIAL_TURF_PITCH => "artificial_turf_pitch",
        _ => "unknown",
    }
}

/// A mapped terrace this near a venue that seats guests outside is its terrace (m; `fetch/venues.py`
/// seats such a venue outside when untagged).
const TERRACE_OF_VENUE_M: f64 = 30.0;

/// Whether a mapped terrace at `place` belongs to a venue, whose guests the people converter seats
/// with its hours (the review of 2026-10-05, Codex B 11: both emitted, +1.1 dB); beside a venue
/// tagged with no outdoor seating it stays its own crowd.
fn terrace_of_a_venue(venues: &Venues, square: Square, (lat, lon): (f64, f64)) -> bool {
    let metres_per_degree = 111_195.0;
    square.with_neighbours().into_iter().any(|near| {
        venues.in_square(near.x, near.y).iter().any(|venue| {
            if venue.seating == Seating::No {
                return false;
            }
            let dy = (venue.lat - lat) * metres_per_degree;
            let dx = (venue.lon - lon) * metres_per_degree * lat.to_radians().cos();
            dx.hypot(dy) <= TERRACE_OF_VENUE_M
        })
    })
}

/// A car park's spaces from its area.
fn parking_spaces(row: &LeisureRow, area_m2: f64) -> f64 {
    leisure_profile(row.class)
        .and_then(|profile| profile.m2_per_parking_space)
        .map_or(0.0, |per_space| area_m2 / per_space)
}

/// The parking spaces of one kind in a square (street strips or lots) against the trip ends of
/// its buildings that may park in them, both per grid cell.
struct ParkingBudget {
    trip_ends: Vec<f64>,
    spaces: Vec<f64>,
}

impl ParkingBudget {
    fn new(trip_ends: Vec<f64>) -> Self {
        let spaces = vec![0.0; trip_ends.len()];
        ParkingBudget { trip_ends, spaces }
    }

    fn around(values: impl Fn(usize) -> f64, (row, column): (usize, usize)) -> f64 {
        let reach = STREET_PARKING_REACH_CELLS;
        let mut sum = 0.0;
        for r in row.saturating_sub(reach)..=(row + reach).min(GRID_SIDE - 1) {
            for c in column.saturating_sub(reach)..=(column + reach).min(GRID_SIDE - 1) {
                sum += values(r * GRID_SIDE + c);
            }
        }
        sum
    }

    /// The share of the car-park rate at `cell`: each nearby origin divides its trips among
    /// all the spaces within its reach, and a space carries at most the car-park rate.
    fn share(&self, cell: (usize, usize)) -> f64 {
        let movements_per_space = Self::around(
            |k| {
                // Each origin cell divides its budget among all spaces it can serve.
                let trips = self.trip_ends[k];
                if trips == 0.0 {
                    return 0.0;
                }
                // The origin's window holds this cell, so its spaces count among these.
                trips / Self::around(|j| self.spaces[j], (k / GRID_SIDE, k % GRID_SIDE))
            },
            cell,
        );
        (movements_per_space / PARKING_MOVEMENTS_PER_SPACE_DAY).min(1.0)
    }
}

/// Converts the leisure rows of one dev4 square, its car parks bound by the trip ends of the
/// square's buildings under `traffic` when given (its buildings mapped); returns how many rows
/// emit.
pub fn convert(
    dev4: &Dev4,
    square: Square,
    (traffic, venues): (Option<&std::path::Path>, Option<&Venues>),
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "leisure.arrow")? else {
        return Ok(0);
    };
    let context = |error: String| format!("leisure.arrow of {square:?}: {error}");
    for (key, value) in [("grid", "z30"), ("leisure_contract", "leisure_v5")] {
        require_stamp(&table, key, value).map_err(context)?;
    }
    let mut rows = Vec::new();
    for batch in &table.batches {
        rows.extend(read_batch(batch).map_err(context)?);
    }
    // A street strip takes the trip ends made by cars that park on the street, a lot any of them.
    let parking = rows
        .iter()
        .any(|row| matches!(row.class, CAR_PARK | CAR_PARK_STREET));
    let budgets = match traffic.filter(|_| parking) {
        Some(dir) => crate::traffic::read_grid(dir, square)?.map(|grid| -> Result<_, String> {
            let street_shares = street_parking_shares(dev4, square)?;
            let ends = |k: usize| f64::from(grid[k]);
            let cells = 0..grid.len();
            let mut street = ParkingBudget::new(
                cells
                    .clone()
                    .map(|k| ends(k) * f64::from(street_shares[k]))
                    .collect(),
            );
            let mut lots = ParkingBudget::new(cells.map(ends).collect());
            for row in &rows {
                let budget = match row.class {
                    CAR_PARK_STREET => &mut street,
                    CAR_PARK => &mut lots,
                    _ => continue,
                };
                if let Some((_, area_m2)) = row_emission(row) {
                    let (r, c) = grid_cell(square, row.centroid);
                    budget.spaces[r * GRID_SIDE + c] += parking_spaces(row, area_m2);
                }
            }
            Ok((street, lots))
        }),
        None => None,
    }
    .transpose()?;
    let mut emitting = 0;
    {
        for row in rows {
            let Some((mut sound, area_m2)) = row_emission(&row) else {
                continue;
            };
            if row.class == OUTDOOR_SEATING
                && venues.is_some_and(|venues| {
                    let place = z30_corner_degrees(row.centroid.0, row.centroid.1);
                    terrace_of_a_venue(venues, square, place)
                })
            {
                continue;
            }
            let spaces = parking_spaces(&row, area_m2);
            let mut movements = spaces * PARKING_MOVEMENTS_PER_SPACE_DAY;
            let budget = budgets.as_ref().and_then(|(street, lots)| match row.class {
                CAR_PARK_STREET => Some(street),
                CAR_PARK => Some(lots),
                _ => None,
            });
            if let Some(budget) = budget {
                let share = budget.share(grid_cell(square, row.centroid));
                if share <= 0.0 {
                    continue;
                }
                sound.day_dba += 10.0 * share.log10();
                movements *= share;
                if sound.day_dba < AUDIBILITY_FLOOR_DBA {
                    continue;
                }
            }
            let site = Site {
                centroid: z30_corner_degrees(row.centroid.0, row.centroid.1),
                ring: row.area_ring(),
                area_m2,
                single_point_up_to_m2: SINGLE_POINT_UP_TO_M2,
                cell_m: CELL_M,
            };
            let decibels = (sound.day_dba * 10.0).round() / 10.0;
            let heard = heard_of(row.class, &sound, movements);
            let attribute = Attribute {
                layer: Layer::Building,
                height_m: SOURCE_HEIGHT_M,
                ground_percent: GROUND_FROM_TERRAIN,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: group_key(&["leisure", &row.osm_kind, &row.osm_id.to_string()]),
                emission: sound.band_levels_db(),
                display: fields(json!([
                    row.name,
                    class_label(row.class),
                    0.0,
                    0,
                    area_m2.round(),
                    "",
                    decibels,
                    (spaces > 0.0).then_some(movements),
                    heard.0,
                    heard.1
                ])),
            };
            push_site_points(&site_points(&site), area_m2, &attribute, out);
            emitting += 1;
        }
    }
    Ok(emitting)
}

/// How a row is heard in time: its events a day by period where it has events, and the share of
/// each period it sounds. A range in its shots, a second each at its single-shot energy level by
/// day (the level's reference second: a real shot is shorter, so a range reads somewhat loud); a
/// car park in its `movements` at a movement's power; any other area in use at its anchor.
fn heard_of(class: u8, sound: &SoundPower, movements: f64) -> (Option<[f64; 3]>, [f64; 3]) {
    let events = |per_day: f64, while_sounding_dba: f64| {
        let (events, duty) = heard_in_events(sound, per_day, while_sounding_dba);
        (Some(events), duty)
    };
    if class == SHOOTING {
        let per_day = SHOTS_PER_YEAR / 365.0;
        return events(
            per_day,
            sound.day_dba + 10.0 * (PERIOD_HOURS[0] * 3_600.0 / per_day).log10(),
        );
    }
    if movements > 0.0 {
        return events(movements, PARKING_MOVEMENT_DBA);
    }
    let in_use_db = leisure_profile(class).map_or(0.0, |profile| profile.in_use_db);
    (
        None,
        displayed_duty(sounding(sound, sound.day_dba + in_use_db)),
    )
}

/// The share of each period a source sounds at `while_sounding_dba`: its mean over that power.
fn sounding(sound: &SoundPower, while_sounding_dba: f64) -> [f64; 3] {
    let offsets = [0.0, sound.evening_offset_db, sound.night_offset_db];
    std::array::from_fn(|p| 10f64.powf((sound.day_dba + offsets[p] - while_sounding_dba) / 10.0))
}

/// A source of `per_day` events, each sounding at `while_sounding_dba`: its events by period as the
/// seconds they sound fall, and the share of each period they sound (rounded up, which under 0.1
/// events a day stretches them).
fn heard_in_events(
    sound: &SoundPower,
    per_day: f64,
    while_sounding_dba: f64,
) -> ([f64; 3], [f64; 3]) {
    let on = sounding(sound, while_sounding_dba);
    let hours: f64 = (0..3).map(|p| on[p] * PERIOD_HOURS[p]).sum();
    (
        std::array::from_fn(|p| per_day * on[p] * PERIOD_HOURS[p] / hours),
        displayed_duty(on),
    )
}

fn read_batch(batch: &arrow_array::RecordBatch) -> Result<Vec<LeisureRow>, String> {
    let integers = |name: &str| column::<Int32Array>(batch, name);
    let (gx, gy) = (integers("centroid_gx")?, integers("centroid_gy")?);
    let texts = |name: &str| column::<StringArray>(batch, name);
    let (kinds, names, tags) = (texts("osm_kind")?, texts("name")?, texts("osm_tags")?);
    let bytes = |name: &str| column::<UInt8Array>(batch, name);
    let (classes, geometry_kinds) = (bytes("sport")?, bytes("geometry_kind")?);
    let (ids, geometry) = (
        column::<Int64Array>(batch, "osm_id")?,
        column::<BinaryArray>(batch, "geom")?,
    );
    let areas = column::<Float32Array>(batch, "area_m2")?;
    Ok((0..batch.num_rows())
        .map(|row| LeisureRow {
            osm_kind: kinds.value(row).to_string(),
            osm_id: ids.value(row),
            centroid: (gx.value(row), gy.value(row)),
            class: classes.value(row),
            name: text(names, row).to_string(),
            ring: ring_cell(geometry, row),
            area_m2: positive(areas, row),
            is_line: geometry_kinds.value(row) == LINE_GEOMETRY,
            tags: match classes.value(row) {
                SHOOTING => parse_tags(tags.value(row)),
                _ => Tags::new(),
            },
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: u8) -> LeisureRow {
        LeisureRow {
            osm_kind: "node".into(),
            osm_id: 1,
            centroid: (1 << 29, 1 << 29),
            class,
            name: String::new(),
            ring: Vec::new(),
            area_m2: None,
            is_line: false,
            tags: Tags::new(),
        }
    }

    /// A padel node is the reference court: active 90 minus 9 dB of season and duty, 81 dB(A).
    #[test]
    fn a_node_is_its_class_reference_footprint_and_a_line_is_no_area() {
        let (sound, area_m2) = row_emission(&node(PADEL)).unwrap();
        assert_eq!(area_m2, 200.0);
        assert!((sound.day_dba - 81.0).abs() < 0.2);
        let (x, y) = (1 << 29, 1 << 29);
        let chain = vec![(x, y), (x + 20_000, y + 20_000), (x, y + 30_000)];
        let line = LeisureRow {
            is_line: true,
            ring: chain,
            ..node(PITCH)
        };
        assert_eq!(row_emission(&line), row_emission(&node(PITCH)));
    }

    #[test]
    fn motorsport_unknown_and_roofed_ranges_are_silent() {
        assert_eq!(row_emission(&node(MOTORSPORT)), None);
        assert_eq!(row_emission(&node(ARTIFICIAL_TURF_PITCH + 1)), None);
        let roofed = LeisureRow {
            tags: parse_tags(r#"{"indoor":"yes"}"#),
            ..node(SHOOTING)
        };
        assert_eq!(row_emission(&roofed), None);
        let archery = LeisureRow {
            tags: parse_tags(r#"{"shooting":"archery"}"#),
            ..node(SHOOTING)
        };
        assert_eq!(row_emission(&archery), None);
        let (range, area_m2) = row_emission(&node(SHOOTING)).unwrap();
        assert!((range.day_dba - 110.0).abs() < 0.05 && area_m2 == 10_000.0);
    }

    /// A Prague villa street's strip (69.1 dB(A) by day, 69 movements a day) is heard in its
    /// movements, 4 s each at a movement's power in every period; a rifle range in its 55 shots a
    /// day, a second each by day; a grass pitch in use, 4 h of the week's 84 by day for 40 weeks,
    /// and a court an eighth of the day, their uses uncounted.
    #[test]
    fn parking_movements_shots_and_play_are_heard_as_events() {
        let mut strip = leisure_sound_power(CAR_PARK_STREET, 411.0).unwrap();
        strip.day_dba = 69.1;
        let (movements, duty) = heard_of(CAR_PARK_STREET, &strip, 69.0);
        let movements = movements.unwrap();
        assert!((movements.iter().sum::<f64>() - 69.0).abs() < 0.02);
        for period in 0..3 {
            let seconds = duty[period] * PERIOD_HOURS[period] * 3_600.0 / movements[period];
            assert!((seconds - 4.0).abs() < 0.1, "period {period}: {seconds} s");
        }
        let (range, _) = row_emission(&node(SHOOTING)).unwrap();
        let (shots, duty) = heard_of(SHOOTING, &range, 0.0);
        let shots = shots.unwrap();
        assert!((shots[0] - 20_000.0 / 365.0).abs() < 0.01 && shots[1] + shots[2] < 0.001);
        assert!((duty[0] * PERIOD_HOURS[0] * 3_600.0 / shots[0] - 1.0).abs() < 0.01);
        let (pitch, _) = row_emission(&node(PITCH)).unwrap();
        let (uses, duty) = heard_of(PITCH, &pitch, 0.0);
        let played = 4.0 * 40.0 / (84.0 * 52.0);
        assert!(
            uses.is_none() && (duty[0] - played).abs() < 0.001,
            "{duty:?}"
        );
        assert!(duty[2] < 0.001);
        let (court, _) = row_emission(&node(TENNIS)).unwrap();
        let (_, duty) = heard_of(TENNIS, &court, 0.0);
        assert!((duty[0] - 0.126).abs() < 0.001, "{duty:?}");
    }

    /// A lot carries no more movements than the trip ends around it: 100 a day shared by 50
    /// spaces are 2 a space, 0.29 of the car-park rate; a lot nobody's trips end near is silent.
    #[test]
    fn a_lot_carries_the_trip_ends_around_it() {
        let cell = |row: usize, column: usize| row * GRID_SIDE + column;
        let mut trip_ends = vec![0.0; GRID_SIDE * GRID_SIDE];
        trip_ends[cell(10, 10)] = 100.0;
        let mut lots = ParkingBudget::new(trip_ends);
        lots.spaces[cell(10, 11)] = 50.0;
        lots.spaces[cell(40, 40)] = 50.0;
        let share = lots.share((10, 11));
        assert!(
            (share - 2.0 / PARKING_MOVEMENTS_PER_SPACE_DAY).abs() < 1e-9,
            "{share}"
        );
        assert_eq!(lots.share((40, 40)), 0.0);
    }

    /// A mapped terrace 10 m from a cafe is the cafe's (its guests are the people converter's); one
    /// 100 m away, or beside a cafe tagged with no outdoor seating, stands alone.
    #[test]
    fn a_terrace_beside_a_venue_is_the_venues() {
        let venues =
            Venues::parse("50\t14\tcafe\tyes\t0\t\t\n50\t14.01\tcafe\tno\t0\t\t\n").unwrap();
        let (gx, gy) = crate::dev4::degrees_to_z30(50.0, 14.0);
        let square = Square::of_z30(gx, gy);
        let north = |metres: f64| (50.0 + metres / 111_195.0, 14.0);
        assert!(terrace_of_a_venue(&venues, square, north(10.0)));
        assert!(!terrace_of_a_venue(&venues, square, north(100.0)));
        // Beside a cafe that seats nobody outside the terrace is its own crowd.
        assert!(!terrace_of_a_venue(
            &venues,
            square,
            (50.0 + 10.0 / 111_195.0, 14.01)
        ));
    }
}
