//! Read-only prepared airborne GPU/CPU acceptance at distributed facade-aware tile receivers.
#[cfg(feature = "gpu")]
fn main() -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    use arrow::ipc::reader::FileReader;
    use grid::bounds::BoundedSquares;
    use noise_compute::{
        compute::aircraft_v6::airborne, emission::aircraft as air,
        propagation::obstacle_index::ObstacleSet,
    };
    use raster_reader::{tile_bbox::TileBbox, FusedGrid, RealRasters};
    use relevant_source_gpu::{
        airborne_field::AirborneScene,
        airborne_pack::ReceiverScreening,
        input_manifest::{parse_digest, InputManifest},
        source_frame::RegionMetricFrame,
        surface_scene::SurfaceScene,
        tile_receivers::TileReceivers,
    };
    use source_reader::aircraft_v6::AirborneRowAccum;
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
    let bbox = TileBbox::from_xyz(13, tile_x, tile_y);
    let dy = air::meters_to_lat_deg(air::AIRBORNE_QUERY_RADIUS_M);
    let dx = air::meters_to_lon_deg(
        bbox.south_lat.abs().max(bbox.north_lat.abs()),
        air::AIRBORNE_QUERY_RADIUS_M,
    );
    let squares: Vec<_> = BoundedSquares::from_degrees(
        bbox.south_lat - dy,
        bbox.west_lon - dx,
        bbox.north_lat + dy,
        bbox.east_lon + dx,
    )
    .context("tile support")?
    .iter()
    .collect();
    let mut indexes = Vec::new();
    let mut exact = Vec::new();
    let mut days = 0;
    let mut weights = air::ClassWeights::uniform();
    let started = Instant::now();
    for square in &squares {
        let prefix = grid::square_name(*square);
        if let Some((bytes, _)) =
            manifest.read_arrow(root, &format!("{prefix}/structures.arrow"))?
        {
            indexes.push(
                source_reader::square_obstacle_index::load_square_obstacle_index(
                    &root.join(&prefix),
                    Some(source_reader::square_obstacle_index::structures_fingerprint(&bytes)),
                )
                .map_err(anyhow::Error::msg)?
                .context("structure index missing")?,
            );
        }
        if let Some((bytes, _)) = manifest.read_arrow(root, &format!("{prefix}/airborne.arrow"))? {
            let reader = FileReader::try_new(Cursor::new(bytes), None)?;
            let schema = reader.schema();
            days = schema.metadata().get("n_days").context("days")?.parse()?;
            weights = air::ClassWeights::parse(
                schema
                    .metadata()
                    .get(air::SAMPLE_DAYS_BY_CLASS_KEY)
                    .map(String::as_str),
                days,
            )
            .map_err(anyhow::Error::msg)?;
            for batch in reader {
                exact.push(batch?);
            }
        }
    }
    let scene = SurfaceScene {
        owner,
        frame: RegionMetricFrame::for_square(owner),
        sources: vec![],
        obstacles: ObstacleSet { indexes },
        raster: FusedGrid::build(
            &rasters,
            bbox.south_lat - 0.002,
            bbox.north_lat + 0.002,
            bbox.west_lon - 0.002,
            bbox.east_lon + 0.002,
        ),
    };
    let mut receivers = TileReceivers::prepare(&scene, tile_x, tile_y)?;
    let mut selected = Vec::new();
    for y in [37usize, 141, 253, 371, 479] {
        for x in [29usize, 137, 269, 389, 491] {
            selected.push(y * 512 + x);
        }
    }
    // Add actual indoor pixel centres, whose shared receiver preparation moves to real facades.
    selected.extend(
        receivers
            .indoor_attenuation
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0.0)
            .step_by(97)
            .take(8)
            .map(|(i, _)| i),
    );
    selected.sort_unstable();
    selected.dedup();
    let moved = selected
        .iter()
        .filter(|&&i| receivers.indoor_attenuation[i] > 0.0)
        .count();
    for values in [
        &mut receivers.x,
        &mut receivers.y,
        &mut receivers.altitude,
        &mut receivers.reflection,
        &mut receivers.floor,
        &mut receivers.indoor_attenuation,
    ] {
        *values = selected.iter().map(|&i| values[i]).collect();
    }
    let prepared_seconds = started.elapsed().as_secs_f64();
    let t = Instant::now();
    let airborne = AirborneScene::load(owner, root, &manifest, &rasters)?;
    let load_seconds = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let gpu = airborne.period_powers(&scene, &receivers)?;
    let field_seconds = t.elapsed().as_secs_f64();
    let rows = AirborneRowAccum::new(&exact).map_err(anyhow::Error::msg)?;
    let t = Instant::now();
    let mut max_db = 0.0f64;
    let mut sum_db = 0.0;
    let mut compared = 0;
    let mut failures = 0;
    let mut screened = 0;
    let mut chord_positive = 0;
    for i in 0..receivers.x.len() {
        let [lat, lon] = scene.frame.decode(receivers.x[i], receivers.y[i]);
        let rx =
            ReceiverScreening::build(lat, lon, receivers.altitude[i], &rasters, &scene.obstacles)?;
        screened += usize::from(!rx.buildings.is_empty());
        let mut flights: Vec<_> = airborne::scatter(
            &rx.receiver,
            rows.views(),
            f64::from(days),
            &weights,
            &rx.terrain,
            Some(&rx.buildings),
            0,
            None,
        )
        .into_iter()
        .collect();
        flights.sort_unstable_by_key(|(id, _)| *id);
        let mut reference = [0.0; 3];
        for (_, flight) in flights {
            for (p, value) in reference.iter_mut().enumerate() {
                *value += flight.period_energy[p] / (f64::from(days) * air::PERIOD_SECONDS[p]);
            }
        }
        for p in 0..3 {
            let observed = f64::from(gpu[i * 3 + p]);
            let delta = if observed == 0.0 && reference[p] == 0.0 {
                0.0
            } else {
                (10.0 * (observed / reference[p]).log10()).abs()
            };
            if !delta.is_finite() || delta > 0.5 {
                failures += 1;
            }
            max_db = max_db.max(delta);
            sum_db += delta;
            compared += 1;
        }
        chord_positive += usize::from(reference.iter().any(|v| *v > 0.0));
    }
    println!("{{\"receivers\":{},\"facade_moved\":{moved},\"building_horizons\":{screened},\"positive_receivers\":{chord_positive},\"independent_rows\":{},\"split_rows\":{},\"preparation_seconds\":{prepared_seconds},\"load_seconds\":{load_seconds},\"field_seconds\":{field_seconds},\"reference_seconds\":{},\"compared_periods\":{compared},\"max_db_error\":{max_db},\"mean_db_error\":{},\"failures\":{failures}}}", receivers.x.len(),airborne.row_counts().0,airborne.row_counts().1,t.elapsed().as_secs_f64(),sum_db/compared as f64);
    ensure!(
        moved > 0 && screened > 0,
        "fixture must exercise real facade moves and building horizons"
    );
    ensure!(failures == 0, "airborne GPU/CPU mismatch");
    Ok(())
}
#[cfg(not(feature = "gpu"))]
fn main() {
    eprintln!("airborne_fixture requires --features gpu");
    std::process::exit(1);
}
