//! Building-envelope classes used by the indoor display estimate.

/// Overture-derived envelope class.  Unknown stored values deliberately become
/// `Default`: old shards must remain usable without creating outdoor holes.
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
    pub const fn delta_db(self) -> Option<f64> {
        match self {
            Self::Outdoor => None,
            Self::Residential => Some(30.0),
            Self::Commercial => Some(35.0),
            Self::Industrial => Some(20.0),
            Self::Historic | Self::Default => Some(28.0),
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Outdoor => "outdoor",
            Self::Residential => "residential",
            Self::Commercial => "commercial",
            Self::Industrial => "industrial",
            Self::Historic => "historic",
            Self::Default => "default",
        }
    }
}
