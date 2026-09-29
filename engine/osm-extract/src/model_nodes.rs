//! Retained transport control points and power nodes, linked by original OSM node identity.

use crate::{
    classify::{self, FeatureType, Tags},
    spill::Spiller,
    transport::ResolvedNode,
};
use anyhow::Result;
use std::collections::BTreeMap;

pub struct ModelNode {
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
    pub control: Option<Tags>,
    pub power: Option<Tags>,
}

pub fn prepare<'a>(
    id: i64,
    lat: f64,
    lon: f64,
    tags: impl Iterator<Item = (&'a str, &'a str)> + Clone,
) -> Option<ModelNode> {
    let control = classify::transport_point_tags(tags.clone()).filter(|_| {
        classify::scope_keeps(&FeatureType::Road) || classify::scope_keeps(&FeatureType::Railway)
    });
    let tag = |key: &str| tags.clone().find(|(k, _)| *k == key).map(|(_, v)| v);
    let power = (tags
        .clone()
        .any(|(key, _)| key == "power" || key.ends_with(":power") || key.ends_with(":landuse"))
        && classify::scope_keeps(&FeatureType::Industrial)
        && classify::is_power_or_inactive_industry(tag)
        && !classify::is_turbine(tag))
    .then(|| classify::extract_tags(tags, &FeatureType::Industrial))
    .filter(|kept| {
        // A plant/generator node without a staged power class has no
        // footprint for the generic area law: it would emit as an invented
        // 10,000 m² factory stacked on the plant polygon. The polygon owns
        // power emission; staged nodes (wind/solar/substation/inactive) stay.
        !matches!(
            kept.get("power").map(String::as_str),
            Some("plant" | "generator")
        ) || classify::industrial_class(kept).is_some()
    });
    (control.is_some() || power.is_some()).then_some(ModelNode {
        id,
        lat,
        lon,
        control,
        power,
    })
}

/// Retained control points keyed by node id. A `BTreeMap`, not a `HashMap`:
/// unlinked orphans flush in `finish` in iteration order, so per-process
/// hashing would make spill bytes (and any unsorted consumer) differ between
/// identical extracts. Node-id order is deterministic by construction.
#[derive(Default)]
pub struct ControlPoints(BTreeMap<i64, (ModelNode, bool)>);

impl ControlPoints {
    pub fn insert(&mut self, node: ModelNode) {
        if node.control.is_some() {
            self.0.insert(node.id, (node, false));
        }
    }

    pub fn link_way(
        &mut self,
        id: i64,
        family: &str,
        nodes: &[ResolvedNode],
        spill: &mut Spiller,
    ) -> Result<()> {
        let metres = crate::transport::way_metres(nodes);
        for (vertex, (node_id, _)) in nodes.iter().enumerate() {
            if let Some((point, linked)) = self.0.get_mut(node_id) {
                spill.emit_control_point(
                    point,
                    Some((id, family, vertex, metres.as_ref().map(|m| m[vertex]))),
                )?;
                *linked = true;
            }
        }
        Ok(())
    }

    pub fn finish(self, spill: &mut Spiller) -> Result<()> {
        for (node, linked) in self.0.into_values() {
            if !linked {
                spill.emit_control_point(&node, None)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_nodes_survive_and_wind_plants_are_not_turbines() {
        let point = prepare(
            1,
            50.,
            14.,
            [("disused:power", "substation")].into_iter(),
        )
        .unwrap();
        assert!(point.power.is_some());
        assert!(prepare(
            1,
            50.,
            14.,
            [("power", "generator"), ("generator:source", "wind")].into_iter()
        )
        .is_none());
    }
    #[test]
    fn unstaged_plant_and_generator_nodes_are_not_industrial_evidence() {
        // A plant/generator node without a staged power class has no
        // footprint, so emitting it would invent a generic 10,000 m² factory
        // on top of the plant polygon (or on nothing). The polygon owns
        // power emission; the node carries no area law input.
        for pairs in [
            vec![("power", "plant"), ("plant:source", "gas")],
            vec![("power", "generator"), ("generator:source", "gas")],
            vec![("power", "generator"), ("generator:source", "coal")],
            // A copied generator tag stages nothing: still no footprint.
            vec![("power", "plant"), ("generator:source", "wind")],
        ] {
            assert!(
                prepare(1, 50., 14., pairs.iter().copied()).is_none(),
                "unstaged power node must not emit: {pairs:?}"
            );
        }
        // Staged power nodes stay: sole-wind outlines and inactive sites are
        // retained silent, solar units emit per-MW, substations per-MVA.
        for pairs in [
            vec![("power", "plant"), ("plant:source", "wind")],
            vec![("power", "generator"), ("generator:source", "solar")],
            vec![("power", "substation")],
            vec![("disused:power", "substation")],
        ] {
            let point = prepare(1, 50., 14., pairs.iter().copied()).unwrap();
            assert!(point.power.is_some(), "staged power node lost: {pairs:?}");
        }
    }
}
