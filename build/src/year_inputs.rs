//! Inputs resolve by public id below a separate root for each year; required kinds cannot disappear.
use super::{Kind, Manifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct YearInputs {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    paths: BTreeMap<Kind, Vec<PathBuf>>,
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
            manifest,
            manifest_path: manifest_path.into(),
            paths,
        })
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
