//! RWS loop sections retain reciprocal twins, measured length classes and each single carriageway's direction.

use super::Source;
use crate::{
    roads::observation::{CountBasis, Observation},
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    search_radius_m: f64,
    minimum_alignment_cosine: f64,
}

struct Section {
    id: String,
    twin: Option<String>,
    main: bool,
    ramp: bool,
    rank: u8,
    references: Vec<String>,
    geometry: Vec<[f64; 2]>,
    counts: [f64; 3],
}

fn count(value: &Value) -> Option<f64> {
    value.as_u64().filter(|v| *v < (1 << 53)).map(|v| v as f64)
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    raw: &Value,
) -> Result<Vec<Observation>> {
    let features = raw["features"]
        .as_array()
        .ok_or_else(|| anyhow!("INWEVA requires GeoJSON features"))?;
    let mut sections = Vec::new();
    let mut identities = HashSet::new();
    for feature in features {
        let p = &feature["properties"];
        let id = p["vbn_id"]
            .as_str()
            .ok_or_else(|| anyhow!("missing INWEVA section identity"))?;
        ensure!(
            !id.is_empty() && identities.insert(id),
            "duplicate INWEVA section {id}"
        );
        let kind = p["bnsubsrt_b"]
            .as_str()
            .ok_or_else(|| anyhow!("missing INWEVA carriageway type"))?;
        // These are the publisher's general-traffic codes; bus lanes and rest areas are not through traffic.
        let ramp = matches!(
            kind,
            "OPR" | "AFR" | "VBR" | "VBD" | "VBS" | "VBI" | "VBW" | "VBK"
        );
        if !ramp && !matches!(kind, "HR" | "NRB") {
            continue;
        }
        if p["afst_mtw"].as_f64() != Some(0.0) {
            continue;
        }
        let counts = [
            count(&p["al_e_wk"]),
            count(&p["l2_e_wk"]),
            count(&p["l3_e_wk"]),
        ];
        let [Some(total), Some(medium), Some(heavy)] = counts else {
            continue;
        };
        if total < medium + heavy {
            continue;
        }
        let references = ["wegnrhmp_b", "wegnrhmp_e"]
            .iter()
            .filter_map(|k| p[k].as_str())
            .map(|r| r.split_whitespace().collect::<String>().to_uppercase())
            .filter(|r| !r.is_empty())
            .collect::<Vec<_>>();
        let rank = match references.first().and_then(|r| r.chars().next()) {
            Some('A') => 0,
            Some('N') => 1,
            _ => continue,
        };
        if feature["geometry"]["type"].as_str() != Some("LineString") {
            continue;
        }
        let geometry: Vec<[f64; 2]> =
            serde_json::from_value(feature["geometry"]["coordinates"].clone())?;
        if geometry.len() < 2
            || !geometry
                .iter()
                .all(crate::roads::observation_target::coordinate)
        {
            continue;
        }
        let twin = match &p["vbn_id_tgn"] {
            Value::Null => None,
            Value::String(s) if s.is_empty() || s == id => None,
            Value::String(s) => Some(s.clone()),
            _ => return Err(anyhow!("invalid INWEVA twin identity")),
        };
        sections.push(Section {
            id: id.into(),
            twin,
            main: kind == "HR",
            ramp,
            rank,
            references,
            geometry,
            // The published all-vehicle total can differ from independently corrected class totals.
            counts: [total - medium - heavy, medium, heavy],
        });
    }
    let by_id = sections
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect::<HashMap<_, _>>();
    let mut rows = Vec::new();
    for section in &sections {
        let twin = section
            .twin
            .as_ref()
            .and_then(|id| by_id.get(id.as_str()))
            .map(|i| &sections[*i])
            .filter(|t| section.main && t.main && t.twin.as_deref() == Some(section.id.as_str()));
        // Retain each measured direction; a reciprocal sum supports only an undivided mapped road.
        for twin in std::iter::once(None).chain(twin.filter(|t| section.id < t.id).map(Some)) {
            let (mut identities, mut references, mut geometry, mut counts, mut rank) = (
                vec![section.id.clone()],
                section.references.clone(),
                vec![section.geometry.clone()],
                section.counts,
                section.rank,
            );
            if let Some(twin) = twin {
                identities.push(twin.id.clone());
                references.extend(twin.references.clone());
                geometry.push(twin.geometry.clone());
                rank = rank.max(twin.rank);
                for (count, extra) in counts.iter_mut().zip(twin.counts) {
                    *count += extra;
                }
            }
            identities.sort();
            references.sort();
            references.dedup();
            let mut row = recipe.source.observation(
                inputs,
                identities.join("-"),
                if twin.is_some() {
                    CountBasis::BothDirections
                } else {
                    CountBasis::Directional
                },
            )?;
            let total = counts.iter().sum::<f64>();
            row.vehicle_classes = [None, Some(counts[1]), Some(counts[2]), None];
            row.vehicle_groups = vec![crate::roads::observation_groups::VehicleGroup {
                classes: 9,
                count: counts[0],
            }];
            row.total = Some(total);
            row.estimated_classes = 9;
            row.reference = references.join(";");
            row.reference_required = !section.ramp;
            row.road_class_mask = if section.ramp {
                if rank == 0 { 3 << 10 } else { 7 << 10 }
            } else if rank == 0 {
                3
            } else {
                7
            };
            row.geometry = geometry;
            row.search_radius_m = recipe.search_radius_m;
            row.direction = u8::from(twin.is_none());
            row.target.required_oneway = twin.map(|_| false);
            row.target.minimum_alignment_cosine = recipe.minimum_alignment_cosine;
            rows.push(row);
        }
    }
    // The publisher's section identity resolves coincident INWEVA observations deterministically.
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    ensure!(!rows.is_empty(), "INWEVA has no usable measured sections");
    Ok(rows)
}
