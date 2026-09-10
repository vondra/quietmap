//! Fingerprint shared surface physics, decoding and producer code for cache compatibility.
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        println!("cargo:rerun-if-changed={}", path.display());
        for item in fs::read_dir(path).expect("source directory") {
            collect(&item.expect("source entry").path(), files);
        }
    } else {
        files.push(path.to_path_buf());
    }
}
fn main() {
    let root = Path::new("..");
    let mut files = Vec::new();
    for package in [
        "grid",
        "noise-compute",
        "raster-reader",
        "square-store",
        "source-reader",
        "tile-painter",
        "relevant-source-gpu",
    ] {
        collect(&root.join(package).join("src"), &mut files);
        for name in ["Cargo.toml", "build.rs", "cuda_archs.rs"] {
            let file = root.join(package).join(name);
            if file.is_file() {
                files.push(file);
            }
        }
    }
    collect(&root.join("relevant-source-gpu/kernels"), &mut files);
    files.push(root.join("Cargo.lock"));
    files.sort();
    let mut hash = Sha256::new();
    hash.update(b"surface-code-v1");
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let relative = file.strip_prefix(root).unwrap().to_string_lossy();
        let bytes = fs::read(&file).expect("source bytes");
        hash.update((relative.len() as u64).to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let digest: [u8; 32] = hash.finalize().into();
    let generated = format!("pub const SURFACE_CODE_DIGEST: [u8;32] = {digest:?};\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("surface_code.rs"),
        generated,
    )
    .unwrap();
}
