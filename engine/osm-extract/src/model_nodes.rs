//! Retained transport control points and power nodes, linked by original OSM node identity.

use crate::{
    classify::{self, FeatureType, Tags},
    spill::Spiller,
    transport::ResolvedNode,
};
use anyhow::Result;
use std::collections::HashMap;

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
    .then(|| classify::extract_tags(tags, &FeatureType::Industrial));
    (control.is_some() || power.is_some()).then_some(ModelNode {
        id,
        lat,
        lon,
        control,
        power,
    })
}

#[derive(Default)]
pub struct ControlPoints(HashMap<i64, (ModelNode, bool)>);

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
        for pairs in [
            vec![("disused:power", "substation")],
            vec![("power", "plant"), ("generator:source", "wind")],
        ] {
            let point = prepare(1, 50., 14., pairs.iter().copied()).unwrap();
            assert!(point.power.is_some());
        }
        assert!(prepare(
            1,
            50.,
            14.,
            [("power", "generator"), ("generator:source", "wind")].into_iter()
        )
        .is_none());
    }
}
