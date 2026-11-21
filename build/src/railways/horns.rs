//! Append the two directed sounding approaches of each crossing, merging shared soundings.
use super::{horn_geometry, horn_write, model::RailModel, types::Crossing};
use crate::osm::geometry::{Square, flat_dist};
use arrow_array::RecordBatch;
use arrow_select::concat::concat_batches;
use std::collections::HashMap;
use std::path::Path;

pub struct NeighborHorns<'a> {
    pub geography: &'a Path,
    pub all_crossings: &'a HashMap<[u16; 2], Vec<Crossing>>,
}

fn append_neighbor_approaches(
    approaches: &mut Vec<horn_geometry::Approach>,
    model: &RailModel,
    own_sounding: &[&Crossing],
    square: Square,
    neighbors: NeighborHorns<'_>,
    regions: Option<&crate::osm::geography::Regions>,
) -> Result<(), String> {
    let reach = horn_geometry::CROSS_SQUARE_HORN_REACH_M;
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (i32::from(square.x) + dx, i32::from(square.y) + dy);
            if !(0..512).contains(&nx) || !(0..512).contains(&ny) {
                continue;
            }
            let nb_key = [nx as u16, ny as u16];
            let Some(nb_list) = neighbors.all_crossings.get(&nb_key) else {
                continue;
            };
            let near: Vec<&Crossing> = nb_list
                .iter()
                .filter(|nb| {
                    model.stamps_whole_line(nb.source_id)
                        && nb.soundings.iter().any(|&s| s > 0.0)
                        && own_sounding
                            .iter()
                            .any(|own| flat_dist(own.lat, own.lon, nb.lat, nb.lon) <= reach)
                })
                .collect();
            if near.is_empty() {
                continue;
            }
            let path = neighbors
                .geography
                .join(format!("z9/{nx}/{ny}/railways.arrow"));
            let Some(table) = crate::dev4::read_table(&path)? else {
                continue;
            };
            let Some(first) = table.batches.first() else {
                continue;
            };
            let merged = concat_batches(&first.schema(), &table.batches)
                .map_err(|err| format!("concat neighbor horn railways: {err}"))?;
            let nb_rows = horn_geometry::base_rows(&merged)?;
            for crossing in near {
                if let Some((row, latitude, longitude)) =
                    horn_geometry::match_crossing(model, crossing, &nb_rows)
                {
                    approaches.extend(horn_geometry::build_approaches(
                        0, crossing, row, latitude, longitude, &nb_rows, regions, false,
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn append(
    base: &RecordBatch,
    model: &RailModel,
    crossings: &[Crossing],
    regions: Option<&crate::osm::geography::Regions>,
    square: Square,
    neighbors: Option<NeighborHorns<'_>>,
) -> Result<RecordBatch, String> {
    let own_sounding: Vec<&Crossing> = crossings
        .iter()
        .filter(|crossing| crossing.soundings.iter().any(|&s| s > 0.0))
        .collect();
    let sounding_count = own_sounding.len();
    if sounding_count == 0 {
        return Ok(base.clone());
    }
    let rows = horn_geometry::base_rows(base)?;
    let mut approaches = Vec::new();
    let mut unmatched = 0;
    for (index, crossing) in crossings.iter().enumerate() {
        if crossing.soundings.iter().all(|&s| s <= 0.0) {
            continue;
        }
        if let Some((row, latitude, longitude)) =
            horn_geometry::match_crossing(model, crossing, &rows)
        {
            let clip = regions.filter(|_| model.stamps_whole_line(crossing.source_id));
            approaches.extend(horn_geometry::build_approaches(
                index, crossing, row, latitude, longitude, &rows, clip, true,
            ));
        } else {
            unmatched += 1;
        }
    }
    if let Some(neighbors) = neighbors {
        let global_own: Vec<&Crossing> = own_sounding
            .iter()
            .copied()
            .filter(|crossing| model.stamps_whole_line(crossing.source_id))
            .collect();
        if !global_own.is_empty() {
            append_neighbor_approaches(
                &mut approaches,
                model,
                &global_own,
                square,
                neighbors,
                regions,
            )?;
        }
    }
    approaches.sort_by_key(|approach| (approach.synth_osm, approach.approach_idx));
    let pieces = horn_geometry::merge_overlaps(&approaches);
    if pieces.is_empty() {
        return Ok(base.clone());
    }
    let horns =
        horn_write::build_horn_batch(&base.schema(), base, model, crossings, &approaches, &pieces)?;
    eprintln!(
        "railways: {sounding_count} sounding crossings, {unmatched} unmatched, {} horn pieces",
        pieces.len()
    );
    concat_batches(&base.schema(), &[base.clone(), horns]).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm::geography::Regions;
    use crate::osm::geometry::lonlat_to_grid;
    use crate::osm::region_input::{RegionFile, RegionInput};
    use arrow_array::{ArrayRef, Float32Array, Float64Array, Int32Array, UInt8Array, UInt16Array};
    use arrow_ipc::writer::FileWriter;
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    fn horn_test_model() -> RailModel {
        serde_json::from_value(serde_json::json!({
            "network_bounds": {},
            "residuals": {},
            "spatial_priors": {},
            "sources": [
                {"id": 111, "rank": 4, "edition": 2024, "owner": "US", "countries": ["US"], "residual": false, "whole_line": true},
                {"id": 112, "rank": 4, "edition": 2024, "owner": "CA", "countries": ["CA"], "residual": false, "whole_line": false}
            ],
            "type_priors": [[80.0, 20.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            "usage_priors": [[80.0, 20.0], [30.0, 5.0], [0.0, 15.0], [40.0, 10.0]],
            "mainline_freight": {},
            "passenger_periods": [0.7, 0.2, 0.1],
            "freight_periods": [0.4, 0.2, 0.4],
            "urban_periods": [0.6, 0.25, 0.15],
            "regional_freight_periods": []
        }))
        .unwrap()
    }

    type TestRow = ([f64; 2], [f64; 2], [u8; 2]);

    fn base_batch(rows: &[TestRow]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("start_gx", DataType::Int32, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
            Field::new("length_m", DataType::Float32, false),
            Field::new("rail_type", DataType::UInt8, false),
            Field::new("country_iso", DataType::UInt16, false),
            Field::new("passenger_source_id", DataType::UInt16, false),
            Field::new("freight_matching", DataType::UInt8, false),
            Field::new("trains_passenger_day", DataType::Float64, false),
        ]));
        let (mut sgx, mut sgy, mut egx, mut egy, mut len, mut iso) = (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        for &([s_lon, s_lat], [e_lon, e_lat], country) in rows {
            let (a, b) = lonlat_to_grid(s_lon, s_lat);
            let (c, d) = lonlat_to_grid(e_lon, e_lat);
            sgx.push(a);
            sgy.push(b);
            egx.push(c);
            egy.push(d);
            len.push(flat_dist(s_lat, s_lon, e_lat, e_lon) as f32);
            iso.push(u16::from_le_bytes(country));
        }
        let n = rows.len();
        let cols: Vec<ArrayRef> = vec![
            Arc::new(Int32Array::from(sgx)),
            Arc::new(Int32Array::from(sgy)),
            Arc::new(Int32Array::from(egx)),
            Arc::new(Int32Array::from(egy)),
            Arc::new(Float32Array::from(len)),
            Arc::new(UInt8Array::from(vec![0u8; n])),
            Arc::new(UInt16Array::from(iso)),
            Arc::new(UInt16Array::from(vec![0u16; n])),
            Arc::new(UInt8Array::from(vec![0u8; n])),
            Arc::new(Float64Array::from(vec![0.0; n])),
        ];
        RecordBatch::try_new(schema, cols).unwrap()
    }

    #[test]
    fn non_whole_line_horns_keep_legacy_stamp_clip_and_neighbor_behavior() {
        let model = horn_test_model();
        let border_lat = 49.0;
        let regions = Regions::from_input(RegionFile {
            regions: vec![
                RegionInput {
                    country: Some("US".into()),
                    continent: 1,
                    disputed: false,
                    polygons: vec![vec![vec![
                        [-113.0, 48.0],
                        [-111.0, 48.0],
                        [-111.0, border_lat],
                        [-113.0, border_lat],
                        [-113.0, 48.0],
                    ]]],
                },
                RegionInput {
                    country: Some("CA".into()),
                    continent: 1,
                    disputed: false,
                    polygons: vec![vec![vec![
                        [-113.0, border_lat],
                        [-111.0, border_lat],
                        [-111.0, 50.0],
                        [-113.0, 50.0],
                        [-113.0, border_lat],
                    ]]],
                },
            ],
        })
        .unwrap();

        // One US track ending 20 m south of the border, one CA track starting 20 m north.
        let lon_us = -112.0;
        let lon_ca = -111.9;
        let d20 = 20.0 / 110_540.0;
        let d500 = 500.0 / 110_540.0;
        let base = base_batch(&[
            (
                [lon_us, border_lat - d500],
                [lon_us, border_lat - d20],
                *b"US",
            ),
            (
                [lon_ca, border_lat + d20],
                [lon_ca, border_lat + d500],
                *b"CA",
            ),
        ]);
        let us_own = Crossing {
            id: "US1".into(),
            lat: border_lat - d20,
            lon: lon_us,
            soundings: [2.0, 0.0, 0.0],
            speed_kmh: Some(64.37),
            source_id: 111,
            synth_osm: -10,
            total_periods: None,
            passenger_daily: None,
            passenger_lt1: false,
            measured_periods: false,
        };
        let ca_own = Crossing {
            id: "CA1".into(),
            lat: border_lat + d20,
            lon: lon_ca,
            soundings: [3.0, 0.0, 0.0],
            speed_kmh: Some(64.37),
            source_id: 112,
            synth_osm: -20,
            total_periods: None,
            passenger_daily: None,
            passenger_lt1: false,
            measured_periods: false,
        };

        // Neighbor square (97, 175) has higher-rate US (111) and CA (112) crossings 150 m away
        // with 100 m approaches (18 km/h), overlapping only the inland approach of each own crossing.
        let d150 = 150.0 / 110_540.0;
        let nb_batch = base_batch(&[
            (
                [lon_us, border_lat - d500],
                [lon_us, border_lat - d20],
                *b"US",
            ),
            (
                [lon_ca, border_lat + d20],
                [lon_ca, border_lat + d500],
                *b"CA",
            ),
        ]);
        let root = tempfile::tempdir().unwrap();
        let nb_dir = root.path().join("z9/97/175");
        std::fs::create_dir_all(&nb_dir).unwrap();
        let file = std::fs::File::create(nb_dir.join("railways.arrow")).unwrap();
        let mut writer = FileWriter::try_new(file, &nb_batch.schema()).unwrap();
        writer.write(&nb_batch).unwrap();
        writer.finish().unwrap();

        let mut all_crossings = HashMap::new();
        all_crossings.insert(
            [97, 175],
            vec![
                Crossing {
                    id: "US_NB".into(),
                    lat: border_lat - d20 - d150,
                    lon: lon_us,
                    soundings: [5.0, 0.0, 0.0],
                    speed_kmh: Some(18.0),
                    synth_osm: -11,
                    ..us_own.clone()
                },
                Crossing {
                    id: "CA_NB".into(),
                    lat: border_lat + d20 + d150,
                    lon: lon_ca,
                    soundings: [7.0, 0.0, 0.0],
                    speed_kmh: Some(18.0),
                    synth_osm: -21,
                    ..ca_own.clone()
                },
            ],
        );

        let out = append(
            &base,
            &model,
            &[us_own, ca_own],
            Some(&regions),
            Square { x: 96, y: 175 },
            Some(NeighborHorns {
                geography: root.path(),
                all_crossings: &all_crossings,
            }),
        )
        .unwrap();

        let rt = super::super::write::col_u8(&out, "rail_type").unwrap();
        let src = super::super::write::col_u16(&out, "passenger_source_id").unwrap();
        let fm = super::super::write::col_u8(&out, "freight_matching").unwrap();
        let len = super::super::write::col_f32(&out, "length_m").unwrap();
        let day = out
            .column_by_name("trains_passenger_day")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();

        let mut us_pieces = Vec::new();
        let mut ca_pieces = Vec::new();
        for i in 0..out.num_rows() {
            if rt.value(i) != 6 {
                continue;
            }
            let tup = (fm.value(i), len.value(i), day.value(i));
            match src.value(i) {
                111 => us_pieces.push(tup),
                112 => ca_pieces.push(tup),
                other => panic!("unexpected horn source {other}"),
            }
        }
        // US 111 (whole_line = true): stamped with MATCHING_GLOBAL_SECTION, clipped at border
        // (~20 m north approach), and deduplicated against the higher-rate neighbor on its south
        // approach (~257.6 m retained out of 357.6 m -> ~277.6 m total).
        assert!(!us_pieces.is_empty());
        assert!(
            us_pieces
                .iter()
                .all(|&(m, _, _)| m == super::super::encode::MATCHING_GLOBAL_SECTION),
            "us_pieces={us_pieces:?}"
        );
        assert!(
            us_pieces.iter().any(|&(_, l, _)| l > 15.0 && l < 25.0),
            "US north approach should clip at border: {us_pieces:?}"
        );
        let us_total_len: f32 = us_pieces.iter().map(|&(_, l, _)| l).sum();
        assert!(
            (us_total_len - 277.6).abs() < 3.0,
            "US south approach should lose 100 m overlap to higher-rate neighbor: {us_total_len} ({us_pieces:?})"
        );

        // CA 112 (whole_line = false): unstamped (0), unclipped (two full ~357.6 m approaches),
        // and unaffected by the neighbor crossing.
        assert_eq!(ca_pieces.len(), 2, "ca_pieces={ca_pieces:?}");
        for &(m, l, d) in &ca_pieces {
            assert_eq!(m, 0, "CA horn must not carry MATCHING_GLOBAL_SECTION");
            assert!((l - 357.61).abs() < 1.5, "CA horn must stay unclipped: {l}");
            assert!((d - 1.5).abs() < 1e-9, "CA horn must ignore neighbor: {d}");
        }
    }
}
