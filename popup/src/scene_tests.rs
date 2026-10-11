//! The ray's profile: a vertex on every lattice line it crosses, each on the bilinear surface with
//! its nearest node's trees (none from a node without forest data), a crest one node wide never
//! stepped over, an ocean tile at sea level.

use super::*;
use tiles::terrain::{Window, encode};

/// Three by three tiles around Prague's centre, every node's height, imperviousness and trees from
/// its global lattice row and column; `ocean` leaves the eastern column of tiles absent.
struct Fixture {
    tiles: Vec<(TileId, Option<Vec<u8>>)>,
    frame: LocalFrame,
    centre: TileId,
}

fn fixture(height: impl Fn(i64, i64) -> f64, ocean: bool) -> Fixture {
    fixture_at(14.42, height, |dx| ocean && dx == 1)
}

/// The same around the tile at `lon` (50.08 N), the columns of tiles `ocean` names absent.
fn fixture_at(lon: f64, height: impl Fn(i64, i64) -> f64, ocean: impl Fn(i32) -> bool) -> Fixture {
    let centre = TileId::containing(Mercator::from_degrees(50.08, lon));
    let mut tiles = Vec::new();
    for dy in -1..=1_i32 {
        for dx in -1..=1_i32 {
            let tile = TileId {
                x: centre.x.wrapping_add_signed(dx),
                y: centre.y.wrapping_add_signed(dy),
            };
            if ocean(dx) {
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
                        forest_percent: (row * 5 + column * 11).rem_euclid(101) as u8,
                        canopy_m: (row * 3 + column * 13).rem_euclid(40) as u8,
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
        let mut ground = Ground::new(self.frame, self.centre, 1, None);
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
fn every_vertex_lies_on_the_bilinear_surface_and_on_a_lattice_line_with_its_nearest_trees() {
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
                assert_eq!(
                    (profile.forest_cover[index], profile.canopy_m[index]),
                    (expected.forest_cover, expected.canopy_m),
                    "{t}"
                );
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

#[test]
fn a_ray_starting_on_a_row_line_reads_the_stretch_it_heads_into() {
    let fixture = fixture(rugged, false);
    let row = (50.08_f64 * 3600.0).round();
    let on_row = |lon: f64| {
        fixture
            .frame
            .to_metres(Mercator::from_degrees(row / 3600.0, lon))
    };
    fixture.with_ground(|ground| {
        let mut profile = Profile::default();
        for (source, receiver) in [
            (
                on_row(14.42),
                [on_row(14.42)[0] + 700.0, on_row(14.42)[1] + 900.0],
            ),
            (
                on_row(14.43),
                [on_row(14.43)[0] - 500.0, on_row(14.43)[1] - 800.0],
            ),
            (
                on_row(14.41),
                [on_row(14.41)[0] + 1_300.0, on_row(14.41)[1]],
            ),
        ] {
            let start_y = fixture.frame.to_mercator(source).y;
            assert!(
                ground.row_y.contains(&start_y),
                "the start lies on a row line"
            );
            ground.fill_profile(source, receiver, &mut profile).unwrap();
            for (index, &t) in profile.t.iter().enumerate() {
                let point = [
                    source[0] + t * (receiver[0] - source[0]),
                    source[1] + t * (receiver[1] - source[1]),
                ];
                let expected = ground.at(point).unwrap().height_m;
                assert!(
                    (profile.ground_m[index] - expected).abs() < 1e-6,
                    "{t} {source:?}"
                );
            }
        }
    });
}

#[test]
fn a_reversed_ray_reads_the_same_ground() {
    let fixture = fixture(rugged, false);
    fixture.with_ground(|ground| {
        let (mut forward, mut backward) = (Profile::default(), Profile::default());
        for (source, receiver) in RAYS {
            ground.fill_profile(source, receiver, &mut forward).unwrap();
            ground
                .fill_profile(receiver, source, &mut backward)
                .unwrap();
            assert_eq!(forward.t.len(), backward.t.len());
            for (index, &t) in forward.t.iter().enumerate() {
                let mirror = backward.t.len() - 1 - index;
                assert!((t - (1.0 - backward.t[mirror])).abs() < 1e-9);
                assert!((forward.ground_m[index] - backward.ground_m[mirror]).abs() < 1e-6);
                assert!(
                    (forward.ground_factor[index] - backward.ground_factor[mirror]).abs() < 1e-9
                );
            }
        }
    });
}

#[test]
fn a_vertex_on_a_coast_seam_reads_the_land_tile() {
    // Tile 2208's west edge, 14.0625 E, is a lattice column (every 32nd tile meridian is); the
    // vertex on it belongs to tile 2208 by the floor, here ocean, while the land tile west of it
    // holds the seam's nodes.
    let fixture = fixture_at(14.08, rugged, |dx| dx >= 0);
    assert_eq!(fixture.centre.x % 32, 0);
    let seam = fixture
        .frame
        .to_metres(Mercator::from_degrees(50.08, 14.0625))[0];
    fixture.with_ground(|ground| {
        let mut profile = Profile::default();
        let (source, receiver) = ([seam - 300.0, 20.0], [seam + 200.0, -35.0]);
        ground.fill_profile(source, receiver, &mut profile).unwrap();
        let on_seam = profile
            .t
            .iter()
            .position(|&t| (source[0] + t * (receiver[0] - source[0]) - seam).abs() < 1e-6)
            .expect("a vertex on the seam");
        assert!(
            profile.ground_m[on_seam] > 150.0,
            "{}",
            profile.ground_m[on_seam]
        );
        assert_eq!(*profile.ground_m.last().unwrap(), OCEAN.height_m);
    });
}

/// A vertex takes the trees of its nearer node, and refuses a nearer node without forest data, as
/// `Terrain::sample` does (a clamp read it as full cover: up to 24 dB at 8 kHz over 200 m).
#[test]
fn a_vertex_refuses_a_nearer_node_without_forest_data() {
    let node = |forest_percent| Node {
        height_code: 3_000,
        impervious_percent: 0,
        forest_percent,
        canopy_m: 20,
    };
    let (trees, no_data) = (node(40), node(PERCENT_MAX + 1));
    let cover = edge_sample(Some(trees), Some(no_data), 0.3).map(|sample| sample.forest_cover);
    assert_eq!(cover, Some(0.4));
    assert!(edge_sample(Some(trees), Some(no_data), 0.7).is_none());
}

/// A fresh year root with the fine terrain of the fixture's land tiles that `fine` names, every
/// fine node's height from its global 1/6" row and column.
fn fine_root(
    fixture: &Fixture,
    height: impl Fn(i64, i64) -> f64,
    fine: impl Fn(TileId) -> bool,
) -> std::path::PathBuf {
    static ROOTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "qm-fine-test-{}-{}",
        std::process::id(),
        ROOTS.fetch_add(1, Ordering::Relaxed)
    ));
    for (tile, bytes) in &fixture.tiles {
        if bytes.is_none() || !fine(*tile) {
            continue;
        }
        let window = tiles::terrain_fine::fine_window(Window::of_tile(*tile));
        let heights: Vec<u16> = (0..window.rows)
            .flat_map(|r| (0..window.columns).map(move |c| (r, c)))
            .map(|(r, c)| {
                tiles::terrain::code_of_height_m(height(
                    i64::from(window.north_node) - i64::from(r),
                    i64::from(window.west_node) + i64::from(c),
                ))
            })
            .collect();
        let path = tile_path(&root, *tile, Kind::TerrainFine);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, tiles::terrain_fine::encode(window, &heights)).unwrap();
    }
    root
}

impl Fixture {
    fn with_fine_ground<T>(&self, root: &std::path::Path, f: impl FnOnce(&Ground) -> T) -> T {
        let mut ground = Ground::new(self.frame, self.centre, 1, Some(root.to_path_buf()));
        for (tile, bytes) in &self.tiles {
            ground.insert(*tile, bytes.as_deref().map(|b| Terrain::parse(b).unwrap()));
        }
        f(&ground)
    }
}

/// A berm 7 m high on every 23rd fine row: what the one arc-second lattice cannot hold.
fn bermed(row: i64, column: i64) -> f64 {
    rugged(row / 6, column / 6) + if row.rem_euclid(23) == 0 { 7.0 } else { 0.0 }
}

/// Rays within the clicked tile, every way round, long and short.
const FINE_RAYS: [([f64; 2], [f64; 2]); 5] = [
    ([0.0, 0.0], [1_000.0, 0.0]),
    ([0.0, 0.0], [0.0, -1_500.0]),
    ([-120.0, 340.0], [2_700.0, -2_300.0]),
    ([12.3, 45.6], [-31.4, -15.9]),
    ([1_900.0, -800.0], [-150.0, 260.0]),
];

#[test]
fn near_either_end_the_ray_walks_the_fine_lattice_and_one_vertex_marks_each_switch() {
    let fixture = fixture(rugged, false);
    let root = fine_root(&fixture, bermed, |tile| tile == fixture.centre);
    fixture.with_fine_ground(&root, |ground| {
        let mut profile = Profile::default();
        for (source, receiver) in FINE_RAYS {
            ground.fill_profile(source, receiver, &mut profile).unwrap();
            let length = (receiver[0] - source[0]).hypot(receiver[1] - source[1]);
            assert!(profile.t.windows(2).all(|pair| pair[0] < pair[1]));
            // A vertex's lattice coordinates as the walk lays the lines: columns by longitude,
            // rows evenly in Mercator y between the one arc-second rows.
            let on_line = |metres: [f64; 2], scale: f64| {
                let position = fixture.frame.to_mercator(metres);
                let column = position.to_degrees().1 * 3_600.0 * scale;
                let rows = &ground.row_y;
                let j = rows.partition_point(|&y| y <= position.y) - 1;
                let row = scale * (j as f64 + (position.y - rows[j]) / (rows[j + 1] - rows[j]));
                let near = |v: f64| (v - v.round()).abs() < 1e-6;
                near(row) || near(column)
            };
            let mut switches = 0;
            for (index, &t) in profile.t.iter().enumerate() {
                let point = [
                    source[0] + t * (receiver[0] - source[0]),
                    source[1] + t * (receiver[1] - source[1]),
                ];
                let from_end = t.min(1.0 - t) * length;
                let fine = from_end <= FINE_ENDS_M + 1e-6;
                let expected = if fine {
                    ground.at(point).unwrap()
                } else {
                    ground.coarse_at(point).unwrap()
                };
                assert!(
                    (profile.ground_m[index] - expected.height_m).abs() < 1e-6,
                    "{t} {source:?}"
                );
                assert!((profile.ground_factor[index] - expected.ground_factor).abs() < 1e-6);
                if index == 0 || index + 1 == profile.t.len() {
                    continue;
                }
                if (from_end - FINE_ENDS_M).abs() < 1e-6 && length > 2.0 * FINE_ENDS_M {
                    switches += 1;
                } else {
                    let scale = if fine { 6.0 } else { 1.0 };
                    assert!(on_line(point, scale), "{t} {source:?} off its lattice");
                }
            }
            assert_eq!(switches, if length > 2.0 * FINE_ENDS_M { 2 } else { 0 });
        }
    });
    std::fs::remove_dir_all(&root).unwrap();
}

/// Where a tile under an end's stretch has no fine terrain, that stretch walks the one arc-second
/// lattice as before, bit for bit; without any fine terrain the whole ray does.
#[test]
fn a_stretch_without_fine_terrain_walks_the_one_arc_second_lattice() {
    let fixture = fixture(rugged, false);
    let root = fine_root(&fixture, bermed, |tile| tile == fixture.centre);
    let east_edge = fixture
        .frame
        .to_metres(fixture.centre.to_mercator([16_384, 0]))[0];
    let (source, receiver) = ([0.0, 0.0], [east_edge + 800.0, 120.0]);
    let (mut fine, mut coarse) = (Profile::default(), Profile::default());
    fixture.with_ground(|ground| ground.fill_profile(source, receiver, &mut coarse).unwrap());
    fixture.with_fine_ground(&root, |ground| {
        ground.fill_profile(source, receiver, &mut fine).unwrap();
        // Without fine terrain at either end the profile is the one arc-second walk's.
        let mut both_coarse = Profile::default();
        let (west, east) = ([-east_edge - 900.0, 0.0], [east_edge + 900.0, 0.0]);
        ground.fill_profile(west, east, &mut both_coarse).unwrap();
        let mut plain = Profile::default();
        fixture.with_ground(|ground| ground.fill_profile(west, east, &mut plain).unwrap());
        assert_eq!(
            (both_coarse.t, both_coarse.ground_m),
            (plain.t, plain.ground_m)
        );
    });
    let switch = FINE_ENDS_M / (receiver[0] - source[0]).hypot(receiver[1] - source[1]);
    let beyond = |profile: &Profile| -> Vec<(f64, f64, f64)> {
        (0..profile.t.len())
            .filter(|&k| profile.t[k] > switch)
            .map(|k| (profile.t[k], profile.ground_m[k], profile.ground_factor[k]))
            .collect()
    };
    assert_eq!(beyond(&fine), beyond(&coarse));
    assert!(
        fine.t.iter().filter(|&&t| t < switch).count()
            > 2 * coarse.t.iter().filter(|&&t| t < switch).count()
    );
    std::fs::remove_dir_all(&root).unwrap();
}
