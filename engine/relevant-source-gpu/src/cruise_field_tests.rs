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
/// Baseline 12 days and increment 4: a secondary-only bucket weighs 3.
fn weights() -> aircraft::ProvenanceWeights {
    aircraft::SamplingWindow {
        baseline_days: 12,
        increment_days: 4,
        baseline_days_sha256: "baseline".into(),
        increment_days_sha256: "increment".into(),
    }
    .provenance_weights()
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
            secondary_only: p == 2,
            unique_count: 1,
            top_candidates: &[],
        })
        .collect()
}
fn groups(rows: &[CruiseRowView<'_>], rasters: &dyn RasterSampler) -> Vec<Group> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let (segment, density) = cruise_segment(row, i, &weights()).unwrap();
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
        &weights(),
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
            (
                "schema_version".into(),
                square_store::aircraft_contract::SCHEMA_VERSION.into(),
            ),
            ("cruise_contract".into(), "cruise_v17".into()),
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

/// The cruise CUDA unit against the canonical CPU kernel: every heading bin,
/// reach and floor boundaries, dateline wrap, and the reduction part/batch seams.
#[cfg(feature = "gpu")]
mod gpu_parity {
    use super::*;
    use noise_compute::propagation::iso9613::fast_exp_f64;

    fn reference_energy(bucket: &Bucket, lat: f64, lon: f64, altitude: f64) -> f64 {
        let north = (bucket.lat - lat) * aircraft::M_PER_DEG_LAT;
        let east =
            grid::geo::wrapped_longitude_delta(lon, bucket.lon)
                * grid::geo::m_per_deg_lon(lat.to_radians());
        if north * north + east * east
            > (aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M + bucket.half_length).powi(2)
        {
            return 0.0;
        }
        let row = aircraft::prepare_row(
            &bucket.prepared,
            lat,
            aircraft::M_PER_DEG_LAT * lat.to_radians().cos().max(0.2),
        );
        aircraft::segment_sel_at_pixel_energy(
            &bucket.prepared,
            &row,
            lon,
            altitude,
            aircraft::NpdLuts::shared(),
            None,
        )
        .map_or(0.0, |sel| {
            fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1) * bucket.density
        })
    }

    fn db_error(observed: f64, wanted: f64) -> f64 {
        if observed == wanted {
            0.0
        } else {
            (10.0 * (observed / wanted).log10()).abs()
        }
    }

    fn view(
        lat: f64,
        lon: f64,
        altitude: f32,
        length: f32,
        heading_bin: u8,
        period: u8,
    ) -> CruiseRowView<'static> {
        CruiseRowView {
            lat,
            lon: grid::geo::normalize_longitude(lon),
            class: 0,
            rep_profile_idx: 0,
            fl_bin: 0,
            period,
            sum_length_m: length,
            heading_bin,
            rep_alt_m: altitude,
            rep_speed_kt: 450.0,
            source_id: heading_bin + 8 * period,
            origin: 0,
            secondary_only: false,
            unique_count: 1,
            top_candidates: &[],
        }
    }

    fn buckets_of(views: &[CruiseRowView<'_>], rasters: &dyn RasterSampler) -> Vec<Bucket> {
        groups(views, rasters)
            .into_iter()
            .flat_map(|group| group.buckets)
            .collect()
    }

    #[test]
    fn layouts_match_the_cuda_static_asserts() {
        assert_eq!(std::mem::size_of::<gpu::DeviceCruiseSource>(), 152);
        assert_eq!(std::mem::size_of::<gpu::DeviceCruiseReceiver>(), 40);
    }

    #[test]
    fn single_buckets_match_the_canonical_kernel_across_regimes() {
        let terrain = Terrain {
            ground: 1200.0,
            latitude: 52.0,
        };
        let mut views = Vec::new();
        for heading in 0..8u8 {
            for &(altitude, length) in
                &[(9_500.0f32, 800.0f32), (11_000.0, 3_000.0), (600.0, 50.0)]
            {
                views.push(view(
                    52.0 + f64::from(heading) * 0.002 - 0.007,
                    14.0,
                    altitude,
                    length,
                    heading,
                    heading % 3,
                ));
                views.push(view(
                    0.0,
                    179.999,
                    altitude,
                    length,
                    heading,
                    heading % 3,
                ));
            }
        }
        let grouped = groups(&views, &terrain);
        assert_eq!(grouped.len(), 48);
        let reach_edge = |bucket: &Bucket, metres: f64| {
            (aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M + bucket.half_length + metres)
                / aircraft::M_PER_DEG_LAT
        };
        let mut max_error = 0.0f64;
        for (view, group) in views.iter().zip(&grouped) {
            let bucket = &group.buckets[0];
            let edge = reach_edge(bucket, 0.0);
            let points = [
                [bucket.lat, bucket.lon],
                [bucket.lat - 0.08, bucket.lon + 0.08],
                [bucket.lat - edge - 50.0 / aircraft::M_PER_DEG_LAT, bucket.lon],
                [bucket.lat - edge + 50.0 / aircraft::M_PER_DEG_LAT, bucket.lon],
                if bucket.lon > 90.0 {
                    [-0.001, -179.999]
                } else {
                    [bucket.lat - 0.02, bucket.lon - 0.11]
                },
            ];
            let altitudes = vec![
                terrain.elevation(points[0][0], points[0][1]) + DEFAULT_RECEIVER_HEIGHT;
                points.len()
            ];
            let gpu = gpu::evaluate(std::slice::from_ref(&bucket), &points, &altitudes).unwrap();
            for (i, &[lat, lon]) in points.iter().enumerate() {
                let wanted = reference_energy(bucket, lat, lon, altitudes[i]);
                let observed = gpu[i][bucket.period];
                assert!(
                    observed == 0.0 || wanted > 0.0,
                    "CUDA kept a source the CPU kernel rejected at ({lat},{lon})"
                );
                if wanted > 0.0 {
                    let error = db_error(observed, wanted);
                    assert!(
                        error < 1e-9,
                        "heading={} altitude={} receiver=({lat},{lon}) error={error}dB \
                         gpu={observed:e} cpu={wanted:e}",
                        view.heading_bin,
                        view.rep_alt_m
                    );
                    max_error = max_error.max(error);
                }
            }
        }
        eprintln!("cruise_gpu parity buckets=48 max_abs_error_db={max_error:.12}");
    }

    #[test]
    fn reduction_spans_two_parts_and_receiver_batches() {
        let terrain = Terrain {
            ground: 0.0,
            latitude: 51.0,
        };
        let mut views = Vec::new();
        for i in 0..8193u16 {
            views.push(view(
                51.0 + f64::from(i % 64) * 0.001 - 0.032,
                14.0 + f64::from(i / 64) * 0.001 - 0.032,
                10_500.0,
                50.0 + f32::from(i % 7) * 111.0,
                (i % 8) as u8,
                (i % 3) as u8,
            ));
        }
        let buckets = buckets_of(&views, &terrain);
        assert_eq!(buckets.len(), 8193);
        let selected: Vec<&Bucket> = buckets.iter().collect();
        let points: Vec<_> = (0..300)
            .map(|i| {
                [
                    51.0 + f64::from(i % 20) * 0.003 - 0.028,
                    14.0 + f64::from(i / 20) * 0.014 - 0.028,
                ]
            })
            .collect();
        let altitudes: Vec<_> = points
            .iter()
            .map(|&[lat, lon]| terrain.elevation(lat, lon) + DEFAULT_RECEIVER_HEIGHT)
            .collect();
        let gpu = gpu::evaluate(&selected, &points, &altitudes).unwrap();
        let reference: Vec<_> = points
            .par_iter()
            .zip(&altitudes)
            .map(|(&[lat, lon], &altitude)| {
                let mut sums = [0.0f64; 3];
                for bucket in &buckets {
                    sums[bucket.period] += reference_energy(bucket, lat, lon, altitude);
                }
                sums
            })
            .collect();
        let mut max_error = 0.0f64;
        for (i, wanted) in reference.iter().enumerate() {
            for period in 0..3 {
                if wanted[period] > 0.0 {
                    max_error = max_error.max(db_error(gpu[i][period], wanted[period]));
                }
            }
        }
        assert!(max_error < 1e-9, "reduction seam error={max_error}dB");
        eprintln!("cruise_gpu reduction 8193 buckets x 300 receivers max_abs_error_db={max_error:.12}");
    }
}
