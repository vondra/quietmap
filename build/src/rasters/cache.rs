//! Bind atomic raster squares to the exact manifest, executable and normalized source recipes.
use super::source::Source;
use crate::{
    pipeline_files::{digest, write_json},
    year::YearInputs,
};
use std::path::Path;

pub(super) fn prepare(inputs: &YearInputs, sources: &[Source], root: &Path) -> Result<(), String> {
    let identity = (|| -> anyhow::Result<_> {
        let files = sources
            .iter()
            .flat_map(|source| std::iter::once(source).chain(source.antimeridian.as_deref()))
            .map(|source| {
                Ok(serde_json::json!([
                    source.path.canonicalize()?,
                    digest(&source.path)?
                ]))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(serde_json::json!({
            "manifest": digest(&inputs.manifest_path)?,
            "builder": digest(&std::env::current_exe()?)?, "sources": files
        }))
    })()
    .map_err(|error| error.to_string())?;
    prepare_identity(root, &identity)
}

fn prepare_identity(root: &Path, identity: &serde_json::Value) -> Result<(), String> {
    let receipt = root.join("model.json");
    if receipt.exists() {
        let actual: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&receipt).map_err(|error| format!("{}: {error}", receipt.display()))?,
        )
        .map_err(|error| error.to_string())?;
        if actual != *identity {
            return Err("raster cache belongs to different inputs or code".into());
        }
    } else {
        std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
        for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
            if entry.map_err(|error| error.to_string())?.file_name() != "model.json.tmp" {
                return Err("raster cache has outputs without a model receipt".into());
            }
        }
        write_json(&receipt, identity).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_same_model_resumes_completed_raster_squares() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        let model = serde_json::json!({"manifest":"year-a","sources":["raw-dem-a"]});
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("model.json.tmp"), b"interrupted receipt").unwrap();
        super::prepare_identity(&root, &model).unwrap();
        std::fs::write(root.join("completed-square"), b"verified nodes").unwrap();
        super::prepare_identity(&root, &model).unwrap();
        assert_eq!(
            std::fs::read(root.join("completed-square")).unwrap(),
            b"verified nodes"
        );
        assert!(super::prepare_identity(&root, &serde_json::json!({"manifest":"year-b"})).is_err());
        std::fs::remove_file(root.join("model.json")).unwrap();
        assert!(super::prepare_identity(&root, &model).is_err());
    }
}
