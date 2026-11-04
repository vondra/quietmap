//! Express the measured section priors in the common preparation model, with no later country or lane fit.

use super::write;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub fn write_model(
    template: &Value,
    inputs: &crate::year::YearInputs,
    directory: &Path,
    rows: &[[f64; 3]; 6],
    weekday_factor: f64,
) -> Result<()> {
    let mut result = template.clone();
    let legal =
        crate::source_tables::CountryTable::<[u8; 4]>::read(inputs, crate::year::Kind::LegalSpeeds)
            .map_err(anyhow::Error::msg)?;
    let calibration = inputs
        .manifest
        .inputs
        .iter()
        .find(|input| input.kind == crate::year::Kind::RoadCalibration)
        .unwrap();
    ensure!(
        inputs
            .manifest
            .inputs
            .iter()
            .filter(|input| input.kind == crate::year::Kind::LegalSpeeds)
            .all(|input| calibration.depends_on.contains(&input.id)),
        "road calibration omits its legal-speed dependency"
    );
    for (country, speeds) in result["speed_transitions"]["countries"]
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("missing transition countries"))?
    {
        let code = country.parse::<u16>()?.to_le_bytes();
        *speeds = json!(legal.get(code).ok_or_else(|| anyhow::anyhow!(
            "no admitted legal speeds for transition country {country}"
        ))?);
    }
    let motorcycle = result["demand"]["world"][0]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("missing admitted fleet share"))?;
    let world = (0..13)
        .map(|class| {
            // Local subtypes share the observed local-road population; links share their parent class.
            let parent = if class >= 10 {
                class - 10
            } else {
                class.min(5)
            };
            let [total, medium, heavy] = rows[parent];
            let fractions = [
                1.0 - medium / 100.0 - heavy / 100.0 - motorcycle,
                medium / 100.0,
                heavy / 100.0,
                motorcycle,
            ];
            ensure!(
                fractions.iter().all(|v| v.is_finite() && *v >= 0.0),
                "incompatible admitted class shares"
            );
            Ok(fractions.map(|v| v * total))
        })
        .collect::<Result<Vec<_>>>()?;
    result["world"] = json!(world);
    result["carriageways"] = json!([]);
    result["regions"] = json!([]);
    result["lanes"] = json!([]);
    result["one_way_section_classes"] = json!([]);
    result["weekday_to_annual_factor"] = json!(weekday_factor);
    let model: crate::roads::model::Model = serde_json::from_value(result.clone())?;
    model.validate()?;
    write(directory, "road-preparation-model", &result)
}
