//! Bridge decks. A road or railway on a bridge emits from its deck, not from the valley the bare
//! earth puts under it. The rows on a bridge (not in a tunnel) of a square and its neighbours, a
//! stretch repeated by several rows counted once, join at their shared ends. A deck is a single
//! path whose two free ends meet the ground (a row of the same layer off the bridge ends there: an
//! approach, or a tunnel's portal) on the release's terrain, the one the popup reads, so the deck
//! meets its approaches; it runs straight between them by distance along it. A branched or closed
//! group, or one with an end that meets no approach (a gap in the data, or a viaduct cut by the
//! neighbourhood's edge), stays on the ground and is counted. The release's terrain tiles must be
//! built first.

use super::metres;
use crate::dev4::{Dev4, Square, z30_corner_degrees, z30_to_global};
use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tiles::geo::TileId;
use tiles::terrain::{OCEAN, Terrain};
use tiles::{Kind, tile_path};

/// A dev4 row's ends in the z30 grid (start x, start y, end x, end y): the row's identity in every
/// square that converts it.
pub type RowEnds = [i32; 4];

type Place = (i32, i32);

/// The deck's altitude (m) at the start and end of every bridge row of a square and its
/// neighbours that stands on one, roads and railways apart; and the groups of the square's own.
#[derive(Default)]
pub struct Decks {
    pub roads: HashMap<RowEnds, [f64; 2]>,
    pub railways: HashMap<RowEnds, [f64; 2]>,
    pub counts: DeckCounts,
}

/// Groups of bridge rows owned by a square (by their first stretch's middle): decks and their
/// rows, and those left on the ground, by why.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckCounts {
    pub decks: usize,
    pub rows: usize,
    pub branched_or_closed: usize,
    pub without_approach: usize,
    pub without_terrain: usize,
}

impl std::ops::AddAssign for DeckCounts {
    fn add_assign(&mut self, other: DeckCounts) {
        self.decks += other.decks;
        self.rows += other.rows;
        self.branched_or_closed += other.branched_or_closed;
        self.without_approach += other.without_approach;
        self.without_terrain += other.without_terrain;
    }
}

impl Decks {
    /// The decks of `square`'s rows and its neighbours' (a deck crossing the square's edge finds
    /// both abutments), their ends on the terrain of the release under `year_root`.
    pub fn of_square(dev4: &Dev4, square: Square, year_root: &Path) -> Result<Decks, String> {
        let squares = square.with_neighbours();
        let terrain: RefCell<HashMap<TileId, Option<Terrain>>> = RefCell::new(HashMap::new());
        let failure = RefCell::new(None);
        let ground = |(gx, gy): Place| {
            let place = z30_to_global(gx, gy).to_mercator();
            let tile = TileId::containing(place);
            let mut tiles = terrain.borrow_mut();
            let read = match tiles.entry(tile) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let path = tile_path(year_root, tile, Kind::Terrain);
                    let parsed = match std::fs::read(&path) {
                        Ok(bytes) => Terrain::parse(&bytes)
                            .map(Some)
                            .map_err(|error| format!("{}: {error:?}", path.display())),
                        // A release without the tile has the open sea there.
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(error) => Err(format!("{}: {error}", path.display())),
                    };
                    match parsed {
                        Ok(parsed) => entry.insert(parsed),
                        Err(error) => {
                            failure.borrow_mut().get_or_insert(error);
                            return None;
                        }
                    }
                }
            };
            match read {
                Some(terrain) => terrain.sample(place).map(|sample| sample.height_m),
                None => Some(OCEAN.height_m),
            }
        };
        let owned = |row: &RowEnds| {
            let middle = |a: i32, b: i32| ((i64::from(a) + i64::from(b)) / 2) as i32;
            Square::of_z30(middle(row[0], row[2]), middle(row[1], row[3])) == square
        };
        let mut decks = Decks::default();
        for (name, altitudes) in [
            ("roads.arrow", &mut decks.roads),
            ("railways.arrow", &mut decks.railways),
        ] {
            let (mut bridges, mut approaches) = (Vec::new(), HashSet::new());
            for &each in &squares {
                read_rows(dev4, each, name, &mut bridges, &mut approaches)?;
            }
            let (found, counts) = deck_altitudes(&bridges, &approaches, &ground, &owned);
            *altitudes = found;
            decks.counts += counts;
        }
        match failure.into_inner() {
            Some(error) => Err(error),
            None => Ok(decks),
        }
    }
}

/// Appends a square's rows on a bridge and not in a tunnel to `bridges`, and every other row's
/// ends to `approaches`.
fn read_rows(
    dev4: &Dev4,
    square: Square,
    name: &str,
    bridges: &mut Vec<RowEnds>,
    approaches: &mut HashSet<Place>,
) -> Result<(), String> {
    let Some(table) = dev4.table(square, name)? else {
        return Ok(());
    };
    for batch in &table.batches {
        let column = |column: &str| {
            batch
                .column_by_name(column)
                .ok_or_else(|| format!("{name}: no column {column}"))
        };
        let ends = [
            column("start_gx")?.as_primitive::<Int32Type>(),
            column("start_gy")?.as_primitive::<Int32Type>(),
            column("end_gx")?.as_primitive::<Int32Type>(),
            column("end_gy")?.as_primitive::<Int32Type>(),
        ];
        let (bridge, tunnel) = (
            column("bridge")?.as_boolean(),
            column("tunnel")?.as_boolean(),
        );
        for row in 0..batch.num_rows() {
            let row_ends = ends.map(|end| end.value(row));
            if row_ends[..2] == row_ends[2..] {
                continue;
            }
            if bridge.value(row) && !tunnel.value(row) {
                bridges.push(row_ends);
            } else {
                approaches.insert((row_ends[0], row_ends[1]));
                approaches.insert((row_ends[2], row_ends[3]));
            }
        }
    }
    Ok(())
}

/// A stretch's two ends in a fixed order, whichever way its rows run.
fn stretch(row: &RowEnds) -> (Place, Place) {
    let (a, b) = ((row[0], row[1]), (row[2], row[3]));
    if a <= b { (a, b) } else { (b, a) }
}

fn length_m(a: Place, b: Place) -> f64 {
    metres(z30_corner_degrees(a.0, a.1), z30_corner_degrees(b.0, b.1))
}

/// Each bridge row's deck altitude at its start and end, for the rows of decks (see the module);
/// the groups whose first stretch `owned` takes, counted.
pub fn deck_altitudes(
    rows: &[RowEnds],
    approaches: &HashSet<Place>,
    ground: &dyn Fn(Place) -> Option<f64>,
    owned: &dyn Fn(&RowEnds) -> bool,
) -> (HashMap<RowEnds, [f64; 2]>, DeckCounts) {
    let mut stretches: Vec<(Place, Place)> = rows.iter().map(stretch).collect();
    stretches.sort_unstable();
    stretches.dedup();
    let mut at_place: HashMap<Place, Vec<usize>> = HashMap::new();
    for (index, &(a, b)) in stretches.iter().enumerate() {
        at_place.entry(a).or_default().push(index);
        at_place.entry(b).or_default().push(index);
    }
    let ends_of = |(a, b): (Place, Place)| [a.0, a.1, b.0, b.1];
    let mut altitude: HashMap<Place, f64> = HashMap::new();
    let mut seen = vec![false; stretches.len()];
    let mut counts = DeckCounts::default();
    for first in 0..stretches.len() {
        if seen[first] {
            continue;
        }
        // The group: every stretch reachable through shared ends, the first the lowest.
        let (mut places, mut stack) = (HashSet::new(), vec![first]);
        seen[first] = true;
        while let Some(index) = stack.pop() {
            let (a, b) = stretches[index];
            places.extend([a, b]);
            for &next in at_place[&a].iter().chain(&at_place[&b]) {
                if !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        let mut free: Vec<Place> = places
            .iter()
            .copied()
            .filter(|place| at_place[place].len() == 1)
            .collect();
        free.sort_unstable();
        let path = free.len() == 2 && places.iter().all(|place| at_place[place].len() <= 2);
        let met = free.iter().all(|end| approaches.contains(end));
        let heights: Option<Vec<f64>> = free.iter().map(|&end| ground(end)).collect();
        let counted = match (path, met, heights) {
            (false, _, _) => &mut counts.branched_or_closed,
            (true, false, _) => &mut counts.without_approach,
            (true, true, None) => &mut counts.without_terrain,
            (true, true, Some(heights)) => {
                // Walk the path from one free end; the deck runs straight to the other.
                let mut along = vec![(free[0], 0.0)];
                let (mut place, mut came, mut walked) = (free[0], usize::MAX, 0.0);
                while place != free[1] {
                    let next = at_place[&place]
                        .iter()
                        .copied()
                        .find(|&s| s != came)
                        .unwrap();
                    let (a, b) = stretches[next];
                    let other = if a == place { b } else { a };
                    walked += length_m(place, other);
                    along.push((other, walked));
                    (place, came) = (other, next);
                }
                for (place, distance) in along {
                    let fraction = if walked > 0.0 { distance / walked } else { 0.0 };
                    altitude.insert(place, heights[0] + fraction * (heights[1] - heights[0]));
                }
                &mut counts.decks
            }
        };
        if owned(&ends_of(stretches[first])) {
            *counted += 1;
        }
    }
    let mut decks = HashMap::new();
    for row in rows {
        let (start, end) = ((row[0], row[1]), (row[2], row[3]));
        if let (Some(&from), Some(&to)) = (altitude.get(&start), altitude.get(&end)) {
            decks.insert(*row, [from, to]);
            if owned(&ends_of(stretch(row))) {
                counts.rows += 1;
            }
        }
    }
    (decks, counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// z30 rows on the equator, 10,000 cells (373 m) a step.
    const EQUATOR: i32 = 1 << 29;
    const STEP: i32 = 10_000;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    fn row(from: i32, to: i32) -> RowEnds {
        [from * STEP, EQUATOR, to * STEP, EQUATOR]
    }

    /// Approaches at every listed step.
    fn approaches(steps: &[i32]) -> HashSet<Place> {
        steps.iter().map(|&step| (step * STEP, EQUATOR)).collect()
    }

    /// A valley 50 m deep under a deck of three rows, its abutments at 300 m and 330 m, the middle
    /// row repeated by a second row the other way round: the deck runs straight between them.
    #[test]
    fn a_span_runs_straight_between_its_abutments() {
        let ground = |(x, _): Place| {
            Some(if x <= STEP {
                300.0
            } else if x >= 4 * STEP {
                330.0
            } else {
                250.0
            })
        };
        let rows = [row(1, 2), row(2, 3), row(3, 2), row(3, 4)];
        let (decks, counts) = deck_altitudes(&rows, &approaches(&[1, 4]), &ground, &|_| true);
        assert!(close(decks[&rows[0]][0], 300.0) && close(decks[&rows[3]][1], 330.0));
        assert!(close(decks[&rows[0]][1], 310.0), "{:?}", decks[&rows[0]]);
        assert!(close(decks[&rows[1]][1], 320.0) && close(decks[&rows[2]][0], 320.0));
        assert_eq!((counts.decks, counts.rows), (1, 4));
    }

    /// A Y of three spans, a span whose end meets no approach (cut, or a gap in the data) and one
    /// over the sea with no terrain at its ends: all stay on the ground, each counted.
    #[test]
    fn groups_that_are_not_two_ended_paths_stay_on_the_ground() {
        let ground = |(x, _): Place| (x < 10 * STEP).then_some(300.0);
        let spur = [2 * STEP, EQUATOR, 2 * STEP, EQUATOR + STEP];
        let rows = [row(0, 2), row(2, 4), spur, row(6, 7), row(20, 21)];
        let (decks, counts) =
            deck_altitudes(&rows, &approaches(&[0, 4, 6, 20, 21]), &ground, &|_| true);
        assert!(decks.is_empty(), "{decks:?}");
        assert_eq!(
            counts,
            DeckCounts {
                branched_or_closed: 1,
                without_approach: 1,
                without_terrain: 1,
                ..DeckCounts::default()
            }
        );
    }
}
