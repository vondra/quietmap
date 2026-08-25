//! Materialise an nvcc-built cubin atomically and load it through cudarc's file API.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use cudarc::driver::CudaDevice;
use cudarc::nvrtc::Ptx;
use sha2::{Digest, Sha256};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn image_sha256(image: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(image) {
        write!(&mut digest, "{byte:02x}").expect("write to String");
    }
    digest
}

fn image_path(module_name: &str, digest: &str) -> PathBuf {
    std::env::temp_dir()
        .join("quietmap-cuda-aot")
        .join(format!("{module_name}-{digest}.cubin"))
}

fn materialise_image(path: &Path, image: &[u8]) -> Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == image) {
        return Ok(());
    }
    let parent = path.parent().expect("cubin cache path has parent");
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".cubin-{}-{sequence}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(image)
        .with_context(|| format!("write {}", temporary.display()))?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        if !fs::read(path).is_ok_and(|existing| existing == image) {
            return Err(error).with_context(|| {
                format!(
                    "publish embedded cubin {} -> {}",
                    temporary.display(),
                    path.display()
                )
            });
        }
    }
    Ok(())
}

/// Load an embedded architecture-specific cubin without driver PTX JIT, falling back to PTX.
///
/// cudarc 0.12 exposes binary modules only through `Ptx::from_file`. The image
/// is therefore published to a content-addressed temp path first; module load is
/// still deterministic, and every later worker/process reuses the same file.
/// A successful load emits the image SHA so benchmark receipts can prove the
/// embedded AOT artifact was used instead of merely observing no fallback.
#[doc(hidden)]
pub fn load_embedded_cubin_or_ptx(
    device: &Arc<CudaDevice>,
    image: &[u8],
    fallback_ptx: &'static str,
    module_name: &str,
    function_names: &[&'static str],
) -> Result<()> {
    let digest = image_sha256(image);
    let path = image_path(module_name, &digest);
    let cubin_result = materialise_image(&path, image).and_then(|()| {
        device
            .load_ptx(Ptx::from_file(&path), module_name, function_names)
            .with_context(|| format!("load AOT cubin {}", path.display()))
    });
    match cubin_result {
        Ok(()) => {
            eprintln!(
                "AOT_CUBIN_LOADED module={module_name} sha256={digest} path={}",
                path.display()
            );
            Ok(())
        }
        Err(cubin_error) => {
            eprintln!(
                "AOT cubin unavailable for module {module_name}; falling back to embedded PTX: {cubin_error:#}"
            );
            device
                .load_ptx(Ptx::from_src(fallback_ptx), module_name, function_names)
                .with_context(|| {
                    format!(
                        "load PTX fallback for module {module_name} after AOT cubin failure: {cubin_error:#}"
                    )
                })
        }
    }
}
