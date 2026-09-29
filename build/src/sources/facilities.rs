//! Industrial facility evidence (dev4 `osm_evidence.rs`), joined when tiles are built instead of
//! at the click: power values of the retained OSM tags, the transformers inside any part of a
//! substation facility (one rating truth per facility) and the solar plants whose own generators
//! stay silent. Every value has an untagged fallback: a missing or unparseable tag is untagged.

use super::cells::PreparedRing;
use std::collections::BTreeMap;

/// A row's retained OSM tags.
pub type Tags = BTreeMap<String, String>;

/// Tags of an `osm_tags` JSON object; an unparseable blob reads as untagged.
pub fn parse_tags(json: &str) -> Tags {
    serde_json::from_str(json).unwrap_or_default()
}

/// `<number>[ <unit>]` with a decimal comma and a leading repeat (`2x630kVA` is 1,260 kVA); a bare
/// number takes `bare_scale` (`None`: no documented unit, rejected).
fn scaled_value(text: &str, bare_scale: Option<f64>, units: &[(&str, f64)]) -> Option<f64> {
    let compact = text.trim().replace(',', ".");
    let (repeat, compact) = match compact.split_once(['x', 'X']) {
        Some((count, rest)) if !count.trim().is_empty() && !rest.trim().is_empty() => {
            match count.trim().parse::<f64>() {
                Ok(count) if count.is_finite() && count > 0.0 => (count, rest.trim().to_string()),
                _ => return None,
            }
        }
        _ => (1.0, compact),
    };
    let split = compact
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E')))
        .unwrap_or(compact.len());
    let number: f64 = compact[..split].trim().parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = compact[split..].trim().to_ascii_lowercase();
    let scale = if unit.is_empty() {
        bare_scale?
    } else {
        units
            .iter()
            .find(|(name, _)| unit == *name)
            .map(|(_, scale)| *scale)?
    };
    Some(number * scale * repeat)
}

/// The largest value of a `;`-separated list (`/` also separates cooling stages, `25/33 MVA`).
fn largest_listed(text: &str, parse: impl Fn(&str) -> Option<f64>, slash: bool) -> Option<f64> {
    text.split(';')
        .flat_map(|part| {
            if slash {
                part.split('/').collect()
            } else {
                vec![part]
            }
        })
        .filter_map(parse)
        .reduce(f64::max)
}

const POWER_UNITS: &[(&str, f64)] = &[
    ("w", 1e-6),
    ("kw", 1e-3),
    ("mw", 1.0),
    ("gw", 1e3),
    ("va", 1e-6),
    ("kva", 1e-3),
    ("mva", 1.0),
    ("gva", 1e3),
];

/// Plant nameplate MW of `plant:output:electricity` (a bare number is MW, OSM wiki); a list
/// takes its largest value, never the sum.
pub fn plant_output_mw(tags: &Tags) -> Option<f64> {
    let text = tags.get("plant:output:electricity")?;
    largest_listed(
        text,
        |part| scaled_value(part, Some(1.0), POWER_UNITS),
        false,
    )
    .filter(|mw| *mw > 0.0)
}

/// Transformer nameplate MVA of `rating`; a bare number has no documented unit and is unknown.
pub fn transformer_rating_mva(tags: &Tags) -> Option<f64> {
    let text = tags.get("rating")?;
    largest_listed(text, |part| scaled_value(part, None, POWER_UNITS), true)
        .filter(|mva| *mva > 0.0)
}

/// Highest `voltage` in kV (a bare number is volts, documented OSM practice).
pub fn maximum_voltage_kv(tags: &Tags) -> Option<f64> {
    const UNITS: &[(&str, f64)] = &[("v", 1e-3), ("kv", 1.0), ("mv", 1e3)];
    let text = tags.get("voltage")?;
    largest_listed(text, |part| scaled_value(part, Some(1e-3), UNITS), false).filter(|kv| *kv > 0.0)
}

/// The legacy `transformer=auto` tagging (`windings:auto` is not retained by the extractor).
pub fn is_autotransformer(tags: &Tags) -> bool {
    let value: String = tags.get("transformer").map_or(String::new(), |value| {
        value
            .to_ascii_lowercase()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect()
    });
    matches!(
        value.as_str(),
        "auto" | "autotransformer" | "autotransformator"
    )
}

/// A gas-network station: no transformer hum, silent until a gas model claims it.
pub fn is_gas_substation(tags: &Tags) -> bool {
    matches!(
        tags.get("substation").map(String::as_str),
        Some("gas" | "valve_group" | "valve" | "compression")
    )
}

/// A class-13 row that is the plant itself (`power=plant`), not one of its generators.
pub fn is_solar_plant(tags: &Tags) -> bool {
    tags.get("power").map(String::as_str) == Some("plant")
}

/// The extractor's area share of one part of a multi-part facility (`qm:facility_share`, not an
/// OSM tag) inside (0, 1]; the whole facility otherwise.
pub fn facility_share(tags: &Tags) -> f64 {
    tags.get("qm:facility_share")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|share| *share > 0.0 && *share <= 1.0)
        .unwrap_or(1.0)
}

/// What a substation facility contains: the summed transformer ratings and the class evidence.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SubstationFeed {
    pub rated_mva_sum: f64,
    pub rated_count: usize,
    pub has_autotransformer: bool,
    pub maximum_contained_voltage_kv: Option<f64>,
}

/// A substation's MVA and class: the joined ratings when any unit carries one, else the row's own
/// `rating` (the facility nameplate); the class auto (2) by architecture, else main (1) when
/// tagged transmission, minor (4) when minor_distribution under 60 kV, main at 220 kV and above,
/// else distribution (3).
pub fn substation_power(own_tags: &Tags, feed: &SubstationFeed) -> (Option<f64>, u8) {
    let mva = (feed.rated_count > 0 && feed.rated_mva_sum > 0.0)
        .then_some(feed.rated_mva_sum)
        .or_else(|| transformer_rating_mva(own_tags));
    let kv = maximum_voltage_kv(own_tags)
        .into_iter()
        .chain(feed.maximum_contained_voltage_kv)
        .fold(0.0f64, f64::max);
    let tagged = |value: &str| {
        own_tags.get("substation").is_some_and(|tag| {
            tag.split(';')
                .any(|token| token.trim().eq_ignore_ascii_case(value))
        })
    };
    let class = if is_autotransformer(own_tags) || feed.has_autotransformer {
        2
    } else if tagged("transmission") {
        1
    } else if tagged("minor_distribution") && kv < 60.0 {
        4
    } else if kv >= 220.0 {
        1
    } else {
        3
    };
    (mva, class)
}

struct TransformerUnit {
    centroid: (i32, i32),
    rating_mva: Option<f64>,
    auto: bool,
    voltage_kv: Option<f64>,
}

/// The facility joins over the industrial rows of a square and its neighbours.
#[derive(Default)]
pub struct FacilityJoins {
    transformers: Vec<TransformerUnit>,
    solar_plants: Vec<PreparedRing>,
    /// Every closed part of a multipolygon relation shares its (OSM kind, id).
    substation_parts: BTreeMap<(String, i64), Vec<PreparedRing>>,
}

impl FacilityJoins {
    /// Indexes one row: a transformer (15) at its centroid, a solar plant polygon (13 tagged
    /// `power=plant`), a substation part polygon (14, by OSM kind and id).
    pub fn add_row(
        &mut self,
        source_type: u8,
        osm: (&str, i64),
        centroid: (i32, i32),
        ring: &[(i32, i32)],
        tags: &Tags,
    ) {
        match source_type {
            15 => self.transformers.push(TransformerUnit {
                centroid,
                rating_mva: transformer_rating_mva(tags),
                auto: is_autotransformer(tags),
                voltage_kv: maximum_voltage_kv(tags),
            }),
            13 if is_solar_plant(tags) => self.solar_plants.extend(PreparedRing::new(ring)),
            14 if osm.1 != 0 => {
                if let Some(part) = PreparedRing::new(ring) {
                    self.substation_parts
                        .entry((osm.0.to_string(), osm.1))
                        .or_default()
                        .push(part);
                }
            }
            _ => {}
        }
    }

    /// Whether a generator at `centroid` lies inside a solar plant: the plant owns the emission.
    pub fn inside_solar_plant(&self, centroid: (i32, i32)) -> bool {
        self.solar_plants
            .iter()
            .any(|plant| plant.contains(centroid))
    }

    /// The units inside any part of the substation's facility (each counted once), else inside
    /// its own ring; a node substation contains nothing.
    pub fn substation_feed(&self, osm: (&str, i64), own_ring: &[(i32, i32)]) -> SubstationFeed {
        let own: Vec<PreparedRing> = PreparedRing::new(own_ring).into_iter().collect();
        let parts = self
            .substation_parts
            .get(&(osm.0.to_string(), osm.1))
            .unwrap_or(&own);
        let mut feed = SubstationFeed::default();
        for unit in &self.transformers {
            if !parts.iter().any(|part| part.contains(unit.centroid)) {
                continue;
            }
            if let Some(rating) = unit.rating_mva {
                feed.rated_mva_sum += rating;
                feed.rated_count += 1;
            }
            feed.has_autotransformer |= unit.auto;
            feed.maximum_contained_voltage_kv = feed
                .maximum_contained_voltage_kv
                .into_iter()
                .chain(unit.voltage_kv)
                .reduce(f64::max);
        }
        feed
    }
}

#[cfg(test)]
#[path = "facilities_tests.rs"]
mod tests;
