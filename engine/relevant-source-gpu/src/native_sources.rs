//! Native z30 Arrow source decoding, shared normalization and immutable source identities.
use crate::{input_manifest::InputManifest, source_frame::*};
use anyhow::{ensure, Context, Result};
use arrow::{array::Array, ipc::reader::FileReader, record_batch::RecordBatch};
use grid::Square;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::{admin::Admin, normalize::*};
use square_store::grid_cols::*;
use std::{io::Cursor, path::Path, sync::Arc};
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
        ] {
            let relative = format!("z9/{}/{}/{name}.arrow", square.x, square.y);
            let Some((bytes, digest)) = manifest.read_arrow(root, &relative)? else {
                continue;
            };
            has_surface_arrow = true;
            if name == "structures" {
                has_structures = true;
                let index = source_reader::structure_store::build_obstacle_index_from_arrow_bytes(
                    *square,
                    &bytes,
                    Path::new(&relative),
                )
                .map_err(anyhow::Error::msg)?;
                indexes.push(Arc::new(index));
            }
            let reader = FileReader::try_new(Cursor::new(bytes), None)?;
            if name == "structures" {
                square_store::structure_contract::validate_schema(&reader.schema())
                    .map_err(anyhow::Error::msg)?;
            }
            let traffic_calendar = (name == "airport_traffic")
                .then(|| traffic::TrafficCalendar::read(&RecordBatch::new_empty(reader.schema())))
                .transpose()?;
            let mut row_base = 0_u64;
            for batch in reader {
                let batch = batch?;
                for row in 0..batch.num_rows() {
                    let identity =
                        |part| SourceIdentity::arrow_row(digest, row_base + row as u64, part);
                    match name {
                        "roads" | "railways" => {
                            if let Some(device) = line(&batch, row, name == "railways", frame)? {
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
                                    layer: if name == "industrial" { 2 } else { 3 },
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
fn integer(batch: &RecordBatch, name: &str, row: usize) -> i32 {
    col_i32(batch, name)
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
fn row_admin(batch: &RecordBatch, row: usize) -> Result<Admin> {
    ensure!(
        col_u16(batch, "country_iso").is_some(),
        "surface rows require baked country identity"
    );
    Ok(noise_compute::defaults::baked_admin(
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
    rail: bool,
    frame: &RegionMetricFrame,
) -> Result<Option<DeviceLineSource>> {
    let start = position(batch, row, "start")?;
    let end = position(batch, row, "end")?;
    let admin = row_admin(batch, row)?;
    let (emission, max_distance_m, source_height_m) = if rail {
        if boolean(batch, "tunnel", row) {
            return Ok(None);
        }
        let norm = normalize_rail(
            RawRailInput {
                rail_type: byte(batch, "rail_type", row),
                usage: byte(batch, "usage", row),
                maxspeed: short(batch, "maxspeed", row),
                service: byte(batch, "service", row),
                highspeed: boolean(batch, "highspeed", row),
                trains_passenger: integer(batch, "trains_passenger", row),
                trains_freight: integer(batch, "trains_freight", row),
                parallel_divisor: byte(batch, "parallel_divisor", row),
            },
            admin,
        );
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
                oneway: boolean(batch, "oneway", row),
                lanes: byte(batch, "lanes", row),
                aadt_light: integer(batch, "aadt_light", row),
                aadt_medium: integer(batch, "aadt_medium", row),
                aadt_heavy: integer(batch, "aadt_heavy", row),
                aadt_moto: integer(batch, "aadt_moto", row),
                provenance: noise_compute::sources::provenance_of(short(batch, "source_id", row)),
                tunnel: boolean(batch, "tunnel", row),
                access: byte(batch, "access", row),
                junction: byte(batch, "junction", row),
                built_up: byte(batch, "built_up", row),
            },
            admin,
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
    fn manifested_surface_without_finished_structures_cannot_paint_unscreened() {
        let temp = tempfile::tempdir().unwrap();
        let square = Square { x: 1, y: 2 };
        let relative = "z9/1/2/roads.arrow";
        let path = temp.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut writer = arrow::ipc::writer::FileWriter::try_new(
            std::fs::File::create(&path).unwrap(),
            &arrow::datatypes::Schema::empty(),
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
