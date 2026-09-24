//! Native z30 Arrow source decoding, shared normalization and immutable source identities.
use crate::{input_manifest::InputManifest, source_frame::*};
use anyhow::{ensure, Context, Result};
use arrow::{array::Array, ipc::reader::FileReader, record_batch::RecordBatch};
use grid::Square;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::{normalize::*, square_country_city::SquareCountryCity};
use square_store::grid_cols::*;
use std::{io::Cursor, path::Path};
use tile_painter::corner_store::SourceIdentity;

#[path = "native_points.rs"]
mod points;
#[path = "native_traffic.rs"]
mod traffic;

#[derive(Clone)]
pub struct SurfaceSource {
    pub identity: SourceIdentity,
    pub layer: u8,
    pub device: DeviceLineSource,
}

pub fn load_sources(
    root: &Path,
    manifest: &InputManifest,
    squares: &[Square],
    frame: &RegionMetricFrame,
) -> Result<(Vec<SurfaceSource>, ObstacleSet)> {
    let mut sources = Vec::new();
    let mut indexes = Vec::new();
    for square in squares {
        let mut has_surface_arrow = false;
        let mut has_structures = false;
        for name in [
            "roads",
            "railways",
            "industrial",
            "structures",
            "leisure",
            "airport_traffic",
            "ships",
        ] {
            let relative = format!("z9/{}/{}/{name}.arrow", square.x, square.y);
            let Some((bytes, digest)) = manifest.read_arrow(root, &relative)? else {
                continue;
            };
            has_surface_arrow = true;
            if name == "structures" {
                has_structures = true;
                // The same `structures.qoix` the popup maps, checked against
                // these manifest-verified Arrow bytes.
                let index = source_reader::square_obstacle_index::load_square_obstacle_index(
                    &root.join(grid::square_name(*square)),
                    Some(source_reader::square_obstacle_index::structures_fingerprint(&bytes)),
                )
                .map_err(anyhow::Error::msg)?
                .with_context(|| format!("{relative} vanished while loading"))?;
                indexes.push(index);
            }
            let reader = FileReader::try_new(Cursor::new(bytes), None)?;
            if name == "structures" {
                square_store::structure_contract::validate_schema(&reader.schema())
                    .map_err(anyhow::Error::msg)?;
            }
            if name == "leisure" {
                // The painter must refuse a stamp it does not know for the same
                // reason the popup does: `leisure_v3` added the car park classes,
                // and an older binary would draw one as a sports pitch.
                let metadata = reader.schema().metadata().clone();
                for (key, expected) in [
                    ("leisure_contract", square_store::store::LEISURE_CONTRACT_V3),
                    ("grid", square_store::store::GRID_CONTRACT_Z30),
                ] {
                    let found = metadata.get(key).map(String::as_str);
                    anyhow::ensure!(
                        found == Some(expected),
                        "{relative}: {key} is {found:?}, this build reads {expected}"
                    );
                }
            }
            if name == "roads" {
                let empty = RecordBatch::new_empty(reader.schema());
                RoadDirections::read(&empty).map_err(anyhow::Error::msg)?;
                source_reader::road_traffic::RoadTrafficColumns::read(&empty)
                    .map_err(anyhow::Error::msg)?;
            }
            if name == "railways" {
                source_reader::rail_traffic::RailTrafficColumns::read(&RecordBatch::new_empty(
                    reader.schema(),
                ))
                .map_err(anyhow::Error::msg)?;
            }
            let traffic_calendar = (name == "airport_traffic")
                .then(|| traffic::TrafficCalendar::read(&RecordBatch::new_empty(reader.schema())))
                .transpose()?;
            let mut row_base = 0_u64;
            for batch in reader {
                let batch = batch?;
                if name == "roads" {
                    RoadDirections::read(&batch).map_err(anyhow::Error::msg)?;
                }
                let road_traffic = (name == "roads")
                    .then(|| source_reader::road_traffic::RoadTrafficColumns::read(&batch))
                    .transpose()
                    .map_err(anyhow::Error::msg)?;
                let rail_traffic = (name == "railways")
                    .then(|| source_reader::rail_traffic::RailTrafficColumns::read(&batch))
                    .transpose()
                    .map_err(anyhow::Error::msg)?;
                for row in 0..batch.num_rows() {
                    let identity =
                        |part| SourceIdentity::arrow_row(digest, row_base + row as u64, part);
                    match name {
                        "roads" | "railways" => {
                            if let Some(device) = line(
                                &batch,
                                row,
                                rail_traffic.as_ref().map(|columns| columns.row(row)),
                                road_traffic.as_ref().map(|columns| columns.row(row)),
                                frame,
                            )? {
                                sources.push(SurfaceSource {
                                    identity: identity(0),
                                    layer: u8::from(name == "railways"),
                                    device,
                                });
                            }
                        }
                        "airport_traffic" => {
                            if let Some(device) = traffic::traffic_row(
                                &batch,
                                row,
                                frame,
                                traffic_calendar.as_ref().unwrap(),
                            )? {
                                sources.push(SurfaceSource {
                                    identity: identity(0),
                                    layer: 4,
                                    device,
                                });
                            }
                        }
                        _ => {
                            for (part, point) in
                                points::points(&batch, row, name)?.iter().enumerate()
                            {
                                sources.push(SurfaceSource {
                                    identity: identity(part.try_into()?),
                                    layer: match name {
                                        "industrial" => 2,
                                        "ships" => 5,
                                        _ => 3,
                                    },
                                    device: point_device(frame, point),
                                });
                            }
                        }
                    }
                }
                row_base += batch.num_rows() as u64;
            }
        }
        ensure!(
            !has_surface_arrow || has_structures,
            "surface square {} requires an explicit structures.arrow, including when empty",
            grid::square_name(*square)
        );
    }
    sources.sort_by_key(|source| (source.layer, source.identity));
    ensure!(
        sources
            .windows(2)
            .all(|pair| (pair[0].layer, pair[0].identity) != (pair[1].layer, pair[1].identity)),
        "duplicate immutable source identity"
    );
    for source in &sources {
        ensure!(
            source.device.fits_the_profile_cadence(),
            "source {:?} exceeds GPU profile support",
            source.identity
        );
        ensure!(
            source
                .device
                .emission_linear
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0),
            "non-finite source emission"
        );
    }
    Ok((sources, ObstacleSet { indexes }))
}

fn required_i32(batch: &RecordBatch, name: &str, row: usize) -> Result<i32> {
    col_i32(batch, name)
        .filter(|column| !column.is_null(row))
        .map(|column| column.value(row))
        .with_context(|| format!("missing native coordinate {name}"))
}
fn position(batch: &RecordBatch, row: usize, prefix: &str) -> Result<[f64; 2]> {
    let (lon, lat) = grid_cell_lonlat(
        required_i32(batch, &format!("{prefix}_gx"), row)?,
        required_i32(batch, &format!("{prefix}_gy"), row)?,
    );
    Ok([lat, lon])
}
fn byte(batch: &RecordBatch, name: &str, row: usize) -> u8 {
    col_u8(batch, name)
        .filter(|c| !c.is_null(row))
        .map_or(0, |c| c.value(row))
}
fn short(batch: &RecordBatch, name: &str, row: usize) -> u16 {
    col_u16(batch, name)
        .filter(|c| !c.is_null(row))
        .map_or(0, |c| c.value(row))
}
fn boolean(batch: &RecordBatch, name: &str, row: usize) -> bool {
    col_bool(batch, name)
        .filter(|c| !c.is_null(row))
        .is_some_and(|c| c.value(row))
}
fn float(batch: &RecordBatch, name: &str, row: usize) -> Option<f32> {
    col_f32(batch, name)
        .filter(|c| !c.is_null(row))
        .map(|c| c.value(row))
}
fn row_square_country_city(batch: &RecordBatch, row: usize) -> Result<SquareCountryCity> {
    ensure!(
        col_u16(batch, "country_iso").is_some(),
        "surface rows require baked country identity"
    );
    Ok(noise_compute::defaults::baked_square_country_city(
        short(batch, "country_iso", row),
        short(batch, "city_id", row),
        byte(batch, "continent", row),
    ))
}
fn emission_linear(periods: ([f32; 8], [f32; 8], [f32; 8])) -> [f32; 24] {
    let values = [periods.0, periods.1, periods.2];
    std::array::from_fn(|i| 10.0_f32.powf(values[i / 8][i % 8] / 10.0))
}
fn line(
    batch: &RecordBatch,
    row: usize,
    rail_traffic: Option<RailTraffic>,
    road_traffic: Option<RoadTraffic>,
    frame: &RegionMetricFrame,
) -> Result<Option<DeviceLineSource>> {
    let start = position(batch, row, "start")?;
    let end = position(batch, row, "end")?;
    let square_country_city = row_square_country_city(batch, row)?;
    let (emission, max_distance_m, source_height_m) = if let Some(traffic) = rail_traffic {
        if traffic.is_silent() || boolean(batch, "tunnel", row) {
            return Ok(None);
        }
        let norm = normalize_rail(RawRailInput {
            rail_type: byte(batch, "rail_type", row),
            maxspeed: short(batch, "maxspeed", row),
            highspeed: boolean(batch, "highspeed", row),
            traffic,
        });
        (
            norm.period_emissions(),
            norm.max_distance_m(),
            norm.source_height_m,
        )
    } else {
        let Some(norm) = normalize_road(
            RawRoadInput {
                road_class: byte(batch, "road_class", row),
                speed_limit: byte(batch, "speed_limit", row),
                speed_taper: byte(batch, "speed_taper", row),
                surface_type: byte(batch, "surface_type", row),
                traffic: road_traffic.context("road row without prepared traffic")?,
                tunnel: boolean(batch, "tunnel", row),
                junction: byte(batch, "junction", row),
                built_up: byte(batch, "built_up", row),
            },
            square_country_city,
        ) else {
            return Ok(None);
        };
        (
            norm.period_emissions(),
            norm.max_distance_m,
            norm.source_height_m,
        )
    };
    let [start_x_m, start_y_m] = frame.encode(start[0], start[1]);
    let [end_x_m, end_y_m] = frame.encode(end[0], end[1]);
    Ok(Some(DeviceLineSource {
        start_x_m,
        start_y_m,
        end_x_m,
        end_y_m,
        extent_m: float(batch, "length_m", row)
            .filter(|v| *v > 0.0)
            .unwrap_or_else(|| grid::geo::flat_dist(start[0], start[1], end[0], end[1]) as f32),
        max_distance_m: max_distance_m as f32,
        source_height_m: source_height_m as f32,
        flags: if boolean(batch, "bridge", row) {
            SOURCE_FLAG_BRIDGE
        } else {
            0
        },
        emission_linear: emission_linear(emission),
    }))
}
fn point_device(frame: &RegionMetricFrame, point: &PreparedPoint) -> DeviceLineSource {
    let [x, y] = frame.encode(point.lat, point.lon);
    DeviceLineSource {
        start_x_m: x,
        start_y_m: y,
        end_x_m: x,
        end_y_m: y,
        extent_m: point.exclusion_radius_m,
        max_distance_m: point.max_radius_m as f32,
        source_height_m: point.source_height_m,
        flags: SOURCE_FLAG_POINT,
        emission_linear: emission_linear((point.lw_day, point.lw_evening, point.lw_night)),
    }
}

#[cfg(test)]
mod completeness_tests {
    use super::*;
    #[test]
    fn native_road_direction_contract_rejects_invalid_data_and_matches_popup() {
        use arrow::array::{
            ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, UInt16Array,
            UInt8Array,
        };
        use arrow::datatypes::{Field, Schema};
        use std::sync::Arc;
        let batch = |direction: Option<ArrayRef>| {
            let mut columns: Vec<(&str, ArrayRef)> = vec![
                ("osm_id", Arc::new(Int64Array::from(vec![1, 2, 3]))),
                ("country_iso", Arc::new(UInt16Array::from(vec![0; 3]))),
                ("source_id", Arc::new(UInt16Array::from(vec![10; 3]))),
                (
                    "aadt_light",
                    Arc::new(Float64Array::from(vec![10_000.0; 3])),
                ),
                ("aadt_medium", Arc::new(Float64Array::from(vec![0.0; 3]))),
                ("aadt_heavy", Arc::new(Float64Array::from(vec![0.0; 3]))),
                ("aadt_moto", Arc::new(Float64Array::from(vec![0.0; 3]))),
                ("cross_section_aadt", Arc::new(Float64Array::from(vec![0.0; 3]))),
                (
                    "traffic_estimated",
                    Arc::new(UInt8Array::from(vec![1; 3])),
                ),
                ("road_class", Arc::new(UInt8Array::from(vec![2; 3]))),
                ("speed_limit", Arc::new(UInt8Array::from(vec![50; 3]))),
            ];
            for (name, value) in [
                ("start_gx", 1 << 29),
                ("start_gy", 1 << 29),
                ("end_gx", (1 << 29) + 100),
                ("end_gy", 1 << 29),
            ] {
                columns.push((name, Arc::new(Int32Array::from(vec![value; 3]))));
            }
            if let Some(column) = direction {
                columns.push(("oneway", column));
            }
            let fields = columns
                .iter()
                .map(|(name, array)| {
                    Field::new(*name, array.data_type().clone(), array.null_count() != 0)
                })
                .collect::<Vec<_>>();
            let schema = Schema::new(fields).with_metadata(std::collections::HashMap::from([(
                "road_traffic_contract".to_owned(),
                "1".to_owned(),
            )]));
            RecordBatch::try_new(
                Arc::new(schema),
                columns.into_iter().map(|(_, column)| column).collect(),
            )
            .unwrap()
        };
        let invalid: Vec<Option<ArrayRef>> = vec![
            None,
            Some(Arc::new(BooleanArray::from(vec![false; 3]))),
            Some(Arc::new(UInt16Array::from(vec![0, 1, 2]))),
            Some(Arc::new(UInt8Array::from(vec![Some(0), None, Some(2)]))),
            Some(Arc::new(UInt8Array::from(vec![0, 1, 3]))),
        ];
        for column in invalid {
            let invalid = batch(column);
            assert!(RoadDirections::read(&invalid).is_err());
            assert!(source_reader::query_roads_from_batches(&[invalid], 0.0, 0.0, 1000.0).is_err());
        }
        let batch = batch(Some(Arc::new(UInt8Array::from(vec![0, 1, 2]))));
        RoadDirections::read(&batch).unwrap();
        let traffic = source_reader::road_traffic::RoadTrafficColumns::read(&batch).unwrap();
        let popup =
            source_reader::query_roads_from_batches(std::slice::from_ref(&batch), 0.0, 0.0, 1000.0)
                .unwrap();
        assert_eq!(
            popup.iter().map(|row| row.oneway).collect::<Vec<_>>(),
            [false, true, true]
        );
        let frame = RegionMetricFrame::for_latitude_longitude(0.0, 0.0);
        let devices: Vec<_> = (0..3)
            .map(|row| {
                line(&batch, row, None, Some(traffic.row(row)), &frame)
                    .unwrap()
                    .unwrap()
            })
            .collect();
        // Direction is strict identity, never a count factor: a published
        // directional 10 000 stays 10 000 on two-way, forward and reverse rows.
        assert_eq!(devices[0].emission_linear, devices[1].emission_linear);
        assert_eq!(devices[1].emission_linear, devices[2].emission_linear);
        assert!(devices[0].emission_linear.iter().all(|value| *value > 0.0));
        // The legacy build-stage Int32 traffic column is rejected by the same
        // shared reader the popup uses.
        let mut fields = batch.schema().fields().to_vec();
        let position = fields
            .iter()
            .position(|field| field.name() == "aadt_light")
            .unwrap();
        fields[position] = Arc::new(Field::new(
            "aadt_light",
            arrow::datatypes::DataType::Int32,
            false,
        ));
        let mut columns = batch.columns().to_vec();
        columns[position] = Arc::new(Int32Array::from(vec![10_000; 3]));
        let legacy = RecordBatch::try_new(
            Arc::new(
                Schema::new(fields).with_metadata(batch.schema().metadata().clone()),
            ),
            columns,
        )
        .unwrap();
        assert!(source_reader::road_traffic::RoadTrafficColumns::read(&legacy).is_err());
    }

    #[test]
    fn fractional_rail_periods_match_popup_and_device_emissions() {
        use arrow::array::{
            ArrayRef, Float64Array, Int16Array, Int32Array, Int64Array, UInt16Array, UInt8Array,
        };
        use arrow::datatypes::{Field, Schema};
        use std::sync::Arc;
        let mut columns: Vec<(String, ArrayRef)> = Vec::new();
        for (name, value) in [
            ("start_gx", 1 << 29),
            ("start_gy", 1 << 29),
            ("end_gx", (1 << 29) + 100),
            ("end_gy", 1 << 29),
        ] {
            columns.push((name.to_owned(), Arc::new(Int32Array::from(vec![value]))));
        }
        columns.push(("osm_id".to_owned(), Arc::new(Int64Array::from(vec![7]))));
        columns.push((
            "segment_idx".to_owned(),
            Arc::new(Int16Array::from(vec![0])),
        ));
        columns.push((
            "maxspeed".to_owned(),
            Arc::new(UInt16Array::from(vec![120])),
        ));
        for name in ["rail_type", "continent"] {
            columns.push((name.to_owned(), Arc::new(UInt8Array::from(vec![0]))));
        }
        for name in ["country_iso", "city_id"] {
            columns.push((name.to_owned(), Arc::new(UInt16Array::from(vec![0]))));
        }
        for (category, values) in [
            ("passenger", [0.0, 0.125, 0.0]),
            ("freight", [0.0, 0.0, 0.25]),
        ] {
            for (period, count) in ["day", "evening", "night"].into_iter().zip(values) {
                columns.push((
                    format!("trains_{category}_{period}"),
                    Arc::new(Float64Array::from(vec![count])),
                ));
            }
            columns.push((
                format!("{category}_status"),
                Arc::new(UInt8Array::from(vec![2])),
            ));
            columns.push((
                format!("{category}_source_id"),
                Arc::new(UInt16Array::from(vec![7])),
            ));
            columns.push((
                format!("{category}_matching"),
                Arc::new(UInt8Array::from(vec![1])),
            ));
        }
        let schema = Schema::new(
            columns
                .iter()
                .map(|(name, array)| Field::new(name, array.data_type().clone(), false))
                .collect::<Vec<_>>(),
        )
        .with_metadata(std::collections::HashMap::from([(
            "rail_traffic_contract".to_owned(),
            "1".to_owned(),
        )]));
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            columns.into_iter().map(|(_, column)| column).collect(),
        )
        .unwrap();
        let popup = source_reader::query_railways_from_batches(
            std::slice::from_ref(&batch),
            0.0,
            0.0,
            1000.0,
        )
        .unwrap();
        assert_eq!(popup.len(), 1);
        let row = &popup[0];
        let normalized = normalize_rail(RawRailInput {
            rail_type: row.rail_type,
            maxspeed: row.maxspeed,
            highspeed: row.highspeed,
            traffic: row.traffic,
        });
        let columns = source_reader::rail_traffic::RailTrafficColumns::read(&batch).unwrap();
        let frame = RegionMetricFrame::for_latitude_longitude(0.0, 0.0);
        let device = line(&batch, 0, Some(columns.row(0)), None, &frame)
            .unwrap()
            .unwrap();
        assert_eq!(
            device.emission_linear,
            emission_linear(normalized.period_emissions())
        );
        assert!(device.emission_linear[..8]
            .iter()
            .all(|value| *value == 0.0));
        assert!(device.emission_linear[8..].iter().all(|value| *value > 0.0));
        assert_eq!(device.max_distance_m, normalized.max_distance_m() as f32);
        assert!(line(&batch, 0, Some(RailTraffic::default()), None, &frame)
            .unwrap()
            .is_none());
    }

    #[test]
    fn manifested_surface_without_finished_structures_cannot_paint_unscreened() {
        let temp = tempfile::tempdir().unwrap();
        let square = Square { x: 1, y: 2 };
        let relative = "z9/1/2/roads.arrow";
        let path = temp.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut writer = arrow::ipc::writer::FileWriter::try_new(
            std::fs::File::create(&path).unwrap(),
            &arrow::datatypes::Schema::new(vec![
                arrow::datatypes::Field::new(
                    "oneway",
                    arrow::datatypes::DataType::UInt8,
                    false,
                ),
                arrow::datatypes::Field::new(
                    "aadt_light",
                    arrow::datatypes::DataType::Float64,
                    false,
                ),
                arrow::datatypes::Field::new(
                    "aadt_medium",
                    arrow::datatypes::DataType::Float64,
                    false,
                ),
                arrow::datatypes::Field::new(
                    "aadt_heavy",
                    arrow::datatypes::DataType::Float64,
                    false,
                ),
                arrow::datatypes::Field::new(
                    "aadt_moto",
                    arrow::datatypes::DataType::Float64,
                    false,
                ),
                arrow::datatypes::Field::new(
                    "cross_section_aadt",
                    arrow::datatypes::DataType::Float64,
                    false,
                ),
                arrow::datatypes::Field::new(
                    "traffic_estimated",
                    arrow::datatypes::DataType::UInt8,
                    false,
                ),
            ])
            .with_metadata(std::collections::HashMap::from([(
                "road_traffic_contract".to_owned(),
                "1".to_owned(),
            )])),
        )
        .unwrap();
        writer.finish().unwrap();
        drop(writer);
        let manifest_path = temp.path().join("inputs.sqlite");
        let db = rusqlite::Connection::open(&manifest_path).unwrap();
        db.execute_batch(
            "CREATE TABLE input_files(relative_path TEXT PRIMARY KEY, sha256 BLOB NOT NULL)",
        )
        .unwrap();
        db.execute(
            "INSERT INTO input_files VALUES(?1,?2)",
            rusqlite::params![
                relative,
                crate::input_manifest::file_digest(&path)
                    .unwrap()
                    .as_slice()
            ],
        )
        .unwrap();
        drop(db);
        let manifest = InputManifest::open(
            &manifest_path,
            crate::input_manifest::file_digest(&manifest_path).unwrap(),
        )
        .unwrap();
        let result = load_sources(
            temp.path(),
            &manifest,
            &[square],
            &RegionMetricFrame::for_square(square),
        );
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("requires an explicit structures.arrow"));
        let absent = Square { x: 2, y: 2 };
        let (sources, obstacles) = load_sources(
            temp.path(),
            &manifest,
            &[absent],
            &RegionMetricFrame::for_square(absent),
        )
        .unwrap();
        assert!(sources.is_empty() && obstacles.indexes.is_empty());
    }
}
