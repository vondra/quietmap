//! Canonical, injection-resistant parser for the 14-line H0 selection record.

use crate::h0_production_selection::{H0ProductionSelection, H0SelectionError};

const RECORD_DOC_LINE: &str =
    "/// Reviewed numerical authority for the selected production H0 line model.";
const RECORD_LINE_COUNT: usize = 14;

pub(crate) fn parse(source: &str) -> Result<H0ProductionSelection, H0SelectionError> {
    if source.contains('\r') {
        return Err(H0SelectionError::new(
            "selection record must use LF line endings",
        ));
    }
    if !source.ends_with('\n') {
        return Err(H0SelectionError::new(
            "selection record must end with one LF",
        ));
    }
    let lines: Vec<_> = source.lines().collect();
    if lines.len() != RECORD_LINE_COUNT {
        return Err(H0SelectionError::new(format!(
            "selection record has {} lines; expected {RECORD_LINE_COUNT}",
            lines.len()
        )));
    }
    if lines[0] != RECORD_DOC_LINE {
        return Err(H0SelectionError::new(format!(
            "selection record must begin with `{RECORD_DOC_LINE}`"
        )));
    }

    Ok(H0ProductionSelection {
        schema: parse_u32(field(lines[1], "H0_PRODUCTION_SELECTION_SCHEMA", "u32")?)?,
        epoch: parse_u64(field(lines[2], "H0_PRODUCTION_SELECTION_EPOCH", "u64")?)?,
        mechanism: parse_string(field(lines[3], "H0_PRODUCTION_MECHANISM", "&str")?)?,
        theta_degrees: parse_u8(field(lines[4], "H0_PRODUCTION_THETA_DEGREES", "u8")?)?,
        theta_radians_bits: parse_hex_u64(field(
            lines[5],
            "H0_PRODUCTION_THETA_RADIANS_BITS",
            "u64",
        )?)?,
        h_max: parse_usize(field(lines[6], "H0_PRODUCTION_H_MAX", "usize")?)?,
        node_cap: parse_usize(field(lines[7], "H0_PRODUCTION_NODE_CAP", "usize")?)?,
        v3_verdict_sha256: parse_string(field(
            lines[8],
            "H0_PRODUCTION_V3_VERDICT_SHA256",
            "&str",
        )?)?,
        v3_root_manifest_sha256: parse_string(field(
            lines[9],
            "H0_PRODUCTION_V3_ROOT_MANIFEST_SHA256",
            "&str",
        )?)?,
        v3_analyzer_sha256: parse_string(field(
            lines[10],
            "H0_PRODUCTION_V3_ANALYZER_SHA256",
            "&str",
        )?)?,
        streaming_design_sha256: parse_string(field(
            lines[11],
            "H0_PRODUCTION_STREAMING_DESIGN_SHA256",
            "&str",
        )?)?,
        model_v2_plan_sha256: parse_string(field(
            lines[12],
            "H0_PRODUCTION_MODEL_V2_PLAN_SHA256",
            "&str",
        )?)?,
        role_integration_design_sha256: parse_string(field(
            lines[13],
            "H0_PRODUCTION_ROLE_INTEGRATION_DESIGN_SHA256",
            "&str",
        )?)?,
    })
}

fn field<'a>(line: &'a str, name: &str, rust_type: &str) -> Result<&'a str, H0SelectionError> {
    let prefix = format!("pub const {name}: {rust_type} = ");
    line.strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(';'))
        .ok_or_else(|| {
            H0SelectionError::new(format!(
                "expected canonical `{prefix}<value>;`, got `{line}`"
            ))
        })
}

fn parse_u8(value: &str) -> Result<u8, H0SelectionError> {
    let parsed = parse_u64(value)?;
    u8::try_from(parsed).map_err(|_| H0SelectionError::new(format!("`{value}` is not a u8")))
}

fn parse_u32(value: &str) -> Result<u32, H0SelectionError> {
    let parsed = parse_u64(value)?;
    u32::try_from(parsed).map_err(|_| H0SelectionError::new(format!("`{value}` is not a u32")))
}

fn parse_usize(value: &str) -> Result<usize, H0SelectionError> {
    let parsed = parse_u64(value)?;
    usize::try_from(parsed).map_err(|_| H0SelectionError::new(format!("`{value}` is not a usize")))
}

fn parse_u64(value: &str) -> Result<u64, H0SelectionError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(H0SelectionError::new(format!(
            "`{value}` is not canonical decimal"
        )));
    }
    value
        .parse()
        .map_err(|error| H0SelectionError::new(format!("invalid decimal `{value}`: {error}")))
}

fn parse_hex_u64(value: &str) -> Result<u64, H0SelectionError> {
    let digits = value.strip_prefix("0x").ok_or_else(|| {
        H0SelectionError::new(format!("`{value}` is not canonical 0x-prefixed u64"))
    })?;
    if digits.len() != 16
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(H0SelectionError::new(format!(
            "`{value}` is not canonical 16-digit lowercase hexadecimal"
        )));
    }
    u64::from_str_radix(digits, 16)
        .map_err(|error| H0SelectionError::new(format!("invalid hexadecimal `{value}`: {error}")))
}

fn parse_string(value: &str) -> Result<String, H0SelectionError> {
    let inner = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| H0SelectionError::new(format!("`{value}` is not a string literal")))?;
    if inner.contains(['"', '\\']) {
        return Err(H0SelectionError::new(
            "selection strings may not contain escapes",
        ));
    }
    Ok(inner.to_owned())
}
