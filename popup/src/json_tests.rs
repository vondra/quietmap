//! Streamed shares preserve the precision needed by the click list's cutoff.

use super::*;
use crate::percentiles::tests::contributor;
use crate::rows::Row;
use crate::update::{LayerAnswer, Statistics};
use tiles::geo::{LocalFrame, Mercator};

#[test]
fn streamed_loudness_shares_do_not_cross_the_list_cutoff_by_rounding() {
    let below_cutoff = 0.0099;
    let mut first_part = contributor(1, Layer::Road, 40.0, 10.0);
    first_part.share = Some(0.004876);
    let mut second_part = contributor(2, Layer::Road, 40.0, 10.0);
    second_part.share = Some(below_cutoff - first_part.share.unwrap());
    let read_display = |_, _| Ok(json!({"name": "Fixture road"}));
    let update = Update {
        partial: false,
        percentiles: None,
        loudness: None,
        rest_share: Some(below_cutoff),
        aircraft_kinds: None,
        aircraft_events: None,
        lat: 0.0,
        lon: 0.0,
        frame: LocalFrame::at(Mercator::from_degrees(0.0, 0.0)),
        receiver_altitude_m: 4.0,
        reflection_db: 0.0,
        weather: None,
        building: None,
        layers: vec![LayerAnswer {
            layer: Layer::Aircraft,
            energy: [0.0; PERIODS],
            omitted_bound: [0.0; PERIODS],
            evaluated: 0,
            candidates: 0,
            nden_sone: None,
            share: Some(below_cutoff),
            unlisted: None,
            unlisted_sources: 0,
        }],
        rows: vec![Row {
            parts: vec![first_part, second_part],
            nden_sone: None,
            share: Some(below_cutoff),
        }],
        flights: Vec::new(),
        unmodelled: &[],
        pieces: Vec::new(),
        arrival: None,
        statistics: Statistics {
            rings_read: 1,
            reads: 0,
            bytes: 0,
            read_seconds: 0.0,
            candidate_seconds: 0.0,
            evaluate_seconds: 0.0,
            elapsed_seconds: 0.0,
        },
        display_record: &read_display,
    };
    let answer: Value = serde_json::from_str(&update_line(&update, 1).unwrap()).unwrap();
    let row = &answer["top_contributors"][0];
    for share in [
        &row["share"],
        &answer["sources"][0]["share"],
        &answer["rest_share"],
    ] {
        assert_eq!(share.as_f64().unwrap(), below_cutoff);
        assert!(share.as_f64().unwrap() < 0.01);
    }
    assert_eq!(
        row["parts"][0]["share"].as_f64().unwrap(),
        0.004876 / below_cutoff
    );
    assert!(answer.get("unmodelled").is_none());
}

#[test]
fn unmodelled_sites_serialize_outline_id_and_never_round_positive_distance_to_zero() {
    let read_display = |_, _| Ok(json!({}));
    let nearby = [
        tiles::unmodelled::NearbySite {
            group_key: 0x00ab_cdef_0123_4567,
            layer: Layer::Industry,
            kind: tiles::unmodelled::UnmodelledKind::MilitaryTrainingArea,
            name: "Training Area A".into(),
            inside: true,
            distance_m: 0.0,
            area_m2: 34_200_000.0,
        },
        tiles::unmodelled::NearbySite {
            group_key: 0x0000_0000_0000_0009,
            layer: Layer::Industry,
            kind: tiles::unmodelled::UnmodelledKind::MilitaryRange,
            name: String::new(),
            inside: false,
            distance_m: 0.2,
            area_m2: 120_000.0,
        },
    ];
    let update = Update {
        partial: false,
        percentiles: None,
        loudness: None,
        rest_share: None,
        aircraft_kinds: None,
        aircraft_events: None,
        lat: 0.0,
        lon: 0.0,
        frame: LocalFrame::at(Mercator::from_degrees(0.0, 0.0)),
        receiver_altitude_m: 4.0,
        reflection_db: 0.0,
        weather: None,
        building: None,
        layers: Vec::new(),
        rows: Vec::new(),
        flights: Vec::new(),
        unmodelled: &nearby,
        pieces: Vec::new(),
        arrival: None,
        statistics: Statistics {
            rings_read: 1,
            reads: 0,
            bytes: 0,
            read_seconds: 0.0,
            candidate_seconds: 0.0,
            evaluate_seconds: 0.0,
            elapsed_seconds: 0.0,
        },
        display_record: &read_display,
    };
    let answer: Value = serde_json::from_str(&update_line(&update, 1).unwrap()).unwrap();
    let list = answer["unmodelled"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "00abcdef01234567");
    assert_eq!(list[0]["kind"], "military_training_area");
    assert_eq!(list[0]["inside"], true);
    assert_eq!(list[0]["distance_m"], 0);
    assert_eq!(list[1]["id"], "0000000000000009");
    assert_eq!(list[1]["inside"], false);
    assert_eq!(list[1]["distance_m"], 1);
}
