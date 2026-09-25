//! Building-envelope classes: enclosed buildings versus open roof structures.

/// Overture-derived envelope class.  Unknown stored values deliberately become
/// `Default`: old shards must remain usable without creating outdoor holes.
///
/// Only [`EnvelopeClass::Outdoor`] (carports, roof structures) is open: a point
/// under it is an outdoor receiver. Every other class is an enclosed building,
/// whose points take the building exposure (its noisiest façade receiver); the
/// class then only names the building in the hover. There is no indoor
/// attenuation anywhere (owner decision 2026-09-24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EnvelopeClass {
    Outdoor = 0,
    Residential = 1,
    Commercial = 2,
    Industrial = 3,
    Historic = 4,
    Default = 5,
}

impl EnvelopeClass {
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Outdoor,
            1 => Self::Residential,
            2 => Self::Commercial,
            3 => Self::Industrial,
            4 => Self::Historic,
            _ => Self::Default,
        }
    }

    /// Whether a point inside this footprint is inside an enclosed building.
    pub const fn is_enclosed(self) -> bool {
        !matches!(self, Self::Outdoor)
    }
}
