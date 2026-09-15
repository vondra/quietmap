//! Cruise field parity across period normalization, terrain heights and shared owner boundaries.
use super::*;
use noise_compute::compute::aircraft_v6::{cruise, CruiseRowView};
use std::collections::HashMap;

struct Terrain {
    ground: f64,
    latitude: f64,
}
impl RasterSampler for Terrain {
    fn elevation(&self, lat: f64, _: f64) -> f64 {
        self.ground + 300.0 * ((lat - self.latitude) * 30.0).sin()
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}
fn rows(lat: f64, lon: f64, altitude: f32) -> Vec<CruiseRowView<'static>> {
    (0..3)
        .map(|p| CruiseRowView {
            lat: lat + (p as f64 - 1.0) * 0.009,
            lon: grid::geo::normalize_longitude(lon),
            class: 0,
            rep_profile_idx: 0,
            fl_bin: 0,
            period: p,
            sum_length_m: if p == 0 { 50.0 } else { 1000.0 },
            heading_bin: p * 3,
            rep_alt_m: altitude,
            rep_speed_kt: 450.0,
            source_id: 0,
            origin: 0,
            unique_count: 1,
            top_candidates: &[],
        })
        .collect()
}
fn groups(rows: &[CruiseRowView<'_>], rasters: &dyn RasterSampler) -> Vec<Group> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let (segment, density) = cruise_segment(row, i).unwrap();
            let terrain = SegmentTerrain::sample(&segment, rasters);
            Group {
                bounds: [row.lat, row.lon, row.lat, row.lon],
                half_length: f64::from(segment.segment_length_m) * 0.5,
                buckets: vec![Bucket {
                    prepared: aircraft::prepare_segment(
                        &segment,
                        terrain.start_elev - 30.0,
                        terrain.end_elev - 30.0,
                    ),
                    lat: row.lat,
                    lon: row.lon,
                    half_length: f64::from(segment.segment_length_m) * 0.5,
                    density,
                    period: row.period as usize,
                }],
            }
        })
        .collect()
}
fn exact(
    rows: &[CruiseRowView<'_>],
    rasters: &dyn RasterSampler,
    lat: f64,
    lon: f64,
    altitude: f32,
) -> [f64; 3] {
    let receiver = noise_compute::types::Receiver {
        lat,
        lon,
        elevation_m: f64::from(altitude) - DEFAULT_RECEIVER_HEIGHT,
        height_m: DEFAULT_RECEIVER_HEIGHT,
    };
    let mut flights = HashMap::new();
    cruise::scatter(
        &receiver,
        rows,
        rasters,
        12.0,
        &mut flights,
        &mut HashMap::new(),
        &mut HashMap::new(),
        None,
    );
    let mut sums = [0.0; 3];
    let mut ids: Vec<_> = flights.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        for (p, power) in sums.iter_mut().enumerate() {
            *power += flights[&id].period_energy[p] / (12.0 * aircraft::PERIOD_SECONDS[p]);
        }
    }
    sums
}

#[test]
fn field_preserves_fractional_density_periods_facade_heights_and_owner_seams() {
    let mut max_error = 0f64;
    for &(lat, lon) in &[(0.0, 179.999), (50.0, 14.0), (68.0, 20.0)] {
        for &ground in &[0.0, 2500.0, 5000.0] {
            let terrain = Terrain {
                ground,
                latitude: lat,
            };
            let rows = rows(lat, lon, 10_000.0);
            let owner = grid::square_of(lat, lon);
            let field = CruiseField::build(
                owner,
                Lattice::new([lat - 0.04, lon - 0.04, lat + 0.04, lon + 0.04]),
                groups(&rows, &terrain),
                12,
                &terrain,
            )
            .unwrap();
            let shifted = CruiseField::build(
                owner,
                Lattice::new([lat - 0.03, lon - 0.03, lat + 0.06, lon + 0.06]),
                groups(&rows, &terrain),
                12,
                &terrain,
            )
            .unwrap();
            let points: Vec<_> = (0..49)
                .map(|i| {
                    [
                        lat + (i / 7) as f64 * 0.004 - 0.012,
                        grid::geo::normalize_longitude(lon + (i % 7) as f64 * 0.004 - 0.012),
                    ]
                })
                .collect();
            let altitudes: Vec<_> = points
                .iter()
                .map(|&[la, lo]| (terrain.elevation(la, lo) + DEFAULT_RECEIVER_HEIGHT) as f32)
                .collect();
            let power = field.powers_at(&points, &altitudes).unwrap();
            let other = shifted.powers_at(&points, &altitudes).unwrap();
            assert_eq!(
                power, other,
                "the same facade must read the same field across owners"
            );
            for (i, &[la, lo]) in points.iter().enumerate() {
                let wanted = exact(&rows, &terrain, la, lo, altitudes[i]);
                for p in 0..3 {
                    assert!(wanted[p] > 0.0 && power[i * 3 + p] > 0.0);
                    let error = (10.0 * (f64::from(power[i * 3 + p]) / wanted[p]).log10()).abs();
                    max_error = max_error.max(error);

                    assert!(error<=0.5,"lat={lat} ground={ground} period={p} error={error}dB exceeds one HM3 quantum");
                    if ground >= 2500.0 {
                        assert!(error < 0.00001, "near-field replacement must be exact");
                    }
                }
            }
        }
    }
    eprintln!("cruise_field: 1323 period comparisons; max_abs_error_db={max_error:.9}");
}

#[test]
fn absent_sources_are_silent_but_unmanifested_or_wrong_contract_inputs_fail() {
    use arrow::datatypes::Schema;
    use sha2::{Digest, Sha256};
    use std::{fs, sync::Arc};
    let tmp = tempfile::tempdir().unwrap();
    let prepared = tmp.path().join("prepared");
    fs::create_dir_all(&prepared).unwrap();
    let database = tmp.path().join("inputs.sqlite");
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch(
        "CREATE TABLE input_files(relative_path TEXT PRIMARY KEY,sha256 BLOB NOT NULL)",
    )
    .unwrap();
    drop(db);
    let manifest = InputManifest::open(
        &database,
        crate::input_manifest::file_digest(&database).unwrap(),
    )
    .unwrap();
    let owner = grid::square_of(50.0, 14.0);
    let rasters = RealRasters::new(&tmp.path().join("rasters"));
    let field = CruiseField::load(owner, &prepared, &manifest, &rasters).unwrap();
    assert_eq!(
        field.powers_at(&[[50.0, 14.0]], &[304.0]).unwrap(),
        [0.0; 3]
    );
    let relative = format!("z9/{}/{}/cruise.arrow", owner.x, owner.y);
    let path = prepared.join(&relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let schema = Arc::new(Schema::new_with_metadata(
        Vec::<arrow::datatypes::Field>::new(),
        HashMap::from([
            ("schema_version".into(), "v15".into()),
            ("cruise_contract".into(), "cruise_v17".into()),
            ("n_days".into(), "12".into()),
        ]),
    ));
    let mut writer =
        arrow::ipc::writer::FileWriter::try_new(fs::File::create(&path).unwrap(), &schema).unwrap();
    writer.finish().unwrap();
    drop(writer);
    assert!(CruiseField::load(owner, &prepared, &manifest, &rasters)
        .err()
        .unwrap()
        .to_string()
        .contains("unmanifested"));
    drop(manifest);
    let db = rusqlite::Connection::open(&database).unwrap();
    let digest = Sha256::digest(fs::read(&path).unwrap()).to_vec();
    db.execute(
        "INSERT INTO input_files VALUES(?1,?2)",
        rusqlite::params![relative, digest],
    )
    .unwrap();
    drop(db);
    let manifest = InputManifest::open(
        &database,
        crate::input_manifest::file_digest(&database).unwrap(),
    )
    .unwrap();
    assert!(CruiseField::load(owner, &prepared, &manifest, &rasters)
        .err()
        .unwrap()
        .to_string()
        .contains("cruise_contract"));
}

#[test]
fn field_and_receiver_parallelism_keep_identical_power_bytes() {
    let terrain = Terrain {
        ground: 250.0,
        latitude: 50.0,
    };
    let mut input = Vec::new();
    for i in 0..256 {
        input.extend(rows(
            50.0 + (i / 16) as f64 * 0.001 - 0.008,
            14.0 + (i % 16) as f64 * 0.001 - 0.008,
            10000.0,
        ));
    }
    let points: Vec<_> = (0..512 * 512)
        .map(|i| {
            [
                49.985 + (i / 512) as f64 * 0.00005,
                13.985 + (i % 512) as f64 * 0.00005,
            ]
        })
        .collect();
    let altitudes: Vec<_> = points
        .iter()
        .map(|&[lat, lon]| (terrain.elevation(lat, lon) + DEFAULT_RECEIVER_HEIGHT) as f32)
        .collect();
    let mut reference = None;
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let result=pool.install(|| {
            let started=std::time::Instant::now();
            let field=CruiseField::build(grid::square_of(50.0,14.0),Lattice::new([49.98,13.98,50.02,14.02]),groups(&input,&terrain),12,&terrain).unwrap();
            let build_ms=started.elapsed().as_secs_f64()*1000.0;
            let node_count=field.lattice.width*field.lattice.height;
            let powers=field.powers_at(&points,&altitudes).unwrap();
            eprintln!("cruise_field_bench threads={threads} buckets={} nodes={node_count} receivers={} build_ms={build_ms:.3} total_ms={:.3}",input.len(),points.len(),started.elapsed().as_secs_f64()*1000.0);
            powers
        });
        if let Some(wanted) = &reference {
            assert_eq!(&result, wanted);
        } else {
            reference = Some(result);
        }
    }
}
