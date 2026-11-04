//! Inputs resolve by public id below a separate root for each year; required kinds cannot disappear.
use super::{Kind, Manifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct YearInputs {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    paths: BTreeMap<Kind, Vec<PathBuf>>,
    root: PathBuf,
}

impl YearInputs {
    pub fn read(manifest_path: &Path, sources: &Path) -> Result<Self, String> {
        let manifest = Manifest::read(manifest_path)?;
        let mut paths: BTreeMap<Kind, Vec<PathBuf>> = BTreeMap::new();
        for input in &manifest.inputs {
            if manifest.admitted(&input.id)? {
                paths
                    .entry(input.kind)
                    .or_default()
                    .push(sources.join(&manifest.year).join(&input.id));
            }
        }
        Ok(Self {
            root: sources.join(&manifest.year),
            manifest,
            manifest_path: manifest_path.into(),
            paths,
        })
    }

    pub fn by_id(&self, id: &str) -> Result<PathBuf, String> {
        if !self.manifest.admitted(id)? {
            return Err(format!(
                "year {}: raster input {id} is not admitted",
                self.manifest.year
            ));
        }
        let path = self.root.join(id);
        if !path.exists() {
            return Err(format!("missing raster input {}", path.display()));
        }
        Ok(path)
    }

    /// A model requiring a kind must have admitted input, even if the inventory explains its absence.
    pub fn paths(&self, kind: Kind) -> Result<&[PathBuf], String> {
        let paths = self
            .paths
            .get(&kind)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                let pending = self
                    .manifest
                    .unavailable
                    .iter()
                    .filter(|i| i.kind == kind)
                    .map(|i| i.description.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(
                    "year {}: no admitted input for required kind {kind}. {pending}",
                    self.manifest.year
                )
            })?;
        for path in paths {
            if !path.exists() {
                return Err(format!(
                    "required kind {kind}: missing input {}",
                    path.display()
                ));
            }
        }
        Ok(paths)
    }

    /// Regional elevation surveys supplement the required world raster; their absence keeps it.
    pub fn national_dem_paths(&self) -> Result<&[PathBuf], String> {
        if self.paths.contains_key(&Kind::NationalDem) {
            self.paths(Kind::NationalDem)
        } else {
            Ok(&[])
        }
    }

    /// An unavailable historical legal table leaves only explicit mapped numeric limits.
    pub fn osm_speed_table(&self) -> Result<Option<&Path>, String> {
        if self.paths.contains_key(&Kind::PrepareOsmSpeeds) {
            self.required(Kind::PrepareOsmSpeeds).map(Some)
        } else if self
            .manifest
            .unavailable
            .iter()
            .any(|i| i.kind == Kind::PrepareOsmSpeeds)
        {
            Ok(None)
        } else {
            Err("declare the OSM legal-speed table or its historical absence".into())
        }
    }

    /// Survey absence falls through the documented height ladder; footprints remain required.
    pub fn structure_observation_sources(&self, kind: Kind) -> Result<&[PathBuf], String> {
        if !matches!(
            kind,
            Kind::PrepareBuildingFootprints
                | Kind::PrepareBuildingHeights
                | Kind::PrepareNoiseBarriers
                | Kind::PrepareBuildingAttributes
        ) {
            return Err(format!("{kind} is not structure evidence"));
        }
        if self.paths.contains_key(&kind) || kind == Kind::PrepareBuildingFootprints {
            self.paths(kind)
        } else if self.manifest.unavailable.iter().any(|i| i.kind == kind) {
            Ok(&[])
        } else {
            Err(format!("declare structure evidence {kind} or its absence"))
        }
    }

    pub fn structure_height_rasters(&self) -> Result<&[PathBuf], String> {
        if self.paths.contains_key(&Kind::StructureHeightRaster) {
            self.paths(Kind::StructureHeightRaster)
        } else if self
            .manifest
            .unavailable
            .iter()
            .any(|i| i.kind == Kind::StructureHeightRaster)
        {
            Ok(&[])
        } else {
            Err("declare measured relative-height rasters or their absence".into())
        }
    }

    pub fn required(&self, kind: Kind) -> Result<&Path, String> {
        match self.paths(kind)? {
            [path] => Ok(path),
            _ => Err(format!("input kind {kind}: expected one input")),
        }
    }

    #[cfg(test)]
    pub fn product_tables() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut inputs = Self::read(
            &root.join("test-data/model-year.json"),
            &root.join("test-data"),
        )
        .unwrap();
        inputs.paths = inputs
            .manifest
            .inputs
            .iter()
            .map(|i| {
                (
                    i.kind,
                    vec![
                        root.join("../years/tables/2026")
                            .join(format!("{}.json", i.kind)),
                    ],
                )
            })
            .collect();
        inputs
    }

    /// Completion publishes exactly the model values used by the build and the About pages.
    pub fn check_published_tables(&self) -> Result<(), String> {
        let directory = self
            .manifest_path
            .parent()
            .ok_or("manifest needs a parent")?
            .join("tables")
            .join(&self.manifest.year);
        for &kind in crate::source_tables::MODEL_KINDS {
            let path = directory.join(format!("{kind}.json"));
            self.check_published_table(kind, &path)?;
        }
        for kind in [
            Kind::PrepareGeography,
            Kind::PrepareOsmSpeeds,
            Kind::StructureHeightModel,
            Kind::FacilityModel,
        ] {
            if self.paths.contains_key(&kind) {
                self.check_published_table(kind, &directory.join(format!("{kind}.json")))?;
            }
        }
        Ok(())
    }

    pub fn check_published_table(&self, kind: Kind, path: &Path) -> Result<(), String> {
        let published =
            std::fs::read(path).map_err(|e| format!("{kind}: {}: {e}", path.display()))?;
        let consumed = std::fs::read(self.required(kind)?).map_err(|e| e.to_string())?;
        if published != consumed {
            return Err(format!(
                "{kind}: input differs from published table {}",
                path.display()
            ));
        }
        Ok(())
    }

    pub fn check_complete(&self, root: &Path) -> Result<(), String> {
        tiles::check_year_complete(root)?;
        let expected = std::fs::read(&self.manifest_path).map_err(|e| e.to_string())?;
        let actual = std::fs::read(root.join("year.json")).map_err(|e| e.to_string())?;
        if actual != expected {
            return Err(format!(
                "{}: completion manifest differs from --year",
                root.display()
            ));
        }
        Ok(())
    }
}
