//! The ray's profile: a vertex on every lattice line it crosses, each on the bilinear surface, a
//! crest one node wide never stepped over, an ocean tile at sea level.

use super::*;
use tiles::terrain::{Window, encode};

/// Three by three tiles around Prague's centre, every node's height and imperviousness from
/// its global lattice row and column; `ocean` leaves the eastern column of tiles absent.
struct Fixture {
    tiles: Vec<(TileId, Option<Vec<u8>>)>,
    frame: LocalFrame,
    centre: TileId,
}

fn fixture(height: impl Fn(i64, i64) -> f64, ocean: bool) -> Fixture {
    let centre = TileId::containing(Mercator::from_degrees(50.08, 14.42));
    let mut tiles = Vec::new();
    for dy in -1..=1_i32 {
        for dx in -1..=1_i32 {
            let tile = TileId {
                x: centre.x.wrapping_add_signed(dx),
                y: centre.y.wrapping_add_signed(dy),
            };
            if ocean && dx == 1 {
                tiles.push((tile, None));
                continue;
            }
            let window = Window::of_tile(tile);
            let nodes: Vec<Node> = (0..window.rows)
                .flat_map(|r| (0..window.columns).map(move |c| (r, c)))
                .map(|(r, c)| {
                    let (row, column) = (
                        i64::from(window.north_node) - i64::from(r),
                        i64::from(window.west_node) + i64::from(c),
                    );
                    Node {
                        height_code: ((height(row, column) + 500.0) * 5.0).round() as u16,
                        impervious_percent: (row * 7 + column * 3).rem_euclid(101) as u8,
                        forest_percent: 0,
                    }
                })
                .collect();
            tiles.push((tile, Some(encode(window, &nodes))));
        }
    }
    Fixture {
        tiles,
        frame: LocalFrame::at(centre.centre()),
        centre,
    }
}

impl Fixture {
    fn with_ground<T>(&self, f: impl FnOnce(&Ground) -> T) -> T {
        let mut ground = Ground::new(self.frame, self.centre, 1);
        for (tile, bytes) in &self.tiles {
            ground.insert(*tile, bytes.as_deref().map(|b| Terrain::parse(b).unwrap()));
        }
        f(&ground)
    }
}

/// Rays inside the clicked tile and across its edges, every way round.
const RAYS: [([f64; 2], [f64; 2]); 6] = [
    ([0.0, 0.0], [1_000.0, 0.0]),
    ([0.0, 0.0], [0.0, -1_500.0]),
    ([-2_500.0, 1_900.0], [3_700.0, -2_800.0]),
    ([3_100.0, 40.0], [-3_300.0, 77.0]),
    ([12.3, 45.6], [-31.4, -15.9]),
    ([-1_000.0, -3_000.0], [2_000.0, 3_500.0]),
];

fn rugged(row: i64, column: i64) -> f64 {
    200.0 + ((row * 7_919 + column * 104_729).rem_euclid(97)) as f64 * 0.6
}

#[test]
fn every_vertex_lies_on_the_bilinear_surface_and_on_a_lattice_line() {
    let fixture = fixture(rugged, false);
    fixture.with_ground(|ground| {
        let mut profile = Profile::default();
        for (source, receiver) in RAYS {
            ground.fill_profile(source, receiver, &mut profile).unwrap();
            assert_eq!((profile.t[0], *profile.t.last().unwrap()), (0.0, 1.0));
            assert!(profile.t.windows(2).all(|pair| pair[0] < pair[1]));
            for (index, &t) in profile.t.iter().enumerate() {
                let point = [
                    source[0] + t * (receiver[0] - source[0]),
                    source[1] + t * (receiver[1] - source[1]),
                ];
                let expected = ground.at(point).unwrap();
                assert!(
                    (profile.ground_m[index] - expected.height_m).abs() < 1e-6,
                    "{t}"
                );
                assert!((profile.ground_factor[index] - expected.ground_factor).abs() < 1e-9);
            }
            // One vertex per lattice line between the ends (none passes through a node here).
            let lattice = |metres: [f64; 2]| {
                let (lat, lon) = fixture.frame.to_mercator(metres).to_degrees();
                (lat * 3600.0, lon * 3600.0)
            };
            let ((row_a, column_a), (row_b, column_b)) = (lattice(source), lattice(receiver));
            let lines =
                |p: f64, q: f64| (p.max(q).ceil() - p.min(q).floor() - 1.0).max(0.0) as usize;
            assert_eq!(
                profile.t.len(),
                2 + lines(row_a, row_b) + lines(column_a, column_b)
            );
        }
    });
}

#[test]
fn a_crest_one_node_wide_is_never_stepped_over() {
    let ridge_row = (50.08_f64 * 3600.0).round() as i64;
    let fixture = fixture(|row, _| if row == ridge_row { 260.0 } else { 200.0 }, false);
    let ridge_north_m = fixture
        .frame
        .to_metres(Mercator::from_degrees(ridge_row as f64 / 3600.0, 14.42))[1];
    fixture.with_ground(|ground| {
        let mut profile = Profile::default();
        for (offset, slope) in [(0.0, 0.0), (7.3, 0.4), (-11.0, -1.7), (3.0, 2.5)] {
            let source = [-800.0, ridge_north_m - 600.0 + offset];
            let receiver = [-800.0 + 1_200.0 * slope, ridge_north_m + 650.0 + offset];
            ground.fill_profile(source, receiver, &mut profile).unwrap();
            let highest = profile.ground_m.iter().copied().fold(f64::MIN, f64::max);
            assert!(
                (highest - 260.0).abs() < 0.2,
                "{highest} on {source:?} -> {receiver:?}"
            );
        }
    });
}

#[test]
fn an_ocean_tile_is_at_sea_level_and_soft_ground_stays_read() {
    let fixture = fixture(rugged, true);
    fixture.with_ground(|ground| {
        let mut profile = Profile::default();
        ground
            .fill_profile([0.0, 0.0], [9_000.0, 0.0], &mut profile)
            .unwrap();
        let last = profile.t.len() - 1;
        assert_eq!(
            (profile.ground_m[last], profile.ground_factor[last]),
            (OCEAN.height_m, OCEAN.ground_factor)
        );
        assert!(profile.ground_m[1] > 150.0);
    });
}
