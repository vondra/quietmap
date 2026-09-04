//! Re-exports of `noise-compute` aircraft NPD + classification types.
//!
//! Centralised so the rest of the crate doesn't reach across module
//! boundaries — and so a future schedule-synth driver can rebind only
//! this file when ANP-v9 anchors shift.

pub use noise_compute::emission::aircraft::{
    clamp_profile_idx, is_ga_sampled_class, is_ga_sampled_profile, is_jet_profile,
    is_negligible_noise_typecode, is_non_aircraft_typecode, noise_class_of, profile_idx,
    profile_typecode, CLASS_NAMES, CLASS_OF_PROFILE, CLASS_REP_PROFILE_IDX, FALLBACK_NOISE_CLASS,
    FALLBACK_PROFILE_IDX, IS_JET, NUM_CLASSES, NUM_PROFILES, PROFILES,
};

pub use noise_compute::flight_id::{
    icao24_to_hex_lower, pack_real, pack_synth, parse_icao24_hex, unpack, FlightIdKind,
    SYNTHETIC_BIT,
};
