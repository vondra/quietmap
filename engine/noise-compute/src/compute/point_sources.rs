//! Point-source compute kernel (buildings + industrial) — groups discretized
//! grid points by osm_id and propagates each to the receiver.
use crate::*;

/// Snapped z30 ring → GeoJSON Polygon (lon/lat degrees). `None` when the
/// source carries no ring — the caller falls back to a Point.
fn grid_ring_to_geojson(ring: &[(i32, i32)]) -> Option<serde_json::Value> {
    if ring.len() < 3 {
        return None;
    }
    let mut coords: Vec<serde_json::Value> = ring
        .iter()
        .map(|&(gx, gy)| {
            let (x_m, y_m) = grid::grid_to_meters(gx, gy);
            let (lon, lat) = grid::poly::meters_to_lonlat(x_m, y_m);
            serde_json::json!([lon, lat])
        })
        .collect();
    // Close the ring when the encoder left it open.
    if coords.first() != coords.last() {
        let first = coords.first()?.clone();
        coords.push(first);
    }
    Some(serde_json::json!({"type": "Polygon", "coordinates": [coords]}))
}

/// Compute noise from pre-discretized point sources (buildings, industrial).
/// Grouped by osm_id with Point geometry for map highlight.
pub(crate) fn compute_point_sources(
    receiver: &Receiver,
    sources: &[PointSource],
    obstacles: &crate::propagation::obstacle_index::ObstacleSet,
    rasters: &dyn RasterSampler,
    source_kind: LayerKind,
    mut traces: Option<&mut TraceCollector>,
) -> (NoisePeriods, Vec<Contributor>) {
    use std::collections::HashMap;

    struct PtAccum {
        name: String,
        subtype: u8,
        lat: f64,
        lon: f64,
        min_dist: f64,
        min_d_slant: f64,
        min_ground_g: f64,
        /// The ray source of the nearest grid point (the popup's path context).
        closest_source: crate::propagation::ray_transfer::RaySource,
        variants: [PropagationVariants; 3],
        emission_energy: f64,
        polygon_grid: Vec<(i32, i32)>,
        /// First-touched PointSource's `floors` / `area_m2`. Each
        /// PointSource on the same osm_id is one grid point of the
        /// same building / industrial site, so they all share these
        /// values — we just keep the first.
        floors: u8,
        area_m2: f32,
        /// Number of PointSource grid points that fell into this
        /// osm_id. For large industrial sites this is the z30 grid
        /// discretisation count (driving the `Lw − 10·log10(N)`
        /// per-point split). 1 for buildings + small industrial.
        grid_point_count: u16,
        /// Dataset stamp of the first-touched PointSource (whole site
        /// shares one source_id) — resolved to `provenance` for the popup.
        source_id: u16,
        /// Ship cell hours by class (zero outside the ship layer).
        ship_hours: [f32; 3],
    }
    let mut pts_by_osm: HashMap<i64, PtAccum> = HashMap::new();
    let reflection = rasters.building_enclosure(receiver.lat, receiver.lon);
    let weather = crate::propagation::meteorology::Meteorology::defaults();
    let bound = crate::propagation::relevance_bound::surface_relevance_bound(&weather);
    let ray_receiver = RayReceiver {
        lat: receiver.lat,
        lon: receiver.lon,
        altitude_m: receiver.altitude_m(),
    };
    let mut ray_scratch = RayScratch::default();
    use crate::propagation::ray_transfer::{
        evaluate_ray_transfer, received_variants, RayReceiver, RayScratch, RaySource, SourceGround,
    };
    use crate::propagation::relevance_bound::SourceSpread;

    for src in sources {
        let max_d = src.max_radius_m.max(0.0);
        if src.dist_m > max_d {
            continue;
        }

        let src_alt = rasters.elevation(src.lat, src.lon) + src.source_height_m as f64;
        let rcv_alt = receiver.altitude_m();
        let prop_dist = geo::effective_area_source_dist(src.dist_m, src.exclusion_radius_m as f64);
        let d_slant = geo::slant_dist(prop_dist, src_alt, rcv_alt).max(1.0);

        // All periods count (#31): a night-only source is never dropped by a day gate.
        let period_emissions = [src.lw_day, src.lw_evening, src.lw_night].map(|bands| bands.map(f64::from));
        if bound.pair_is_inaudible(&period_emissions, SourceSpread::Point, src.dist_m) {
            continue;
        }

        let mut detail = None;
        let ray_source = RaySource {
            lat: src.lat,
            lon: src.lon,
            height_m: f64::from(src.source_height_m),
            ground: SourceGround::UnderSource,
            platform_half_width_m: 0.0,
            exclusion_radius_m: f64::from(src.exclusion_radius_m),
        };
        let transfer = evaluate_ray_transfer(
            &ray_receiver,
            &ray_source,
            obstacles,
            true,
            rasters,
            &weather,
            true,
            &mut ray_scratch,
            traces.is_some().then_some(&mut detail),
        );
        // Spherical divergence (2.5.12) at the footprint-floored slant distance.
        let divergence = 10f64.powf(-(20.0 * d_slant.log10() + 11.0) / 10.0);
        let [v_day, v_eve, v_night] = [0, 1, 2].map(|period| {
            let scaled = transfer.periods[period].map(|bands| bands.map(|t| t * divergence));
            received_variants(&scaled, &period_emissions[period], reflection)
        });
        let ground_g = detail.as_ref().map_or(0.5, |d: &crate::propagation::ray_transfer::RayDetail| d.ground_factor);

        // Display aggregate is A-weighted so the popup's emission_db equals the
        // nominal LwA (post-C7 the bands are normalized to it; a Z-sum would
        // read ~+2 dB over the rated value — Codex C7 review).
        let day_em: f64 = src
            .lw_day
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let a = v as f64 + crate::constants::A_WEIGHTING[i];
                crate::propagation::iso9613::fast_exp_f64(a * std::f64::consts::LN_10 * 0.1)
            })
            .sum();

        let acc = pts_by_osm.entry(src.osm_id).or_insert_with(|| PtAccum {
            name: src.name.clone(),
            subtype: src.source_type,
            lat: src.lat,
            lon: src.lon,
            min_dist: f64::MAX,
            min_d_slant: 0.0,
            min_ground_g: 0.5,
            closest_source: ray_source,
            variants: [
                PropagationVariants::default(),
                PropagationVariants::default(),
                PropagationVariants::default(),
            ],
            emission_energy: 0.0,
            polygon_grid: src.polygon_grid.clone(),
            floors: src.floors,
            area_m2: src.area_m2,
            grid_point_count: 0,
            source_id: src.source_id,
            ship_hours: src.ship_hours.unwrap_or([0.0; 3]),
        });
        acc.variants[0].add(&v_day);
        acc.variants[1].add(&v_eve);
        acc.variants[2].add(&v_night);
        acc.emission_energy += day_em;
        acc.grid_point_count = acc.grid_point_count.saturating_add(1);
        if src.dist_m < acc.min_dist {
            acc.min_dist = src.dist_m;
            acc.min_d_slant = d_slant;
            acc.min_ground_g = ground_g;
            acc.lat = src.lat;
            acc.lon = src.lon;
            acc.closest_source = ray_source;
        }

        if let (Some(t), Some(node)) = (traces.as_deref_mut(), detail) {
            let seg_variants = [v_day, v_eve, v_night];
            let lw_bands: [[f64; NUM_BANDS]; 3] = [
                std::array::from_fn(|i| src.lw_day[i] as f64),
                std::array::from_fn(|i| src.lw_evening[i] as f64),
                std::array::from_fn(|i| src.lw_night[i] as f64),
            ];
            let trace = build_point_segment_trace(BuildPointTrace {
                src,
                source_kind,
                rcv_alt,
                d_slant,
                prop_dist,
                reflection_boost_db: reflection,
                node,
                seg_variants,
                lw_bands,
            });
            t.segments.push(trace);
        }
    }

    let mut contributors = Vec::new();
    // Ascending osm_id, not HashMap order — see `crate::compute::key_sorted`.
    for (osm_id, acc) in crate::compute::key_sorted(&pts_by_osm) {
        let ld = PropagationVariants::to_db(acc.variants[0].full_energy);
        let le = PropagationVariants::to_db(acc.variants[1].full_energy);
        let ln = PropagationVariants::to_db(acc.variants[2].full_energy);
        let pt_periods = periods::periods(ld, le, ln);

        let ld_free = PropagationVariants::to_db(acc.variants[0].free_field_energy);
        let le_free = PropagationVariants::to_db(acc.variants[1].free_field_energy);
        let ln_free = PropagationVariants::to_db(acc.variants[2].free_field_energy);
        let free_periods = periods::periods(ld_free, le_free, ln_free);

        let geometry = grid_ring_to_geojson(&acc.polygon_grid).or(Some(serde_json::json!({
            "type": "Point", "coordinates": [acc.lon, acc.lat],
        })));

        let pt_effects = nearest_path_breakdown(rasters, obstacles, &acc.closest_source, receiver, &weather);

        let impacts = PropagationVariants::impact_deltas(&acc.variants, pt_periods.lden_db);

        let subtype_name: &'static str = match source_kind {
            LayerKind::Industrial => industrial_type_name(acc.subtype),
            LayerKind::Ship => ship_type_name(acc.subtype),
            _ => building_type_name(acc.subtype),
        };

        // Build per-source metadata (popup only). `floors` / `area_m2` /
        // `grid_point_count` come from the first PointSource hit on this
        // osm_id (all grid points of the same site share these values).
        let metadata = if source_kind == LayerKind::Industrial {
            Some(SourceMetadata::Industrial(IndustrialMetadata {
                area_m2: acc.area_m2 as f64,
                source_type: subtype_name,
                nace: None,
                grid_point_count: acc.grid_point_count,
                source_id: acc.source_id,
                provenance: crate::sources::dataset_meta(acc.source_id),
            }))
        } else if source_kind == LayerKind::Ship {
            Some(SourceMetadata::Ship(ShipMetadata {
                area_m2: acc.area_m2 as f64,
                source_type: subtype_name,
                hours_per_month: acc.ship_hours,
                source_id: acc.source_id,
                provenance: crate::sources::dataset_meta(acc.source_id),
            }))
        } else {
            Some(SourceMetadata::Building(BuildingMetadata {
                height_m: crate::emission::settlement::building_height_from_source(
                    acc.closest_source.height_m, acc.floors,
                ),
                floors: acc.floors,
                area_m2: acc.area_m2 as f64,
                building_type: subtype_name,
                address: acc.name.clone(),
            }))
        };

        contributors.push(Contributor {
            osm_id: Some(*osm_id),
            geometry,
            source_type: source_kind,
            name: acc.name.clone(),
            subtype: subtype_name.to_string(),
            distance_m: acc.min_dist,
            periods: pt_periods,
            periods_free: free_periods,
            emission_db: 10.0 * acc.emission_energy.max(1e-12).log10(),
            baseline: iso9613::compute_baseline(
                acc.min_d_slant,
                SourceGeometry::Point,
                acc.min_ground_g,
            ),
            terrain: pt_effects.0,
            screening: pt_effects.1,
            vegetation: pt_effects.2,
            terrain_impact_db: round1(impacts.terrain),
            screening_impact_db: round1(impacts.screening),
            vegetation_impact_db: round1(impacts.vegetation),
            atmospheric_impact_db: round1(impacts.atmospheric),
            ground_impact_db: round1(impacts.ground),
            received_bands: std::array::from_fn(|j| {
                10.0 * acc.variants[0].band_energy[j].max(1e-30).log10()
            }),
            metadata,
        });
    }

    let mut total_energy = [0.0f64; 3];
    // f64 addition is not associative: ascending key order, not HashMap
    // order, or this total moves ±1 ULP per query.
    for (_, acc) in crate::compute::key_sorted(&pts_by_osm) {
        total_energy[0] += acc.variants[0].full_energy;
        total_energy[1] += acc.variants[1].full_energy;
        total_energy[2] += acc.variants[2].full_energy;
    }
    let ld = 10.0 * total_energy[0].max(1e-12).log10();
    let le = 10.0 * total_energy[1].max(1e-12).log10();
    let ln = 10.0 * total_energy[2].max(1e-12).log10();
    (periods::periods(ld, le, ln), contributors)
}
