//! Read-only prepared cruise GPU acceptance: one owner build and tile receivers.
#[cfg(feature = "gpu")]
fn main() -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    use arrow::ipc::reader::FileReader;
    use noise_compute::{
        compute::aircraft_v6::cruise::cruise_segment,
        emission::aircraft::{self as air, SegmentTerrain},
        propagation::iso9613::fast_exp_f64,
    };
    use rayon::prelude::*;
    use raster_reader::{tile_bbox::TileBbox, FusedGrid, RealRasters};
    use relevant_source_gpu::{
        cruise_field::CruiseField,
        input_manifest::{parse_digest, InputManifest},
        source_frame::RegionMetricFrame,
        surface_scene::SurfaceScene,
        tile_receivers::TileReceivers,
    };
    use source_reader::aircraft_v6::CruiseRowAccum;
    use std::{io::Cursor, path::Path, time::Instant};

    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 6,
        "expected prepared manifest manifest_sha256 tile_x tile_y"
    );
    let root = Path::new(&args[1]);
    let manifest = InputManifest::open(Path::new(&args[2]), parse_digest(&args[3])?)?;
    let tile_x: u32 = args[4].parse()?;
    let tile_y: u32 = args[5].parse()?;
    let owner = grid::Square {
        x: (tile_x / 16).try_into()?,
        y: (tile_y / 16).try_into()?,
    };
    let rasters = RealRasters::new(root);
    let npd = air::NpdLuts::shared();

    // The manifest's cruise entries are the independently anchored support set;
    // the reference below decodes exactly the rows the field loads.
    let connection = rusqlite::Connection::open(Path::new(&args[2]))?;
    let mut statement = connection
        .prepare(
            "SELECT relative_path FROM input_files \
             WHERE relative_path LIKE 'z9/%/cruise.arrow' ORDER BY relative_path",
        )
        .context("manifest cruise enumeration")?;
    let relatives: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    drop(statement);
    drop(connection);

    struct Direct {
        prepared: air::SegmentPrepared,
        lat: f64,
        lon: f64,
        half_length: f64,
        density: f64,
        period: usize,
    }
    fn direct_energy(direct: &Direct, lat: f64, lon: f64, altitude: f64, npd: &air::NpdLuts) -> f64 {
        let north = (direct.lat - lat) * air::M_PER_DEG_LAT;
        let east =
            grid::geo::wrapped_longitude_delta(lon, direct.lon) * grid::geo::m_per_deg_lon(lat.to_radians());
        if north * north + east * east
            > (air::AIRCRAFT_MAX_HORIZONTAL_REACH_M + direct.half_length).powi(2)
        {
            return 0.0;
        }
        let row = air::prepare_row(
            &direct.prepared,
            lat,
            air::M_PER_DEG_LAT * lat.to_radians().cos().max(0.2),
        );
        air::segment_sel_at_pixel_energy(&direct.prepared, &row, lon, altitude, npd, None).map_or(
            0.0,
            |sel| fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1) * direct.density,
        )
    }

    let started = Instant::now();
    let mut directs: Vec<Direct> = Vec::new();
    let mut days: Option<u16> = None;
    for relative in &relatives {
        let Some((bytes, _)) = manifest.read_arrow(root, relative)? else {
            continue;
        };
        let reader = FileReader::try_new(Cursor::new(bytes), None)?;
        let file_days = reader
            .schema()
            .metadata()
            .get("n_days")
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|v| *v > 0)
            .context("cruise has no valid n_days")?;
        ensure!(
            days.is_none_or(|value| value == file_days),
            "mixed cruise normalization windows"
        );
        days = Some(file_days);
        for batch in reader {
            let decoded = CruiseRowAccum::new(&[batch?]).map_err(anyhow::Error::msg)?;
            let slices = decoded.views();
            let views = slices.as_row_views();
            for (index, row) in views.iter().enumerate() {
                let Some((segment, density)) = cruise_segment(row, index) else {
                    continue;
                };
                let terrain = SegmentTerrain::sample(&segment, &rasters);
                ensure!(
                    [
                        terrain.start_elev,
                        terrain.q1_elev,
                        terrain.mid_elev,
                        terrain.q3_elev,
                        terrain.end_elev
                    ]
                    .iter()
                    .all(|v| v.is_finite()),
                    "cruise source terrain is unavailable"
                );
                if !air::is_valid_airborne_with_terrain(&segment, &terrain) {
                    continue;
                }
                directs.push(Direct {
                    prepared: air::prepare_segment(
                        &segment,
                        terrain.start_elev - 30.0,
                        terrain.end_elev - 30.0,
                    ),
                    lat: row.lat,
                    lon: row.lon,
                    half_length: f64::from(segment.segment_length_m) * 0.5,
                    density,
                    period: usize::from(row.period.min(2)),
                });
            }
        }
    }
    let days = days.context("no cruise input")?;
    let reference_seconds = started.elapsed().as_secs_f64();

    let bbox = TileBbox::from_xyz(13, tile_x, tile_y);
    let scene = SurfaceScene {
        owner,
        frame: RegionMetricFrame::for_square(owner),
        sources: vec![],
        obstacles: noise_compute::propagation::obstacle_index::ObstacleSet { indexes: vec![] },
        raster: FusedGrid::build(
            &rasters,
            bbox.south_lat,
            bbox.north_lat,
            bbox.west_lon,
            bbox.east_lon,
        ),
    };
    let t = Instant::now();
    let field = CruiseField::load(owner, root, &manifest, &rasters)?;
    let load_seconds = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let receivers = TileReceivers::prepare(&scene, tile_x, tile_y)?.points;
    let preparation_seconds = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let powers = field.period_powers(&scene, &receivers)?;
    let field_seconds = t.elapsed().as_secs_f64();
    let sum_power: f64 = powers.iter().map(|v| f64::from(*v)).sum();

    let stride = (receivers.x.len() / 24).max(1);
    let samples: Vec<usize> = (0..receivers.x.len())
        .step_by(stride)
        .chain([receivers.x.len() - 1])
        .collect();
    let reference: Vec<[f64; 3]> = samples
        .par_iter()
        .map(|&i| {
            let [lat, lon] = scene.frame.decode(receivers.x[i], receivers.y[i]);
            let altitude = f64::from(receivers.altitude[i]);
            let mut sums = [0.0f64; 3];
            for direct in &directs {
                sums[direct.period] += direct_energy(direct, lat, lon, altitude, npd);
            }
            sums
        })
        .collect();
    let mut max_db = 0.0f64;
    let mut sum_db = 0.0;
    let mut compared = 0;
    let mut failures = 0;
    for (slot, &i) in samples.iter().enumerate() {
        for period in 0..3 {
            let observed = f64::from(powers[i * 3 + period]);
            let wanted = reference[slot][period] / (f64::from(days) * air::PERIOD_SECONDS[period]);
            let delta = if observed == wanted {
                0.0
            } else {
                (10.0 * (observed / wanted).log10()).abs()
            };
            if !delta.is_finite() || delta > 0.5 {
                failures += 1;
            }
            max_db = max_db.max(delta);
            sum_db += delta;
            compared += 1;
        }
    }
    println!(
        "{{\"cruise_owners\":{},\"reference_buckets\":{},\"days\":{days},\"load_seconds\":{load_seconds},\"reference_decode_seconds\":{reference_seconds},\"receiver_preparation_seconds\":{preparation_seconds},\"period_powers_seconds\":{field_seconds},\"values\":{},\"sum_power\":{sum_power},\"sample_receivers\":{},\"compared_periods\":{compared},\"max_db_error\":{max_db},\"mean_db_error\":{},\"failures\":{failures}}}",
        relatives.len(),
        directs.len(),
        powers.len(),
        samples.len(),
        sum_db / compared as f64
    );
    ensure!(
        directs.len() == 1_927_662,
        "the M25 reference must decode the measured bucket count, got {}",
        directs.len()
    );
    ensure!(failures == 0, "cruise GPU/direct mismatch");
    Ok(())
}
#[cfg(not(feature = "gpu"))]
fn main() {
    eprintln!("cruise_fixture requires --features gpu");
    std::process::exit(1);
}
