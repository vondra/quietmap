//! Normalize manifest-selected GPPD, E-PRTR, GEM and national records into one facility schema.

#[path = "facilities_classify.rs"]
mod classify;
#[path = "facilities_manifest.rs"]
mod manifest;
#[path = "facilities_read.rs"]
mod read;
use read::features;
#[path = "facilities_dates.rs"]
mod dates;
use crate::{
    facilities::{
        model::{InputSpec, Model},
        records::{self, Facility, REGISTRY_AREA, REGISTRY_POINT},
    },
    osm::geography::Regions,
    year::{Kind, YearInputs},
};
use anyhow::{Context, Result, ensure};
use geos::{Geom, Geometry};
#[path = "facilities_geometry.rs"]
mod geometry;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

fn normalize(
    spec: &InputSpec,
    input: &crate::year::Input,
    raw: Vec<Value>,
    regions: &Regions,
    seen: &mut HashSet<String>,
) -> Result<Vec<Facility>> {
    let observed = dates::observation_date(input)?;
    let status = spec
        .status
        .as_ref()
        .map(|s| {
            regex::RegexBuilder::new(&s.pattern)
                .case_insensitive(true)
                .build()
        })
        .transpose()?;
    let inactive = spec
        .status
        .as_ref()
        .filter(|rule| !rule.inactive_pattern.is_empty())
        .map(|rule| {
            regex::RegexBuilder::new(&rule.inactive_pattern)
                .case_insensitive(true)
                .build()
        })
        .transpose()?;
    let operating = spec
        .status
        .as_ref()
        .filter(|s| !s.operating_pattern.is_empty())
        .map(|s| {
            regex::RegexBuilder::new(&s.operating_pattern)
                .case_insensitive(true)
                .build()
        })
        .transpose()?;
    let border = spec
        .border_names
        .as_ref()
        .map(|s| regex::RegexBuilder::new(s).case_insensitive(true).build())
        .transpose()?;
    let mut rows = Vec::new();
    let mut observation_ids = HashSet::new();
    for feature in raw {
        let properties = if matches!(spec.format.as_str(), "gppd-csv" | "eprtr-json") {
            &feature
        } else {
            &feature["properties"]
        };
        ensure!(
            properties.is_object(),
            "facility properties must be an object"
        );
        let Some(geometry::Location {
            center: [longitude, latitude],
            polygon,
        }) = geometry::location(&feature, properties, spec)?
        else {
            continue;
        };
        if !longitude.is_finite()
            || !latitude.is_finite()
            || longitude.abs() > 180.0
            || latitude.abs() > 90.0
        {
            continue;
        }
        let country = regions.resolve(latitude, longitude, false).country;
        let name = classify::field(properties, &spec.names);
        if !spec.containment
            && spec
                .scope
                .as_ref()
                .is_some_and(|s| !s.area_contains(latitude, longitude, country))
            && !border
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(&name))
        {
            continue;
        }
        if spec.point_land
            && spec
                .scope
                .as_ref()
                .is_some_and(|s| !s.countries.contains(&country))
        {
            continue;
        }
        let active = match (&spec.status, &status) {
            (Some(rule), Some(pattern)) => {
                let value = classify::field(properties, &rule.fields);
                (rule.blank && value.is_empty()) || pattern.is_match(&value)
            }
            _ => true,
        };
        // An unbuilt project is not a closure of the mapped factory containing its proposed point.
        let operating_state = if operating.as_ref().is_some_and(|pattern| {
            pattern.is_match(&classify::field(
                properties,
                &spec.status.as_ref().unwrap().fields,
            ))
        }) {
            1
        } else if inactive.as_ref().is_some_and(|pattern| {
            pattern.is_match(&classify::field(
                properties,
                &spec.status.as_ref().unwrap().fields,
            ))
        }) {
            2
        } else {
            0
        };
        let voltage = classify::number(&classify::field(properties, &spec.voltage_fields));
        if spec.minimum_voltage.is_some_and(|minimum| {
            voltage.is_some_and(|v| (!spec.voltage_positive_only || v > 0.0) && v < minimum)
        }) {
            continue;
        }
        let activity = classify::activity(&spec.classifier, properties, spec.activity)?;
        if let Some(digits) = spec.deduplicate_digits
            && active
        {
            let key = format!("{latitude:.digits$}_{longitude:.digits$}");
            if !seen.insert(key) {
                continue;
            }
        }
        let identity = classify::field(properties, &spec.identities);
        let identity = if identity.is_empty() {
            // Older exports omit IDs: a content identity survives reordering, but cannot link a renamed or relocated record.
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&json!([longitude, latitude, name]))?)
            )
        } else {
            identity
        };
        let evidence_identity = format!(
            "{}/observation/{:x}",
            spec.input,
            Sha256::digest(serde_json::to_vec(&feature)?)
        );
        if !observation_ids.insert(evidence_identity.clone()) {
            continue;
        }
        let capacity = classify::number(&classify::field(properties, &spec.capacity_fields))
            .filter(|v| *v >= 0.0)
            .map(|v| v * spec.capacity_scale);
        let observed_on = if spec.format == "eprtr-json" {
            dates::report_date(properties, &observed)
        } else {
            observed.clone()
        };
        // WRI's GPPD commissioning_year is a capacity-weighted mean of unit ages, not a plant opening.
        let operating_from = (spec.format != "gppd-csv"
            && (operating_state != 0 || spec.status.is_none()))
        .then(|| {
            dates::dated_history(
                &classify::field(properties, &spec.start_fields),
                &observed_on,
            )
        })
        .flatten();
        let operating_until = (operating_state == 2)
            .then(|| {
                dates::dated_history(&classify::field(properties, &spec.end_fields), &observed_on)
            })
            .flatten();
        crate::facilities::temporal::operation_at(
            operating_from.as_deref(),
            operating_until.as_deref(),
            &observed_on,
            operating_state,
            &observed_on,
        )
        .with_context(|| evidence_identity.clone())?;
        let geometry = match polygon {
            Some(polygon) if spec.containment => Some(polygon),
            _ => Some(
                Geometry::new_from_geojson(
                    &json!({"type":"Point","coordinates":[longitude,latitude]}).to_string(),
                )?
                .to_wkb()?,
            ),
        };
        rows.push(Facility {
            identity: format!("{}/{identity}", spec.input),
            evidence_identity,
            role: if spec.containment {
                REGISTRY_AREA
            } else {
                REGISTRY_POINT
            },
            longitude,
            latitude,
            geometry,
            geometry_kind: u8::from(spec.containment),
            name: (!name.is_empty()).then_some(name),
            activity,
            capacity,
            capacity_unit: spec.capacity_unit.clone(),
            operating_from,
            operating_until,
            observed_on,
            operating_state,
            classification: 1,
            classification_admitted: u8::from(active),
            source: spec.input.clone(),
            source_id: spec.source_id,
            authority_rank: spec.authority_rank,
            authority_edition: spec.authority_edition,
            country,
            ..Facility::default()
        });
    }
    Ok(rows)
}

pub fn convert(inputs: &YearInputs, out: &Path) -> Result<()> {
    ensure!(!out.exists(), "facility observation output already exists");
    let model = Model::read(
        inputs
            .required(Kind::FacilityModel)
            .map_err(anyhow::Error::msg)?,
    )?;
    let regions = Regions::read(
        inputs
            .required(Kind::PrepareRegions)
            .map_err(anyhow::Error::msg)?,
    )?;
    manifest::declared_sources(inputs, &model.inputs)?;
    let mut rows = Vec::new();
    let mut duplicate_groups = HashMap::<String, HashSet<String>>::new();
    for spec in &model.inputs {
        let input = inputs
            .manifest
            .inputs
            .iter()
            .find(|input| input.id == spec.input)
            .unwrap();
        let raw = features(
            &inputs.by_id(&spec.input).map_err(anyhow::Error::msg)?,
            &spec.format,
        )
        .with_context(|| spec.input.clone())?;
        ensure!(
            !raw.is_empty() || spec.known_empty,
            "{}: undocumented empty facility input",
            spec.input
        );
        let raw_count = raw.len();
        let group = if spec.deduplicate_group.is_empty() {
            &spec.input
        } else {
            &spec.deduplicate_group
        };
        let normalized = normalize(
            spec,
            input,
            raw,
            &regions,
            duplicate_groups.entry(group.clone()).or_default(),
        )
        .with_context(|| spec.input.clone())?;
        eprintln!(
            "facility observations: {}: {raw_count} raw, {} normalized",
            spec.input,
            normalized.len()
        );
        rows.extend(normalized);
    }
    records::write(
        out,
        &rows,
        &inputs.manifest.year,
        &crate::facilities::fingerprint::observation_inputs(inputs)?,
    )?;
    eprintln!("facility observations: {} rows", rows.len());
    Ok(())
}

#[cfg(test)]
#[path = "facilities_tests.rs"]
mod tests;
