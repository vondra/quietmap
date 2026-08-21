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
            Self::Historic => Some(28.0),
            // WHO Environmental Noise Guidelines for the European Region
            // (2018) provide the newer ~25 dB closed-window context for an
            // otherwise unclassified building; the other class-specific
            // values remain EN 12354 practice assumptions.
            Self::Default => Some(25.0),
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

/// Select the in-memory envelope class used for the indoor display estimate.
///
/// The Arrow `envelope_class` remains the source classification. Only an
/// unclassified low building uses the lightweight 20 dB assumption, encoded
/// by reusing the existing `Industrial` delta; all taller/default buildings
/// keep the WHO 2018-informed 25 dB `Default` delta. Keeping this decision in
/// the shared noise-compute crate makes painter and popup selection identical.
// Owner product decision (2026-08-22, variant A): the 6 m / 20 dB pairing
// treats an unclassified building at or below 6 m as the garage/shed/lightweight
// class, following EN 12354 practice. WHO 2018 anchors only the 25 dB default;
// the resulting 5 dB discontinuity at 6 m is deliberate and auditable.
pub const fn effective_envelope_class(class: EnvelopeClass, height_m: f32) -> EnvelopeClass {
    match class {
        EnvelopeClass::Default if height_m <= 6.0 => EnvelopeClass::Industrial,
        _ => class,
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_envelope_class, EnvelopeClass};

    #[test]
    fn effective_envelope_class_height_matrix() {
        assert_eq!(
            effective_envelope_class(EnvelopeClass::Default, 5.0),
            EnvelopeClass::Industrial
        );
        assert_eq!(
            effective_envelope_class(EnvelopeClass::Default, 6.0),
            EnvelopeClass::Industrial,
            "the owner boundary includes exactly 6.0 m"
        );
        let just_above_six_m = f32::from_bits(6.0_f32.to_bits() + 1);
        assert!(just_above_six_m > 6.0);
        assert_eq!(
            effective_envelope_class(EnvelopeClass::Default, just_above_six_m),
            EnvelopeClass::Default,
            "the owner boundary switches immediately above 6.0 m"
        );
        assert_eq!(
            effective_envelope_class(EnvelopeClass::Default, 7.0),
            EnvelopeClass::Default
        );
        assert_eq!(
            effective_envelope_class(EnvelopeClass::Default, 5.0)
                .delta_db()
                .unwrap(),
            20.0
        );
        assert_eq!(
            effective_envelope_class(EnvelopeClass::Default, 7.0)
                .delta_db()
                .unwrap(),
            25.0
        );

        for class in [
            EnvelopeClass::Residential,
            EnvelopeClass::Commercial,
            EnvelopeClass::Industrial,
            EnvelopeClass::Historic,
        ] {
            assert_eq!(effective_envelope_class(class, 5.0), class);
            assert_eq!(effective_envelope_class(class, 7.0), class);
        }
    }
}
