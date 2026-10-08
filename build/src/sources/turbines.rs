//! The wind turbines standing today (`fetch/turbines.sh`: OpenStreetMap's, and those of the
//! national registers that OpenStreetMap lacks, each turbine once) by z9 square, each a point
//! source at its hub; dev4's turbine rows are silent (`industry`).

use super::cells::point_piece;
use super::{Converted, group_key};
use crate::dev4::{Square, degrees_to_z30};
use physics::emission::wind::{TURBINE_MAXIMUM_PLAUSIBLE_POWER_KW, turbine_sound_power};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// Hubs: the known-data median when unknown; taller ones are errors (dev4 audit I-10b).
const DEFAULT_HUB_HEIGHT_M: f64 = 105.0;
const MAXIMUM_HUB_HEIGHT_M: f64 = 175.0;

/// One standing turbine: hub height (m) and rated power (kW), 0 when unknown, and the input that
/// placed it (`osm` or a register).
#[derive(Debug, Clone, PartialEq)]
pub struct Turbine {
    pub lat: f64,
    pub lon: f64,
    pub hub_m: f64,
    pub power_kw: f64,
    pub source: String,
}

/// The standing turbines by z9 square.
pub struct Turbines {
    by_square: HashMap<(u32, u32), Vec<Turbine>>,
}

impl Turbines {
    /// Reads `turbines.txt`: `lat lon hub_m power_kw source` per line (tab-separated).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut by_square: HashMap<(u32, u32), Vec<Turbine>> = HashMap::new();
        for (number, line) in text.lines().enumerate() {
            let bad = |error: &dyn std::fmt::Display| format!("line {}: {error}", number + 1);
            let [lat, lon, hub, power, source] = line.split('\t').collect::<Vec<_>>()[..] else {
                return Err(bad(&"expected five fields"));
            };
            let number = |field: &str| field.parse::<f64>().map_err(|error| bad(&error));
            let turbine = Turbine {
                lat: number(lat)?,
                lon: number(lon)?,
                hub_m: number(hub)?,
                power_kw: number(power)?,
                source: source.to_string(),
            };
            if !(turbine.lat.abs() <= 90.0 && turbine.lon.abs() <= 180.0) {
                return Err(bad(&"no place on Earth"));
            }
            let (gx, gy) = degrees_to_z30(turbine.lat, turbine.lon);
            let square = Square::of_z30(gx, gy);
            by_square
                .entry((square.x, square.y))
                .or_default()
                .push(turbine);
        }
        Ok(Turbines { by_square })
    }
}

/// The turbines of `square` as point sources at their hubs (105 m when unknown, at most 175 m),
/// rated powers over 8 MW unknown; returns how many.
pub fn convert(turbines: &Turbines, square: Square, out: &mut Vec<Converted>) -> usize {
    let standing = turbines
        .by_square
        .get(&(square.x, square.y))
        .map_or(&[][..], Vec::as_slice);
    for turbine in standing {
        let power_kw = Some(turbine.power_kw)
            .filter(|kw| *kw > 0.0 && *kw <= TURBINE_MAXIMUM_PLAUSIBLE_POWER_KW);
        let hub_m = if turbine.hub_m > 0.0 {
            turbine.hub_m.min(MAXIMUM_HUB_HEIGHT_M)
        } else {
            DEFAULT_HUB_HEIGHT_M
        };
        let sound = turbine_sound_power(power_kw);
        // The industry layer's display fields (`tiles/src/sources.rs`).
        let display = json!([
            "",
            "wind_turbine",
            0.0,
            null,
            1,
            hub_m,
            power_kw,
            (sound.day_dba * 10.0).round() / 10.0,
            0
        ]);
        let place = format!("{:.6},{:.6}", turbine.lat, turbine.lon);
        let (tile, ends) = point_piece(turbine.lat, turbine.lon);
        out.push(Converted {
            tile,
            ends,
            attribute: Attribute {
                layer: Layer::Industry,
                height_m: hub_m,
                ground_percent: GROUND_FROM_TERRAIN,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: group_key(&["turbine", &place]),
                emission: sound.band_levels_db(),
                display: display.to_string(),
            },
        });
    }
    standing.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turbine stands at its hub with its rating's sound; an unknown hub stands at 105 m, a hub
    /// tag error clamps to 175 m, a rating over 8 MW is unknown (the fleet's median sound).
    #[test]
    fn turbines_stand_at_their_hubs() {
        let turbines = Turbines::parse(
            "50.0\t14.0\t120\t3000\tuswtdb\n50.0001\t14.0\t0\t20000\tosm\n50.0002\t14.0\t250\t0\tosm\n",
        )
        .unwrap();
        let (gx, gy) = degrees_to_z30(50.0, 14.0);
        let mut out = Vec::new();
        assert_eq!(convert(&turbines, Square::of_z30(gx, gy), &mut out), 3);
        let heights: Vec<f64> = out.iter().map(|item| item.attribute.height_m).collect();
        assert_eq!(heights, vec![120.0, 105.0, 175.0]);
        let unknown = turbine_sound_power(None).band_levels_db();
        assert_eq!(out[1].attribute.emission, unknown);
        assert_ne!(out[0].attribute.emission, unknown);
        assert!(Turbines::parse("50\t14\t0\t0\n").is_err());
    }
}
