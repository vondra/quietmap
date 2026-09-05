//! The single physical and numeric contract for the three native-lattice raster channels.

use grid::{raster::RasterWindow, square_name, Square};
use std::path::{Path, PathBuf};

pub const CONTRACT: &str = "raster_z9_arcsec_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Dem,
    Forest,
    Imd,
}

impl Channel {
    pub const ALL: [Self; 3] = [Self::Dem, Self::Forest, Self::Imd];

    pub fn name(self) -> &'static str {
        match self {
            Self::Dem => "dem",
            Self::Forest => "forest",
            Self::Imd => "imd",
        }
    }

    pub fn bytes_per_node(self) -> usize {
        if self == Self::Dem {
            2
        } else {
            1
        }
    }

    pub fn byte_len(self, window: RasterWindow) -> usize {
        window.cell_count() * self.bytes_per_node()
    }

    pub fn path(self, root: &Path, square: Square) -> PathBuf {
        let extension = if self == Self::Dem { "i16be" } else { "u8" };
        root.join(square_name(square))
            .join(format!("{}.{extension}", self.name()))
    }

    pub fn source_extension(self) -> &'static str {
        if self == Self::Dem {
            "hgt"
        } else {
            "raw"
        }
    }

    /// Permission failures and dangling symlinks are not evidence of an absent data file.
    pub fn file_is_absent(self, root: &Path, square: Square) -> std::io::Result<bool> {
        match std::fs::symlink_metadata(self.path(root, square)) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// Only a coverage-verified absent source window permits these ocean values.
    pub fn ocean_value(self) -> i16 {
        if self == Self::Imd {
            100
        } else {
            0
        }
    }

    pub fn decode(self, bytes: &[u8], node: usize) -> f64 {
        if self == Self::Dem {
            let offset = node * 2;
            let value = i16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            if value == i16::MIN {
                f64::NAN
            } else {
                f64::from(value)
            }
        } else {
            let value = bytes[node];
            if value > 100 {
                f64::NAN
            } else {
                f64::from(value)
            }
        }
    }
}
