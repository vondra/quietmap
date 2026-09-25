//! Extracted source evidence versions shared by producers and native consumers.

use arrow::datatypes::Schema;

/// Spill serialization and semantics: changing this invalidates completed scratch.
pub const EXTRACT_FORMAT: u32 = 2;
pub const ROADS_CONTRACT: &str = "2";
pub const RAILWAYS_CONTRACT: &str = "2";
pub const INDUSTRIAL_CONTRACT: &str = "2";
pub const TRANSPORT_NODES_CONTRACT: &str = "1";
pub const LEISURE_CONTRACT_V5: &str = "leisure_v5";

pub fn contract(family: &str) -> Option<(&'static str, &'static str)> {
    Some(match family {
        "roads" => ("osm_roads_contract", ROADS_CONTRACT),
        "railways" => ("osm_railways_contract", RAILWAYS_CONTRACT),
        "industrial" => ("osm_industrial_contract", INDUSTRIAL_CONTRACT),
        "leisure" => ("leisure_contract", LEISURE_CONTRACT_V5),
        "transport_nodes" => ("transport_nodes_contract", TRANSPORT_NODES_CONTRACT),
        _ => return None,
    })
}

pub fn validate(schema: &Schema, family: &str) -> Result<(), String> {
    let Some((key, expected)) = contract(family) else {
        return Ok(());
    };
    if schema.metadata().get(key).map(String::as_str) != Some(expected) {
        return Err(format!(
            "{family} requires {key}={expected}; re-extract OSM"
        ));
    }
    Ok(())
}
