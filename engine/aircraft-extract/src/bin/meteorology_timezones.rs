//! Export the meteorology node timezone identities using the aircraft resolver.
use std::{collections::BTreeMap, io::Write};

fn main() -> anyhow::Result<()> {
    let mut zones = Vec::new();
    let mut lookup = BTreeMap::new();
    let mut indices = Vec::with_capacity(721 * 1440);
    for y in 0..721 {
        for x in 0..1440 {
            let longitude = (f64::from(x) * 0.25 + 180.0).rem_euclid(360.0) - 180.0;
            let name =
                aircraft_extract::period::resolve_tz(90.0 - f64::from(y) * 0.25, longitude).name();
            let next = u16::try_from(zones.len())?;
            let index = *lookup.entry(name).or_insert_with(|| {
                zones.push(name);
                next
            });
            indices.push(index);
        }
    }
    serde_json::to_writer(
        std::io::stdout().lock(),
        &serde_json::json!({"zones": zones, "indices": indices}),
    )?;
    std::io::stdout().flush()?;
    Ok(())
}
