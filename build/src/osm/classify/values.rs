//! Parse explicit OSM widths and speeds without choosing a country or legal edition.

/// Parse OSM `width` tag to metres. Returns max across multi-stripe /
/// range fragments. Examples: `45;35;50` → 50, `45-50` → 50,
/// `45,5` → 45.5 (EU decimal), `147 ft` → 44.8.
pub fn parse_width_m(width: Option<&str>) -> Option<f32> {
    let raw = width?.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let parse_fragment = |s: &str| -> Option<f32> {
        let s = s.trim();
        let (numeric_part, is_feet) =
            if let Some(stripped) = s.strip_suffix("ft").or_else(|| s.strip_suffix(" ft")) {
                (stripped.trim(), true)
            } else if let Some(stripped) = s.strip_suffix("m").or_else(|| s.strip_suffix(" m")) {
                (stripped.trim(), false)
            } else {
                (s, false)
            };
        // Require a leading digit so non-numeric fragments like "ft" (from
        // splitting "60-ft" → ["60", "ft"]) return None instead of 0.
        if !numeric_part.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let numeric: String = numeric_part
            .chars()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ','))
            .collect();
        let value: f32 = numeric.replace(',', ".").parse().ok()?;
        Some(if is_feet { value * 0.3048 } else { value })
    };

    // Split on `;` (multi-stripe) and `-` (range). NOT `,` — that's the
    // EU decimal separator (`45,5` = 45.5 m, not max(45, 5)).
    lower
        .split([';', '-'])
        .filter_map(parse_fragment)
        .reduce(f32::max)
}

/// `roads.arrow` keeps `speed_limit` as u8; `maxspeed=none` (derestricted,
/// e.g. German Autobahn) stores this sentinel and real limits clamp to 254
/// in `spill.rs` so they can never collide. Owned by the future
/// `noise-compute` transfer (see [`ids`]); duplicated here alone.
pub(crate) use crate::osm::ids::SPEED_LIMIT_DERESTRICTED;

/// [`parse_maxspeed_kmh`] result for `maxspeed=none`. Outside the clamped
/// numeric range (≤ 400) by construction, so no real tag value can
/// produce it.
pub const MAXSPEED_NONE: u16 = u16::MAX;

/// Parse OSM `maxspeed` to km/h. Multi-value tags take the first
/// `;`-token (`"50;30"` → 50; conditional/lane variants are out of scope).
/// Units: bare number = km/h, `mph`, `knots`. `walk` → 10 km/h
/// (OSM-wiki convention for walking pace); `none` → [`MAXSPEED_NONE`];
/// legal zone/country tokens are resolved separately from manifest inputs;
/// `signals` / `variable` / garbage → 0 (unknown → class
/// default downstream). Numeric values clamp to 400 km/h (above any
/// legal limit; guards typos like "999").
pub fn parse_maxspeed_kmh(raw: &str) -> u16 {
    let token = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match token.as_str() {
        "none" => return MAXSPEED_NONE,
        "walk" => return 10,
        _ => {}
    }
    let numeric: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let Ok(value) = numeric.parse::<f64>() else {
        return 0; // "", "signals", "variable", other garbage
    };
    let kmh = match token[numeric.len()..].trim() {
        "" | "km/h" | "kmh" | "kph" => value,
        "mph" => value * 1.609344,
        "knots" => value * 1.852,
        _ => return 0, // unknown unit
    };
    (kmh.round() as u32).min(400) as u16
}
