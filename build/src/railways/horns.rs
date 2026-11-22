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
    use crate::osm::geometry::lonlat_to_grid;
    use arrow_array::{ArrayRef, Float32Array, Float64Array, Int32Array, UInt8Array, UInt16Array};
    use arrow_ipc::writer::FileWriter;
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

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
            let ((a, b), (c, d)) = (lonlat_to_grid(s_lon, s_lat), lonlat_to_grid(e_lon, e_lat));
            sgx.push(a);
            sgy.push(b);
            egx.push(c);
            egy.push(d);
            len.push(flat_dist(s_lat, s_lon, e_lat, e_lon) as f32);
            iso.push(u16::from_le_bytes(country));
        }
        let n = rows.len();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(sgx)) as ArrayRef,
                Arc::new(Int32Array::from(sgy)),
                Arc::new(Int32Array::from(egx)),
                Arc::new(Int32Array::from(egy)),
                Arc::new(Float32Array::from(len)),
                Arc::new(UInt8Array::from(vec![0u8; n])),
                Arc::new(UInt16Array::from(iso)),
                Arc::new(UInt16Array::from(vec![0u16; n])),
                Arc::new(UInt8Array::from(vec![0u8; n])),
                Arc::new(Float64Array::from(vec![0.0; n])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn non_whole_line_horns_keep_legacy_stamp_clip_and_neighbor_behavior() {
        let model = horn_geometry::tests::test_model();
        let border_lat = 49.0;
        let regions = horn_geometry::tests::us_ca_regions(border_lat);
        let (lon_us, lon_ca) = (-112.0, -111.9);
        let (d20, d150, d500) = (20.0 / 110_540.0, 150.0 / 110_540.0, 500.0 / 110_540.0);
        let tracks = [
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
        ];
        let base = base_batch(&tracks);
        let us_own = horn_geometry::tests::test_crossing(
            "US1",
            border_lat - d20,
            lon_us,
            [2.0, 0.0, 0.0],
            64.37,
            111,
            -10,
        );
        let ca_own = horn_geometry::tests::test_crossing(
            "CA1",
            border_lat + d20,
            lon_ca,
            [3.0, 0.0, 0.0],
            64.37,
            112,
            -20,
        );

        let root = tempfile::tempdir().unwrap();
        let nb_dir = root.path().join("z9/97/175");
        std::fs::create_dir_all(&nb_dir).unwrap();
        let mut writer = FileWriter::try_new(
            std::fs::File::create(nb_dir.join("railways.arrow")).unwrap(),
            &base.schema(),
        )
        .unwrap();
        writer.write(&base).unwrap();
        writer.finish().unwrap();

        let all_crossings = HashMap::from([(
            [97, 175],
            vec![
                horn_geometry::tests::test_crossing(
                    "US_NB",
                    border_lat - d20 - d150,
                    lon_us,
                    [5.0, 0.0, 0.0],
                    18.0,
                    111,
                    -11,
                ),
                horn_geometry::tests::test_crossing(
                    "CA_NB",
                    border_lat + d20 + d150,
                    lon_ca,
                    [7.0, 0.0, 0.0],
                    18.0,
                    112,
                    -21,
                ),
            ],
        )]);
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

        let (rt, src, fm, len) = (
            super::super::write::col_u8(&out, "rail_type").unwrap(),
            super::super::write::col_u16(&out, "passenger_source_id").unwrap(),
            super::super::write::col_u8(&out, "freight_matching").unwrap(),
            super::super::write::col_f32(&out, "length_m").unwrap(),
        );
        let day = out
            .column_by_name("trains_passenger_day")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let (mut us_pieces, mut ca_pieces) = (Vec::new(), Vec::new());
        for i in 0..out.num_rows() {
            if rt.value(i) == 6 {
                let tup = (fm.value(i), len.value(i), day.value(i));
                match src.value(i) {
                    111 => us_pieces.push(tup),
                    112 => ca_pieces.push(tup),
                    other => panic!("unexpected horn source {other}"),
                }
            }
        }
        assert!(!us_pieces.is_empty());
        assert!(
            us_pieces
                .iter()
                .all(|&(m, _, _)| m == super::super::encode::MATCHING_GLOBAL_SECTION)
        );
        assert!(us_pieces.iter().any(|&(_, l, _)| l > 15.0 && l < 25.0));
        let us_total_len: f32 = us_pieces.iter().map(|&(_, l, _)| l).sum();
        assert!((us_total_len - 277.6).abs() < 3.0);
        assert_eq!(ca_pieces.len(), 2);
        for &(m, l, d) in &ca_pieces {
            assert_eq!(m, 0);
            assert!((l - 357.61).abs() < 1.5 && (d - 1.5).abs() < 1e-9);
        }
    }
}
