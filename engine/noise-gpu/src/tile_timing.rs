//! Finite machine-readable timing records for GPU surface tiles.

use serde::{Deserialize, Serialize};

/// Whether CUDA events measured the isolated kernel duration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KernelMsStatus {
    Available,
    Unavailable,
}

/// One tile's host wall plus its optional CUDA-event duration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileTimingRecord {
    pub wall_ms: f64,
    pub kernel_ms: Option<f64>,
    pub kernel_ms_status: KernelMsStatus,
}

impl TileTimingRecord {
    /// Build a record, rejecting non-finite values and a fabricated zero kernel duration.
    pub fn new(wall_ms: f64, kernel_ms: Option<f64>) -> Result<Self, &'static str> {
        let kernel_ms_status = match kernel_ms {
            Some(value) if value.is_finite() && value > 0.0 => KernelMsStatus::Available,
            Some(_) => return Err("kernel_ms must be finite and positive when available"),
            None => KernelMsStatus::Unavailable,
        };
        let record = Self {
            wall_ms,
            kernel_ms,
            kernel_ms_status,
        };
        record.validate()?;
        Ok(record)
    }

    /// Parse and validate one compact JSON object from renderer evidence.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let record: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        record.validate().map_err(str::to_string)?;
        Ok(record)
    }

    /// Serialize only after rechecking the status/value contract.
    pub fn to_json(&self) -> Result<String, String> {
        self.validate().map_err(str::to_string)?;
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    fn validate(&self) -> Result<(), &'static str> {
        if !self.wall_ms.is_finite() || self.wall_ms < 0.0 {
            return Err("wall_ms must be finite and nonnegative");
        }
        match (self.kernel_ms_status, self.kernel_ms) {
            (KernelMsStatus::Unavailable, None) => Ok(()),
            (KernelMsStatus::Available, Some(value)) if value.is_finite() && value > 0.0 => Ok(()),
            (KernelMsStatus::Available, Some(_)) => {
                Err("available kernel_ms must be finite and positive")
            }
            _ => Err("kernel_ms value and status disagree"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_timing_has_a_stable_null_golden_record() {
        let record = TileTimingRecord::new(1234.0, None).unwrap();
        let json = record.to_json().unwrap();
        assert_eq!(
            json,
            r#"{"wall_ms":1234.0,"kernel_ms":null,"kernel_ms_status":"unavailable"}"#
        );
        assert_eq!(TileTimingRecord::from_json(&json).unwrap(), record);
    }

    #[test]
    fn available_timing_has_a_stable_finite_golden_record() {
        let record = TileTimingRecord::new(1234.0, Some(1200.25)).unwrap();
        let json = record.to_json().unwrap();
        assert_eq!(
            json,
            r#"{"wall_ms":1234.0,"kernel_ms":1200.25,"kernel_ms_status":"available"}"#
        );
        assert_eq!(TileTimingRecord::from_json(&json).unwrap(), record);
    }

    #[test]
    fn invalid_numbers_and_fabricated_zero_are_rejected() {
        for wall_ms in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            assert!(TileTimingRecord::new(wall_ms, None).is_err());
        }
        for kernel_ms in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(TileTimingRecord::new(1.0, Some(kernel_ms)).is_err());
        }
        for invalid_json in [
            r#"{"wall_ms":NaN,"kernel_ms":null,"kernel_ms_status":"unavailable"}"#,
            r#"{"wall_ms":Infinity,"kernel_ms":null,"kernel_ms_status":"unavailable"}"#,
            r#"{"wall_ms":1.0,"kernel_ms":NaN,"kernel_ms_status":"available"}"#,
            r#"{"wall_ms":1.0,"kernel_ms":Infinity,"kernel_ms_status":"available"}"#,
            r#"{"wall_ms":1.0,"kernel_ms":0.0,"kernel_ms_status":"available"}"#,
        ] {
            assert!(TileTimingRecord::from_json(invalid_json).is_err());
        }
    }

    #[test]
    fn status_value_disagreement_and_unknown_fields_are_rejected() {
        for invalid_json in [
            r#"{"wall_ms":1.0,"kernel_ms":null,"kernel_ms_status":"available"}"#,
            r#"{"wall_ms":1.0,"kernel_ms":2.0,"kernel_ms_status":"unavailable"}"#,
            r#"{"wall_ms":1.0,"kernel_ms":null,"kernel_ms_status":"unavailable","extra":0}"#,
        ] {
            assert!(TileTimingRecord::from_json(invalid_json).is_err());
        }
    }
}
