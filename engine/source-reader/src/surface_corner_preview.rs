//! Explicitly provisional outdoor surface powers from generation-verified canonical vertices.
use grid::surface_corner::SurfaceCornerInterpolation;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};
use tile_painter::{
    corner_directory::CornerDirectory,
    generation_receipt::{file_digest, GenerationReceipt},
};

#[derive(Clone, PartialEq)]
struct FileIdentity {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl FileIdentity {
    fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            length: metadata.len(),
            modified: metadata.modified().ok()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}
struct PinnedFile {
    path: PathBuf,
    identity: FileIdentity,
    digest: [u8; 32],
}
impl PinnedFile {
    fn open(path: PathBuf) -> Option<Self> {
        let identity = FileIdentity::read(&path)?;
        let digest = file_digest(&path).ok()?;
        (FileIdentity::read(&path)? == identity).then_some(Self {
            path,
            identity,
            digest,
        })
    }
    fn unchanged(&self) -> bool {
        FileIdentity::read(&self.path).as_ref() == Some(&self.identity)
    }
}

pub struct SurfaceCornerReader {
    root: PathBuf,
    sources: PinnedFile,
    rasters: PinnedFile,
    receipt: GenerationReceipt,
}
#[derive(Serialize)]
pub struct SurfaceLayerPreview {
    pub layer: &'static str,
    pub period_power: [f64; 3],
    pub lden_db: Option<f64>,
}
#[derive(Serialize)]
pub struct SurfaceCornerPreview {
    pub status: &'static str,
    pub receiver: &'static str,
    pub accuracy: &'static str,
    pub center: [f64; 2],
    pub layers: [SurfaceLayerPreview; 5],
}
impl SurfaceCornerReader {
    /// Prepared manifests and vectors remain immutable for the initialized process lifetime.
    pub fn open(prepared: &Path, corners: &Path) -> Option<Self> {
        let sources = PinnedFile::open(prepared.join("inputs.sqlite"))?;
        let rasters = PinnedFile::open(prepared.join(raster_reader::catalog::CATALOG_FILE))?;
        let receipt =
            GenerationReceipt::read_compatible(corners, sources.digest, rasters.digest).ok()??;
        Some(Self {
            root: corners.to_path_buf(),
            sources,
            rasters,
            receipt,
        })
    }
    pub fn query(&self, latitude: f64, longitude: f64) -> Option<SurfaceCornerPreview> {
        let lattice = SurfaceCornerInterpolation::at(latitude, longitude)?;
        if !self.sources.unchanged() || !self.rasters.unchanged() {
            return None;
        }
        let receipt = GenerationReceipt::read_compatible(
            &self.root,
            self.sources.digest,
            self.rasters.digest,
        )
        .ok()??;
        if receipt != self.receipt {
            return None;
        }
        let directory = CornerDirectory::new(&self.root, receipt.generation());
        let mut powers = [[0.0_f64; 3]; 5];
        for (corner, weight) in lattice.corners.into_iter().zip(lattice.weights) {
            let totals = directory.read(corner).ok()??;
            for (layer, totals) in powers.iter_mut().zip(totals.0) {
                for (power, value) in layer.iter_mut().zip(totals) {
                    *power += weight * f64::from(value);
                }
            }
        }
        Some(SurfaceCornerPreview {
            status: "provisional",
            receiver: "outdoor",
            accuracy: "unmeasured",
            center: [latitude, longitude],
            layers: std::array::from_fn(|layer| {
                let period_power = powers[layer];
                let db = period_power.map(|value| 10.0 * value.log10());
                let lden = noise_compute::periods::compute_lden(db[0], db[1], db[2]);
                SurfaceLayerPreview {
                    layer: ["road", "rail", "industry", "building", "ground_ops"][layer],
                    period_power,
                    lden_db: lden.is_finite().then_some(lden),
                }
            }),
        })
    }
}

#[cfg(feature = "node")]
static ACTIVE: std::sync::LazyLock<std::sync::RwLock<Option<SurfaceCornerReader>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

#[cfg(feature = "node")]
#[napi_derive::napi]
pub fn source_init_surface_corners(corners_root: String) -> bool {
    let Ok(mut active) = ACTIVE.write() else {
        return false;
    };
    *active = crate::YEAR_DIR
        .get()
        .and_then(|prepared| SurfaceCornerReader::open(prepared, Path::new(&corners_root)));
    active.is_some()
}

#[cfg(feature = "node")]
#[napi_derive::napi]
pub fn query_surface_corner_preview(latitude: f64, longitude: f64) -> String {
    let preview = ACTIVE.read().ok().and_then(|active| {
        active
            .as_ref()
            .and_then(|reader| reader.query(latitude, longitude))
    });
    serde_json::to_string(&preview).unwrap_or_else(|_| "null".into())
}

#[cfg(test)]
#[path = "surface_corner_preview_tests.rs"]
mod tests;
