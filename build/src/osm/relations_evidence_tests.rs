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
