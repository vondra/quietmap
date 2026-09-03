//! Static renderer attestations and the shared prepared-dependency resolver.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use h3o::CellIndex;
use serde::Serialize;

use crate::renderer_evidence::{
    emit_json, required_env, runtime_shape_from_env, sha256_file, RendererEvidence,
    RuntimeParameters,
};

pub const RENDERER_STATIC_ATTEST_FLAG: &str = "QM_RENDERER_STATIC_ATTEST_V1";

#[derive(Clone, Copy, Debug)]
pub enum DependencyProfile {
    Surface,
    Aircraft,
}

pub struct StaticAttestationParameters {
    pub runtime: RuntimeParameters,
    pub accepted_options: Vec<String>,
    pub prepared_root: PathBuf,
    pub h3r4_dir: PathBuf,
    pub halo_m: f64,
    pub layers: Vec<String>,
    pub profile: DependencyProfile,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct DependencyRecord {
    role: String,
    relative_path: String,
    required: bool,
    resolution: &'static str,
}

#[derive(Debug, Serialize)]
struct CliContractV1 {
    schema: &'static str,
    role: String,
    executable_sha256: String,
    accepted_options: Vec<String>,
}

#[derive(Debug, Serialize)]
struct WorksetV1 {
    schema: &'static str,
    role: String,
    cells: Vec<String>,
    tile_layer_keys: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DependencyPlanV1 {
    schema: &'static str,
    role: String,
    cell: String,
    dependencies: Vec<DependencyRecord>,
}

impl RendererEvidence {
    /// Resolve and record the paths the production region pipeline is about to consume.
    #[allow(clippy::too_many_arguments)]
    pub fn region_dependencies(
        &self,
        cell: u64,
        prepared_root: &Path,
        h3r4_dir: &Path,
        tiles: &[(u32, u32)],
        zoom: u8,
        halo_m: f64,
        layers: &[&str],
        profile: DependencyProfile,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        for dependency in collect_region_dependencies(
            cell,
            prepared_root,
            h3r4_dir,
            tiles,
            zoom,
            halo_m,
            layers,
            profile,
        )? {
            self.dependency_open(
                cell,
                &dependency.role,
                &dependency.relative_path,
                dependency.required,
                dependency.resolution == "present",
            )?;
            if dependency.required && dependency.resolution != "present" {
                bail!(
                    "required renderer dependency is absent: {} ({})",
                    dependency.relative_path,
                    dependency.role
                );
            }
        }
        Ok(())
    }
}

/// Emit one static contract and return before source, raster, output or CUDA initialization.
pub fn maybe_run_static_attestation(
    role: &str,
    parameters: StaticAttestationParameters,
) -> Result<bool> {
    let Ok(mode) = std::env::var(RENDERER_STATIC_ATTEST_FLAG) else {
        return Ok(false);
    };
    if !matches!(
        mode.as_str(),
        "cli-contract" | "workset" | "runtime-shape" | "dependency-plan"
    ) {
        bail!(
            "{RENDERER_STATIC_ATTEST_FLAG} must be cli-contract, workset, runtime-shape or dependency-plan"
        );
    }
    match mode.as_str() {
        "cli-contract" => {
            let mut accepted_options = parameters.accepted_options;
            accepted_options.sort();
            accepted_options.dedup();
            emit_json(&CliContractV1 {
                schema: "cli-contract-v1",
                role: role.to_string(),
                executable_sha256: sha256_file(&std::env::current_exe()?)?,
                accepted_options,
            })?;
        }
        "runtime-shape" => emit_json(&runtime_shape_from_env(role, parameters.runtime)?)?,
        "workset" | "dependency-plan" => {
            let vector_buildings = matches!(parameters.profile, DependencyProfile::Surface)
                || parameters.layers.iter().any(|layer| {
                    matches!(layer.as_str(), "aircraft-airborne" | "aircraft-combined")
                });
            if mode == "dependency-plan"
                && vector_buildings
                && (std::env::var_os("QM_OBSTACLES_ALLOW_PARTIAL").is_some()
                    || std::env::var_os("QM_OBSTACLES_DIR").is_some())
            {
                bail!(
                    "vector-building dependency attestation requires canonical complete obstacles"
                );
            }
            let cells_path = required_env("QM_RENDERER_ATTEST_CELLS")?;
            let mut cells = crate::region_runner::read_r4_file(Path::new(&cells_path))?;
            cells.sort_unstable();
            cells.dedup();
            if mode == "workset" {
                let mut keys = Vec::new();
                for &cell in &cells {
                    for (x, y) in crate::region_runner::region_tiles(cell, parameters.runtime.zoom)
                    {
                        for layer in &parameters.layers {
                            keys.push(format!("{layer}/{}/{x}/{y}", parameters.runtime.zoom));
                        }
                    }
                }
                keys.sort();
                keys.dedup();
                emit_json(&WorksetV1 {
                    schema: "workset-v1",
                    role: role.to_string(),
                    cells: cells.iter().map(|cell| format!("{cell:015x}")).collect(),
                    tile_layer_keys: keys,
                })?;
            } else {
                let layer_refs: Vec<&str> = parameters.layers.iter().map(String::as_str).collect();
                for cell in cells {
                    let tiles = crate::region_runner::region_tiles(cell, parameters.runtime.zoom);
                    let dependencies = collect_region_dependencies(
                        cell,
                        &parameters.prepared_root,
                        &parameters.h3r4_dir,
                        &tiles,
                        parameters.runtime.zoom,
                        parameters.halo_m,
                        &layer_refs,
                        parameters.profile,
                    )?;
                    emit_json(&DependencyPlanV1 {
                        schema: "dependency-plan-v1",
                        role: role.to_string(),
                        cell: format!("{cell:015x}"),
                        dependencies,
                    })?;
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn collect_region_dependencies(
    cell: u64,
    prepared_root: &Path,
    h3r4_dir: &Path,
    tiles: &[(u32, u32)],
    zoom: u8,
    halo_m: f64,
    layers: &[&str],
    profile: DependencyProfile,
) -> Result<Vec<DependencyRecord>> {
    let mut dependencies = Vec::new();
    let h3r4_relative = h3r4_dir
        .strip_prefix(prepared_root)
        .context("H3R4 directory is outside prepared root")?;
    let ring = CellIndex::try_from(cell)?.grid_disk::<Vec<_>>(1);
    let mut source_files = Vec::new();
    for layer in layers {
        match *layer {
            "road" => source_files.push("roads.arrow"),
            "rail" => source_files.push("railways.arrow"),
            "industrial" => source_files.push("industrial.arrow"),
            "building" => source_files.extend(["buildings.arrow", "leisure.arrow"]),
            "aircraft-ground" => source_files.push("airport_traffic.arrow"),
            "aircraft-airborne" => source_files.push("airborne.arrow"),
            "aircraft-cruise" => source_files.push("cruise.arrow"),
            "aircraft-combined" => source_files.extend(["airborne.arrow", "cruise.arrow"]),
            other => bail!("unsupported renderer evidence layer {other:?}"),
        }
    }
    let airborne_buildings = layers
        .iter()
        .any(|layer| matches!(*layer, "aircraft-airborne" | "aircraft-combined"));
    for ring_cell in ring {
        let cell_hex = format!("{:015x}", u64::from(ring_cell));
        for file in &source_files {
            collect_resolved_path(
                &mut dependencies,
                prepared_root,
                "source-arrow",
                &h3r4_relative.join(&cell_hex).join(file),
                false,
            )?;
        }
        if matches!(profile, DependencyProfile::Surface) {
            collect_resolved_path(
                &mut dependencies,
                prepared_root,
                "barrier-arrow",
                &h3r4_relative.join(&cell_hex).join("barriers.arrow"),
                false,
            )?;
        }
        if matches!(profile, DependencyProfile::Surface) || airborne_buildings {
            collect_resolved_path(
                &mut dependencies,
                prepared_root,
                "obstacle-arrow",
                &h3r4_relative.join(&cell_hex).join("obstacles.arrow"),
                true,
            )?;
            collect_resolved_path(
                &mut dependencies,
                prepared_root,
                "low-profile-arrow",
                &h3r4_relative.join(&cell_hex).join("buildings.arrow"),
                false,
            )?;
        }
    }
    if matches!(profile, DependencyProfile::Surface)
        && layers.iter().any(|layer| matches!(*layer, "road" | "rail"))
    {
        let admin = noise_compute::admin::default_admin_path(h3r4_dir);
        let relative = admin
            .strip_prefix(prepared_root)
            .context("physical admin path is outside prepared root")?;
        collect_resolved_path(
            &mut dependencies,
            prepared_root,
            "physical-admin",
            relative,
            true,
        )?;
    }
    collect_raster_dependencies(
        &mut dependencies,
        prepared_root,
        tiles,
        zoom,
        halo_m,
        profile,
    )?;
    dependencies.sort();
    dependencies.dedup();
    Ok(dependencies)
}

fn collect_resolved_path(
    dependencies: &mut Vec<DependencyRecord>,
    prepared_root: &Path,
    role: &str,
    relative: &Path,
    required: bool,
) -> Result<()> {
    let relative_text = relative.to_str().context("dependency path is not UTF-8")?;
    dependencies.push(DependencyRecord {
        role: role.to_string(),
        relative_path: relative_text.to_string(),
        required,
        resolution: if prepared_root.join(relative).is_file() {
            "present"
        } else {
            "absent"
        },
    });
    Ok(())
}

fn collect_raster_dependencies(
    dependencies: &mut Vec<DependencyRecord>,
    prepared_root: &Path,
    tiles: &[(u32, u32)],
    zoom: u8,
    halo_m: f64,
    profile: DependencyProfile,
) -> Result<()> {
    use std::collections::BTreeSet;

    let mut degree_cells = BTreeSet::new();
    for &(x, y) in tiles {
        let bbox = crate::grid::tile_bbox(zoom, x, y);
        let lat_pad = halo_m / noise_compute::emission::aircraft::M_PER_DEG_LAT;
        let mid_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
        let lon_scale = mid_lat.to_radians().cos().abs().max(0.01);
        let lon_pad = lat_pad / lon_scale;
        let lat0 = (bbox.south_lat - lat_pad).floor() as i32;
        let lat1 = (bbox.north_lat + lat_pad).floor() as i32;
        let lon0 = (bbox.west_lon - lon_pad).floor() as i32;
        let lon1 = (bbox.east_lon + lon_pad).floor() as i32;
        for lat in lat0..=lat1 {
            for lon in lon0..=lon1 {
                degree_cells.insert((lat, lon));
            }
        }
    }
    for (lat, lon) in degree_cells {
        let base = format!(
            "{}{:02}{}{:03}",
            if lat >= 0 { 'N' } else { 'S' },
            lat.unsigned_abs(),
            if lon >= 0 { 'E' } else { 'W' },
            lon.unsigned_abs(),
        );
        let primary = Path::new("dem/copernicus").join(format!("{base}.hgt"));
        let primary_present = prepared_root.join(&primary).is_file();
        collect_resolved_path(
            dependencies,
            prepared_root,
            "dem-primary",
            &primary,
            primary_present,
        )?;
        collect_resolved_path(
            dependencies,
            prepared_root,
            "dem-fallback",
            &Path::new("dem/srtm").join(format!("{base}.hgt")),
            !primary_present,
        )?;
        if matches!(profile, DependencyProfile::Surface) {
            for (role, dir) in [
                ("forest-raster", "rasters/forest"),
                ("imd-raster", "rasters/imd"),
            ] {
                collect_resolved_path(
                    dependencies,
                    prepared_root,
                    role,
                    &Path::new(dir).join(format!("{base}.raw")),
                    true,
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn surface_plan_names_every_building_source_and_requires_complete_geometry() {
        let root = tempdir().unwrap();
        let h3r4 = root.path().join("2026/h3r4");
        std::fs::create_dir_all(&h3r4).unwrap();
        let cell = 0x841e309ffffffff;
        let tiles = crate::region_runner::region_tiles(cell, 13);
        let dependencies = collect_region_dependencies(
            cell,
            root.path(),
            &h3r4,
            &tiles,
            13,
            10_000.0,
            &["building"],
            DependencyProfile::Surface,
        )
        .unwrap();

        assert!(dependencies
            .iter()
            .any(|item| item.relative_path.ends_with("/buildings.arrow")));
        assert!(dependencies
            .iter()
            .any(|item| item.relative_path.ends_with("/leisure.arrow")));
        assert!(dependencies.iter().any(|item| {
            item.role == "obstacle-arrow" && item.required && item.resolution == "absent"
        }));
        assert!(dependencies.iter().any(|item| {
            item.role == "dem-fallback" && item.required && item.resolution == "absent"
        }));
    }

    #[test]
    fn unknown_layer_never_degrades_to_an_empty_dependency_plan() {
        let root = tempdir().unwrap();
        let h3r4 = root.path().join("2026/h3r4");
        std::fs::create_dir_all(&h3r4).unwrap();
        let error = collect_region_dependencies(
            0x841e309ffffffff,
            root.path(),
            &h3r4,
            &[],
            13,
            0.0,
            &["misspelled-layer"],
            DependencyProfile::Aircraft,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported renderer evidence layer"));
    }

    #[test]
    fn airborne_plan_requires_obstacles_and_tracks_optional_low_profile_roofs() {
        let root = tempdir().unwrap();
        let h3r4 = root.path().join("2026/h3r4");
        std::fs::create_dir_all(&h3r4).unwrap();
        let dependencies = collect_region_dependencies(
            0x841e309ffffffff,
            root.path(),
            &h3r4,
            &[],
            13,
            noise_compute::emission::aircraft::RECEIVER_HORIZON_MAX_M,
            &["aircraft-airborne"],
            DependencyProfile::Aircraft,
        )
        .unwrap();

        assert!(dependencies.iter().any(|item| {
            item.role == "obstacle-arrow" && item.required && item.resolution == "absent"
        }));
        assert!(dependencies.iter().any(|item| {
            item.role == "low-profile-arrow" && !item.required && item.resolution == "absent"
        }));
        assert!(!dependencies
            .iter()
            .any(|item| matches!(item.role.as_str(), "forest-raster" | "imd-raster")));
    }
}
