//! structures.arrow decoding: z30 geometry, footprint ids, the antimeridian and wall heights.

use super::*;

fn at(x: i64, y: i64) -> GlobalSteps {
    GlobalSteps { x, y }
}

#[test]
fn z30_geometry_decodes_whole_or_not_at_all() {
    let mut bytes = Vec::new();
    for word in [1u32, 2, 3] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for value in [0i32, 0, 10, 0, 10, 10] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&3u32.to_le_bytes());
    for value in [2i32, 2, 4, 2, 4, 4] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let parts = decode_parts(&bytes).unwrap();
    assert_eq!(
        parts,
        vec![vec![
            vec![(0, 0), (10, 0), (10, 10)],
            vec![(2, 2), (4, 2), (4, 4)]
        ]]
    );
    assert_eq!(decode_parts(&bytes[..bytes.len() - 1]), None);
    assert_eq!(decode_parts(&[bytes.as_slice(), &[0]].concat()), None);
    let wall: Vec<u8> = [2i32, 5, 6, 7, 8]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(decode_wall(&wall), Some(vec![(5, 6), (7, 8)]));
    assert_eq!(decode_wall(&wall[..wall.len() - 4]), None);
}

#[test]
fn footprint_ids_name_the_owner_square_and_outlines_run_across_the_antimeridian() {
    let id = footprint_id(Square { x: 276, y: 173 }, 591_408);
    assert_eq!(
        (id >> 48, (id >> 32) & 0xffff, id & 0xffff_ffff),
        (276, 173, 591_408)
    );
    let east_edge = WORLD_STEPS - 3;
    assert_eq!(nearest_copy(at(east_edge, 9), 100), at(-3, 9));
    assert_eq!(nearest_copy(at(2, 9), east_edge), at(WORLD_STEPS + 2, 9));
    assert_eq!(nearest_copy(at(2, 9), 5_000), at(2, 9));
}

#[test]
fn an_unmapped_wall_stands_four_metres_and_a_mapped_one_its_own_height() {
    assert_eq!(wall_height_m(HEIGHT_SOURCE_WALL_DEFAULT, 2), 4.0);
    assert_eq!(wall_height_m(HEIGHT_SOURCE_WALL_DEFAULT, 6), 4.0);
    assert_eq!(wall_height_m(0, 6), 6.0);
}
