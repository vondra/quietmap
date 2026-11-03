//! Retained measured-footprint and wall inventories normalize into the shared structure observations.

use super::{Objects, Observation, SUPPORT_HALO_M, Square, batches, number, selected, text};
use crate::structures::{
    observations::{HEIGHT, WALL},
    prepare_geometry::centroid,
};
use anyhow::{Result, ensure};
use arrow_array::{Array, BinaryArray, UInt8Array};
use geos::{Geom, Geometry};
use std::path::PathBuf;

struct SurveyObject {
    path: PathBuf,
    degree: [i16; 2],
    sources: Vec<String>,
}

pub fn convert(
    sources: &[(Objects, PathBuf)],
    squares: &[Square],
    rows: &mut [Vec<Observation>],
    walls: bool,
) -> Result<()> {
    let touched: std::collections::BTreeSet<_> = squares
        .iter()
        .flat_map(|&s| super::touched_degrees(s, if walls { SUPPORT_HALO_M } else { 0.0 }))
        .collect();
    let mut objects: Vec<SurveyObject> = Vec::new();
    for (source, root) in sources {
        let publisher = source
            .source
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("shared survey inventory requires a publisher source")
            })?;
        for object in &source.objects {
            let degree = object
                .degree
                .ok_or_else(|| anyhow::anyhow!("survey cache object needs its degree"))?;
            if !touched.contains(&degree) {
                continue;
            }
            let path = std::fs::canonicalize(root.join(&object.path))?;
            if let Some(existing) = objects.iter_mut().find(|o| o.path == path) {
                ensure!(
                    existing.degree == degree,
                    "one survey object has conflicting degree declarations"
                );
                existing.sources.push(publisher.to_owned());
            } else {
                objects.push(SurveyObject {
                    path,
                    degree,
                    sources: vec![publisher.to_owned()],
                });
            }
        }
    }
    // A shared cache is read once, preserving its original interleaving of admitted publishers.
    objects.sort_by_key(|o| o.degree);
    for object in objects {
        let path = &object.path;
        batches(
            path,
            &["geometry", "height_m", "kind", "source"],
            None,
            Some(if walls {
                ("official_barriers_contract", "official_barriers_v2")
            } else {
                ("measured_heights_contract", "measured_heights_v1")
            }),
            |batch| {
                let geometry = batch
                    .column_by_name("geometry")
                    .and_then(|a| a.as_any().downcast_ref::<BinaryArray>())
                    .ok_or_else(|| anyhow::anyhow!("missing survey geometry"))?;
                let kind = batch
                    .column_by_name("kind")
                    .and_then(|a| a.as_any().downcast_ref::<UInt8Array>());
                ensure!(!walls || kind.is_some(), "wall inventory lacks kind");
                for i in 0..batch.num_rows() {
                    if !object
                        .sources
                        .iter()
                        .any(|s| text(&batch, "source", i) == Some(s.as_str()))
                    {
                        continue;
                    }
                    if walls && !matches!(kind.unwrap().value(i), 0 | 2) {
                        continue;
                    }
                    ensure!(!geometry.is_null(i), "null survey geometry");
                    let bytes = geometry.value(i);
                    let geometry = Geometry::new_from_wkb(bytes)?;
                    if geometry.is_empty()? {
                        continue;
                    }
                    let (longitude, latitude) = centroid(&geometry)?;
                    let height = number(&batch, "height_m", i)
                        .ok_or_else(|| anyhow::anyhow!("missing survey height"))?;
                    ensure!(height.is_finite(), "invalid measured height");
                    for (square, rows) in squares.iter().zip(rows.iter_mut()) {
                        let (keep, owned) = selected(
                            longitude,
                            latitude,
                            *square,
                            if walls { SUPPORT_HALO_M } else { 0.0 },
                        );
                        if !keep {
                            continue;
                        }
                        rows.push(Observation {
                            kind: if walls { WALL } else { HEIGHT },
                            geometry: Some(bytes.to_vec()),
                            longitude,
                            latitude,
                            height_m: Some(height),
                            floors: 0,
                            building_type: None,
                            envelope: 0,
                            open_roof: false,
                            owned,
                        });
                    }
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}
