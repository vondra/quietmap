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
    let mut cand_scratch = Vec::new();
    use std::collections::HashMap;

    struct PtAccum {
        name: String,
        subtype: u8,
        lat: f64,
        lon: f64,
        min_dist: f64,
        min_d_slant: f64,
        min_ground_g: f64,
        src_height: f64,
        exclusion_radius_m: f32,
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
    }
    let mut pts_by_osm: HashMap<i64, PtAccum> = HashMap::new();
    let reflection = rasters.building_enclosure(receiver.lat, receiver.lon);

    for src in sources {
        let max_d = src.max_radius_m.max(0.0);
        if src.dist_m > max_d {
            continue;
        }

        let src_alt = rasters.elevation(src.lat, src.lon) + src.source_height_m as f64;
        let rcv_alt = receiver.altitude_m();
        let prop_dist = geo::effective_area_source_dist(src.dist_m, src.exclusion_radius_m as f64);
        let d_slant = geo::slant_dist(prop_dist, src_alt, rcv_alt).max(1.0);

        // Early exit: skip if free-field < threshold (matching pipeline)
        {
            let me = src.lw_day.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
            if geo::below_free_field_threshold(me, src.dist_m, 0.0) {
                continue;
            }
        }

        // Unified path profile — one sampling, all path effects read from it.
        let mut path_profile = propagation::PathProfile::new();
        rasters.build_path_profile(
            src.lat,
            src.lon,
            receiver.lat,
            receiver.lon,
            src.dist_m,
            &mut path_profile,
        );
        // Point sources used receiver-local G until the literal ground core.
        // The direct CNOSSOS path requires the same ray-mean IMD semantics as
        // line sources, including the source-end §2.5.14 correction.
        let ground_path = propagation::path_effects::cnossos_ground_path_from_profile(
            &mut path_profile,
            src_alt,
            rcv_alt,
            false,
        );
        let ground_g = ground_path.ground_path_g;
        let ground_bands = iso9613::ground_atten_bands(ground_path);
        let (terrain, _terrain_profile_points) =
            propagation::path_effects::terrain_attenuation_with_meta(
                &mut path_profile,
                src_alt,
                rcv_alt,
            );
        let obstacle_input = crate::obstacle_input_for_ray(
            obstacles,
            &mut cand_scratch,
            src.lat,
            src.lon,
            receiver.lat,
            receiver.lon,
            None,
        );
        let (screening_atten, obstacle_trace) =
            propagation::path_effects::screening_attenuation_with_meta(
                &mut path_profile,
                obstacle_input,
                src_alt,
                rcv_alt,
                src.exclusion_radius_m as f64,
                &terrain.attenuation_bands,
            );
        let veg_atten = propagation::path_effects::vegetation_attenuation_path(&path_profile);

        let v_day = iso9613::propagate_variants_cnossos_ground_full(
            &src.lw_day.map(|v| v as f64),
            d_slant,
            SourceGeometry::Point,
            ground_path,
            &terrain.attenuation_bands,
            &screening_atten,
            &veg_atten,
            reflection,
            0.0,
        );
        let v_eve = iso9613::propagate_variants_cnossos_ground_full(
            &src.lw_evening.map(|v| v as f64),
            d_slant,
            SourceGeometry::Point,
            ground_path,
            &terrain.attenuation_bands,
            &screening_atten,
            &veg_atten,
            reflection,
            0.0,
        );
        let v_night = iso9613::propagate_variants_cnossos_ground_full(
            &src.lw_night.map(|v| v as f64),
            d_slant,
            SourceGeometry::Point,
            ground_path,
            &terrain.attenuation_bands,
            &screening_atten,
            &veg_atten,
            reflection,
            0.0,
        );

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
            src_height: src_alt,
            exclusion_radius_m: src.exclusion_radius_m,
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
            acc.src_height = src_alt;
            acc.exclusion_radius_m = src.exclusion_radius_m;
        }

        if let Some(t) = traces.as_deref_mut() {
            let seg_variants = [v_day, v_eve, v_night];
            let lw_bands: [[f64; NUM_BANDS]; 3] = [
                std::array::from_fn(|i| src.lw_day[i] as f64),
                std::array::from_fn(|i| src.lw_evening[i] as f64),
                std::array::from_fn(|i| src.lw_night[i] as f64),
            ];
            let trace = build_point_segment_trace(BuildPointTrace {
                src,
                source_kind,
                src_alt,
                rcv_alt,
                d_slant,
                prop_dist,
                ground_g,
                ground_bands,
                reflection_boost_db: reflection,
                path_profile: std::mem::take(&mut path_profile),
                terrain,
                screening_atten,
                obstacle_trace,
                veg_atten,
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

        let pt_effects = compute_path_effects(
            rasters,
            obstacles,
            acc.lat,
            acc.lon,
            acc.src_height,
            receiver,
            acc.min_dist,
            acc.exclusion_radius_m as f64,
        );

        let impacts = PropagationVariants::impact_deltas(&acc.variants, pt_periods.lden_db);

        let subtype_name: &'static str = if source_kind == LayerKind::Industrial {
            industrial_type_name(acc.subtype)
        } else {
            building_type_name(acc.subtype)
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
        } else {
            // building. `acc.src_height` is `elevation + height/2`
            // (mid-facade anchor), so subtracting `elevation` gives
            // half the building height. Double to recover the full
            // building height the popup shows under "Height".
            Some(SourceMetadata::Building(BuildingMetadata {
                height_m: ((acc.src_height - rasters.elevation(acc.lat, acc.lon)) * 2.0).max(0.0),
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
