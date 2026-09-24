//! The single physical and numeric contract for the four native-lattice raster channels.

use grid::{raster::RasterWindow, square_name, Square};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Dem,
    Canopy,
    Forest,
    Imd,
}

pub const DEM_OFFSET_M: f64 = -500.0;
pub const DEM_CODES_PER_METRE: f64 = 5.0;
pub const DEM_MISSING: u16 = u16::MAX;

impl Channel {
    pub const ALL: [Self; 4] = [Self::Dem, Self::Canopy, Self::Forest, Self::Imd];

    pub fn name(self) -> &'static str {
        match self {
            Self::Dem => "dem",
            Self::Canopy => "canopy",
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
        let extension = if self == Self::Dem { "u16le" } else { "u8" };
        root.join(square_name(square))
            .join(format!("{}.{extension}", self.name()))
    }

    pub fn source_extension(self) -> &'static str {
        if self == Self::Dem {
            "u16le"
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

    /// Encode a physical value; NaN is the channel's explicit missing node.
    pub fn encode(self, value: f64) -> [u8; 2] {
        if self == Self::Dem {
            let raw = if value.is_nan() {
                u16::MAX
            } else {
                assert!((DEM_OFFSET_M
                    ..=DEM_OFFSET_M + f64::from(DEM_MISSING - 1) / DEM_CODES_PER_METRE)
                    .contains(&value));
                ((value - DEM_OFFSET_M) * DEM_CODES_PER_METRE).round() as u16
            };
            raw.to_le_bytes()
        } else {
            let maximum = if self == Self::Canopy { 250.0 } else { 100.0 };
            assert!(value.is_nan() || (0.0..=maximum).contains(&value));
            [
                if value.is_nan() {
                    255
                } else {
                    value.round() as u8
                },
                0,
            ]
        }
    }

    pub fn decode(self, bytes: &[u8], node: usize) -> f64 {
        if self == Self::Dem {
            let offset = node * 2;
            let value = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            if value == u16::MAX {
                f64::NAN
            } else {
                DEM_OFFSET_M + f64::from(value) / DEM_CODES_PER_METRE
            }
        } else {
            let value = bytes[node];
            if value > if self == Self::Canopy { 250 } else { 100 } {
                f64::NAN
            } else {
                f64::from(value)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_dem_and_canopy_have_distinct_missing_codes() {
        for (raw, metres) in [
            (0_u16, -500.0),
            (2499, -0.2),
            (2500, 0.0),
            (2501, 0.2),
            (65534, 12606.8),
        ] {
            assert!((Channel::Dem.decode(&raw.to_le_bytes(), 0) - metres).abs() < 1e-10);
            assert_eq!(Channel::Dem.encode(metres), raw.to_le_bytes());
        }
        assert!(Channel::Dem.decode(&[255, 255], 0).is_nan());
        for raw in 0..=255 {
            assert_eq!(Channel::Canopy.decode(&[raw], 0).is_finite(), raw <= 250);
        }
    }
}
