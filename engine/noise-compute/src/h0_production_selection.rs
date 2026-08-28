//! Strict parser and theorem verifier for the checked-in H0 production selection.
//!
//! Owner 2026-08-28: the deleted `h0_v3_*` / `h0-v3-analyze` campaign sources
//! stay deleted. Regenerating the sealed evidence record is the over-engineering
//! this cleanup removed; production `v2-h0` is this module, not those binaries.

use std::fmt;

pub const H0_PRODUCTION_SELECTION_SCHEMA: u32 = 1;
pub const H0_PRODUCTION_MECHANISM: &str = "H0StreamingReduction";
pub const H0_STREAMING_DESIGN_SHA256: &str =
    "382ee570373553c7007ef4a477ffc6951d407daccff0d8725f0582d0e857605b";
pub const MODEL_V2_PLAN_SHA256: &str =
    "65ce0074a3b9bfc8a62293a093a3ebfc1b5216d07c7f14c3fae4e68ecdc09dba";
pub const H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256: &str =
    "44b606ca8fb8c5fd0b4f81d3e81c103ed0d45f495fa173d4f0760b791c939b1e";
pub const H0_PRODUCTION_SELECTION_RECORD_PATH: &str = "src/h0_production_selection_record.rs";

const LINE_MAX_LENGTH_M: f64 = 250.0;
const R_ATM_BASE_M_PER_RAD: f64 = 1_000.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H0ThetaCapPair {
    pub theta_degrees: u8,
    pub theta_radians_bits: u64,
    pub node_cap: usize,
}

pub const H0_THETA_CAP_PAIRS: [H0ThetaCapPair; 4] = [
    H0ThetaCapPair {
        theta_degrees: 5,
        theta_radians_bits: (core::f64::consts::PI / 36.0).to_bits(),
        node_cap: 40,
    },
    H0ThetaCapPair {
        theta_degrees: 4,
        theta_radians_bits: (core::f64::consts::PI / 45.0).to_bits(),
        node_cap: 50,
    },
    H0ThetaCapPair {
        theta_degrees: 3,
        theta_radians_bits: (core::f64::consts::PI / 60.0).to_bits(),
        node_cap: 66,
    },
    H0ThetaCapPair {
        theta_degrees: 2,
        theta_radians_bits: (core::f64::consts::PI / 90.0).to_bits(),
        node_cap: 99,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct H0ProductionSelection {
    pub schema: u32,
    pub epoch: u64,
    pub mechanism: String,
    pub theta_degrees: u8,
    pub theta_radians_bits: u64,
    pub h_max: usize,
    pub node_cap: usize,
    pub v3_verdict_sha256: String,
    pub v3_root_manifest_sha256: String,
    pub v3_analyzer_sha256: String,
    pub streaming_design_sha256: String,
    pub model_v2_plan_sha256: String,
    pub role_integration_design_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct H0SelectionError(String);

impl H0SelectionError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for H0SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for H0SelectionError {}

impl H0ProductionSelection {
    pub fn parse_and_verify(source: &str) -> Result<Self, H0SelectionError> {
        let selection = crate::h0_production_selection_parser::parse(source)?;
        selection.verify()?;
        Ok(selection)
    }

    pub fn verify(&self) -> Result<(), H0SelectionError> {
        if self.schema != H0_PRODUCTION_SELECTION_SCHEMA {
            return Err(H0SelectionError::new(format!(
                "selection schema {} is not {H0_PRODUCTION_SELECTION_SCHEMA}",
                self.schema
            )));
        }
        if self.epoch == 0 {
            return Err(H0SelectionError::new(
                "selection epoch zero is reserved for no selection",
            ));
        }
        if self.mechanism != H0_PRODUCTION_MECHANISM {
            return Err(H0SelectionError::new(format!(
                "selection mechanism {:?} is not {H0_PRODUCTION_MECHANISM}",
                self.mechanism
            )));
        }
        if self.h_max != 0 {
            return Err(H0SelectionError::new(format!(
                "H0 production selection requires h_max=0, got {}",
                self.h_max
            )));
        }
        let pair = theta_cap_pair(self.theta_degrees).ok_or_else(|| {
            H0SelectionError::new(format!(
                "theta {} degrees is outside the reviewed 5/4/3/2 set",
                self.theta_degrees
            ))
        })?;
        if self.theta_radians_bits != pair.theta_radians_bits {
            return Err(H0SelectionError::new(format!(
                "theta bits 0x{:016x} do not equal PI/{} bits 0x{:016x}",
                self.theta_radians_bits,
                degrees_denominator(self.theta_degrees),
                pair.theta_radians_bits
            )));
        }
        let derived = derived_node_cap(f64::from_bits(self.theta_radians_bits));
        if derived != pair.node_cap || self.node_cap != derived {
            return Err(H0SelectionError::new(format!(
                "node cap {} does not equal reviewed theorem cap {derived} for {} degrees",
                self.node_cap, self.theta_degrees
            )));
        }
        verify_sha256("V3 verdict", &self.v3_verdict_sha256, None)?;
        verify_sha256("V3 root manifest", &self.v3_root_manifest_sha256, None)?;
        verify_sha256("V3 analyzer", &self.v3_analyzer_sha256, None)?;
        verify_sha256(
            "streaming design",
            &self.streaming_design_sha256,
            Some(H0_STREAMING_DESIGN_SHA256),
        )?;
        verify_sha256(
            "model V2 plan",
            &self.model_v2_plan_sha256,
            Some(MODEL_V2_PLAN_SHA256),
        )?;
        verify_sha256(
            "H0 production-role integration design",
            &self.role_integration_design_sha256,
            Some(H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256),
        )?;
        Ok(())
    }

    #[must_use]
    pub fn theta_radians(&self) -> f64 {
        f64::from_bits(self.theta_radians_bits)
    }

    #[must_use]
    pub fn render_build_receipt(&self) -> String {
        format!(
            "selection_schema={}\nselection_epoch={}\nmechanism={}\ntheta_degrees={}\n\
             theta_radians_bits=0x{:016x}\nh_max={}\nnode_cap={}\n\
             v3_verdict_sha256={}\nv3_root_manifest_sha256={}\nv3_analyzer_sha256={}\n\
             streaming_design_sha256={}\nmodel_v2_plan_sha256={}\n\
             role_integration_design_sha256={}\n",
            self.schema,
            self.epoch,
            self.mechanism,
            self.theta_degrees,
            self.theta_radians_bits,
            self.h_max,
            self.node_cap,
            self.v3_verdict_sha256,
            self.v3_root_manifest_sha256,
            self.v3_analyzer_sha256,
            self.streaming_design_sha256,
            self.model_v2_plan_sha256,
            self.role_integration_design_sha256,
        )
    }
}

#[must_use]
pub fn theta_cap_pair(theta_degrees: u8) -> Option<H0ThetaCapPair> {
    H0_THETA_CAP_PAIRS
        .iter()
        .copied()
        .find(|pair| pair.theta_degrees == theta_degrees)
}

#[must_use]
pub fn derived_node_cap(theta_radians: f64) -> usize {
    (core::f64::consts::PI / theta_radians).ceil() as usize
        + (LINE_MAX_LENGTH_M / (theta_radians * R_ATM_BASE_M_PER_RAD)).floor() as usize
        + 2
}

fn degrees_denominator(theta_degrees: u8) -> u16 {
    180 / u16::from(theta_degrees)
}

fn verify_sha256(label: &str, value: &str, exact: Option<&str>) -> Result<(), H0SelectionError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || value.bytes().all(|byte| byte == b'0')
    {
        return Err(H0SelectionError::new(format!(
            "{label} SHA-256 must be 64 lowercase nonzero hexadecimal digits"
        )));
    }
    if let Some(expected) = exact {
        if value != expected {
            return Err(H0SelectionError::new(format!(
                "{label} SHA-256 `{value}` is not sealed authority `{expected}`"
            )));
        }
    }
    Ok(())
}
