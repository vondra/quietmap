//! Verify the declared GDAL/PROJ runtime and use only the admitted, fingerprinted datum bundle.
use super::config::RasterPlan;
use crate::year::YearInputs;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, ffi::CString, io::Read, path::Path};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RasterRuntime {
    pub gdal_version: String,
    pub proj_version: String,
    pub datum_input: String,
    pub files: BTreeMap<String, String>,
}

impl RasterRuntime {
    pub fn validate(&self) -> Result<(), String> {
        if self.gdal_version.is_empty()
            || self.proj_version.is_empty()
            || !self.files.contains_key("proj.db")
        {
            return Err("raster runtime needs GDAL, PROJ and its database".into());
        }
        for (name, digest) in &self.files {
            if Path::new(name).file_name().and_then(|v| v.to_str()) != Some(name)
                || matches!(name.as_str(), "." | "..")
                || digest.len() != 64
                || !digest.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err(format!("invalid datum file or SHA-256: {name}"));
            }
        }
        Ok(())
    }
}

fn verify_bundle(root: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
    let found = std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            entry
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    if found != files.keys().cloned().collect() {
        return Err("datum directory contains missing or undeclared files".into());
    }
    let mut buffer = vec![0; 1024 * 1024];
    for (name, expected) in files {
        let mut file = std::fs::File::open(root.join(name)).map_err(|e| e.to_string())?;
        let mut digest = Sha256::new();
        loop {
            let size = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if size == 0 {
                break;
            }
            digest.update(&buffer[..size]);
        }
        if format!("{:x}", digest.finalize()) != *expected {
            return Err(format!("datum fingerprint differs: {name}"));
        }
    }
    Ok(())
}

pub fn configure(inputs: &YearInputs, plan: &RasterPlan) -> Result<(), String> {
    let expected = &plan.runtime;
    let gdal = gdal::version_info("RELEASE_NAME");
    let (mut major, mut minor, mut patch) = (0, 0, 0);
    unsafe { gdal_sys::OSRGetPROJVersion(&mut major, &mut minor, &mut patch) };
    let proj = format!("{major}.{minor}.{patch}");
    if gdal != expected.gdal_version || proj != expected.proj_version {
        return Err(format!("raster runtime differs: GDAL {gdal}, PROJ {proj}"));
    }
    let directory = inputs.by_id(&expected.datum_input)?;
    verify_bundle(&directory, &expected.files)?;
    let directory =
        CString::new(directory.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    unsafe {
        // Explicit PROJ search paths replace user and system paths, including proj.db.
        gdal_sys::OSRSetPROJSearchPaths([directory.as_ptr(), std::ptr::null()].as_ptr());
        gdal_sys::OSRSetPROJAuxDbPaths([std::ptr::null()].as_ptr());
        gdal_sys::OSRSetPROJEnableNetwork(0);
        gdal_sys::GDALSetCacheMax64(256 * 1024 * 1024);
    }
    for (name, value) in [("GDAL_FORCE_CACHING", "YES"), ("GTIFF_SRS_SOURCE", "EPSG")] {
        gdal::config::set_config_option(name, value).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_bundle_refuses_changed_missing_and_extra_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("proj.db"), b"original").unwrap();
        let files = BTreeMap::from([(
            "proj.db".into(),
            format!("{:x}", Sha256::digest(b"original")),
        )]);
        verify_bundle(root.path(), &files).unwrap();
        std::fs::write(root.path().join("other"), b"extra").unwrap();
        assert!(verify_bundle(root.path(), &files).is_err());
        std::fs::remove_file(root.path().join("other")).unwrap();
        std::fs::write(root.path().join("proj.db"), b"changed").unwrap();
        assert!(verify_bundle(root.path(), &files).is_err());
        std::fs::remove_file(root.path().join("proj.db")).unwrap();
        assert!(verify_bundle(root.path(), &files).is_err());
    }
}
