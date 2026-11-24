//! Regression cases for relations.

use super::*;
#[test]
fn shared_last_member_survives_until_all_completed_relations_are_assembled() {
    for order in [[100, 200], [200, 100]] {
        let manifest = RelationManifest {
            relations: [(100, 11), (200, 12)]
                .into_iter()
                .map(|(id, way)| {
                    (
                        id,
                        RelationInfo {
                            feature_types: vec![FeatureType::Industrial],
                            tags: Tags::new(),
                            member_ways: vec![(way, "outer".into()), (30, "outer".into())],
                        },
                    )
                })
                .collect(),
            way_to_relations: HashMap::from([
                (11, vec![(100, "outer".into())]),
                (12, vec![(200, "outer".into())]),
                (
                    30,
                    order.into_iter().map(|id| (id, "outer".into())).collect(),
                ),
            ]),
        };
        let mut assembler = RelationAssembler::new(&manifest);
        assert!(
            assembler
                .add_way(11, vec![[0., 0.], [0., 1.], [1., 1.]], &manifest)
                .is_empty()
        );
        assert!(
            assembler
                .add_way(12, vec![[0., 0.], [1., 0.], [1., 1.]], &manifest)
                .is_empty()
        );
        let completed = assembler.add_way(30, vec![[1., 1.], [0., 0.]], &manifest);
        assert_eq!(completed, order);
        for id in completed {
            let relation = assembler.assemble(id, &manifest).unwrap();
            assert_eq!(relation.rings.len(), 1);
            assert!(
                crate::osm::classify::is_a_closed_ring(&relation.rings[0]),
                "relation {id}"
            );
            assembler.cleanup(id, &manifest);
        }
        assert!(assembler.way_geoms.is_empty());
        assert!(assembler.pending_count.is_empty());
    }
}

#[test]
fn a_closed_outer_is_not_consumed_by_an_unclosed_fragment() {
    let a = [0., 0.];
    let b = [0., 1.];
    let c = [1., 1.];
    let closed = vec![a, b, c, a];
    let rings = merge_outer_rings(vec![vec![a, b, c], closed.clone()]);
    assert!(rings.contains(&closed));
    assert_eq!(rings.len(), 2);
}

#[test]
fn outer_parts_and_reversed_members_survive() {
    let a = [0., 0.];
    let b = [0., 1.];
    let c = [1., 1.];
    let d = [2., 2.];
    let e = [2., 3.];
    let f = [3., 3.];
    let rings = merge_outer_rings(vec![vec![b, c], vec![a, b], vec![a, c], vec![d, e, f, d]]);
    assert_eq!(rings.len(), 2);
    assert_eq!(rings[0].first(), rings[0].last());
    assert_eq!(rings[0].len(), 4);
    assert_eq!(rings[1], vec![d, e, f, d]);
}

#[test]
fn inner_holes_are_merged_and_assigned_to_their_immediate_enclosing_outer_ring() {
    let range_tags: Tags = [
        ("landuse".into(), "military".into()),
        ("military".into(), "training_area".into()),
    ]
    .into();
    // Large outer shell (0..20) with an outer hole (2..18), an island inside the hole (4..16),
    // and an inner hole inside that island (6..14) whose first vertex touches the island boundary:
    // even when the large shell appears first, the island's hole must belong to the island.
    let shell = vec![[0., 0.], [0., 20.], [20., 20.], [20., 0.], [0., 0.]];
    let island = vec![[4., 4.], [4., 16.], [16., 16.], [16., 4.], [4., 4.]];
    let outer_hole_a = vec![[2., 2.], [2., 18.], [18., 18.]];
    let outer_hole_b = vec![[18., 18.], [18., 2.], [2., 2.]];
    let island_hole = vec![[4., 10.], [6., 14.], [14., 14.], [14., 6.], [4., 10.]];
    let manifest = RelationManifest {
        relations: HashMap::from([(
            77,
            RelationInfo {
                feature_types: vec![FeatureType::Industrial],
                tags: range_tags,
                member_ways: vec![
                    (1, "outer".into()),
                    (2, "outer".into()),
                    (3, "inner".into()),
                    (4, "inner".into()),
                    (5, "inner".into()),
                ],
            },
        )]),
        way_to_relations: HashMap::from([
            (1, vec![(77, "outer".into())]),
            (2, vec![(77, "outer".into())]),
            (3, vec![(77, "inner".into())]),
            (4, vec![(77, "inner".into())]),
            (5, vec![(77, "inner".into())]),
        ]),
    };
    let mut assembler = RelationAssembler::new(&manifest);
    assert!(assembler.add_way(1, shell, &manifest).is_empty());
    assert!(assembler.add_way(2, island, &manifest).is_empty());
    assert!(assembler.add_way(3, outer_hole_a, &manifest).is_empty());
    assert!(assembler.add_way(4, outer_hole_b, &manifest).is_empty());
    assert_eq!(assembler.add_way(5, island_hole, &manifest), vec![77]);
    let relation = assembler.assemble(77, &manifest).unwrap();
    assert_eq!(relation.rings.len(), 2);
    assert_eq!(relation.holes_per_ring.len(), 2);
    assert_eq!(relation.holes_per_ring[0].len(), 1);
    assert_eq!(relation.holes_per_ring[1].len(), 1);
    assert!(crate::osm::classify::is_a_closed_ring(
        &relation.holes_per_ring[0][0]
    ));
    assert!(crate::osm::classify::is_a_closed_ring(
        &relation.holes_per_ring[1][0]
    ));
}
