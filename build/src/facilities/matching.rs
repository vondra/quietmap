//! One containing footprint per registry site, with shared activity gates and duplicate-site election.

use super::{
    geometry::{self, Ring},
    model::Model,
    records::{Facility, INDUSTRY, REGISTRY_AREA},
    temporal::{OperatingEvidence, Operation},
};
use anyhow::{Result, ensure};
use rstar::{AABB, RTree, RTreeObject};
use std::collections::HashMap;

struct Indexed {
    bounds: AABB<[f64; 2]>,
    row: usize,
}
impl RTreeObject for Indexed {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.bounds
    }
}

fn point(row: &Facility) -> [f64; 2] {
    [row.longitude, row.latitude]
}
fn area(row: &Facility) -> f64 {
    f64::from(row.area_m2.unwrap_or(0.0))
}
fn bounds(ring: &Ring) -> AABB<[f64; 2]> {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for p in ring {
        for axis in 0..2 {
            low[axis] = low[axis].min(p[axis]);
            high[axis] = high[axis].max(p[axis]);
        }
    }
    AABB::from_corners(low, high)
}
fn stamp_classification(site: &mut Facility, evidence: &Facility) {
    site.evidence_identity = evidence.evidence_identity.clone();
    site.activity = evidence.activity;
    site.source = evidence.source.clone();
    site.source_id = evidence.source_id;
    site.authority_rank = evidence.authority_rank;
    site.authority_edition = evidence.authority_edition;
    site.classification = evidence.classification;
    site.suppressed = 0;
}

pub fn classify(
    sites: &mut [Facility],
    observations: &[Facility],
    model: &Model,
    cutoff: &str,
) -> Result<()> {
    let mut rings = Vec::with_capacity(sites.len());
    let mut indexed = Vec::new();
    for (row, site) in sites.iter().enumerate() {
        let ring = if site.role == INDUSTRY {
            site.geometry
                .as_deref()
                .map(geometry::rings)
                .transpose()?
                .and_then(|p| p.into_iter().next())
                .and_then(|p| p.into_iter().next())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let ring: Ring = ring
            .into_iter()
            .map(|p| {
                [
                    site.longitude + geometry::longitude_delta(p[0] - site.longitude),
                    p[1],
                ]
            })
            .collect();
        if ring.len() >= 3 {
            indexed.push(Indexed {
                bounds: bounds(&ring),
                row,
            });
        }
        rings.push(ring);
    }
    let tree = RTree::bulk_load(indexed);
    let centres = RTree::bulk_load(
        sites
            .iter()
            .enumerate()
            .filter(|(_, site)| site.role == INDUSTRY && site.source_type < 10)
            .map(|(row, site)| Indexed {
                bounds: AABB::from_point(point(site)),
                row,
            })
            .collect(),
    );
    let recipes: HashMap<_, _> = model.inputs.iter().map(|s| (s.input.as_str(), s)).collect();
    let mut winners: HashMap<usize, &Facility> = HashMap::new();
    let mut operating_winners: HashMap<usize, OperatingEvidence> = HashMap::new();
    let mut area_winners: HashMap<usize, &Facility> = HashMap::new();
    for observation in observations {
        let spec = recipes
            .get(observation.source.as_str())
            .ok_or_else(|| anyhow::anyhow!("unrecognized facility observation source"))?;
        if observation.activity == 0 {
            continue;
        }
        let operating = OperatingEvidence::new(observation, cutoff)?;
        let classifiable = observation.classification_admitted == 1
            && operating.operation != Operation::Inactive
            && (observation.observed_on.as_str() < cutoff
                || operating.operation == Operation::Operating);
        if !classifiable && operating.operation != Operation::Inactive {
            continue;
        }
        if observation.role == REGISTRY_AREA {
            // A concession's status describes the licensed area, not every tenant inside it.
            if !classifiable {
                continue;
            }
            let parts = geometry::rings(
                observation
                    .geometry
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("missing facility area"))?,
            )?;
            // A concession's first admitted area owns the point; holes remain outside its authority.
            let mut candidates = std::collections::BTreeSet::new();
            for part in &parts {
                if let Some(outer) = part.first().filter(|ring| !ring.is_empty()) {
                    let ring = outer
                        .iter()
                        .map(|p| {
                            [
                                outer[0][0] + geometry::longitude_delta(p[0] - outer[0][0]),
                                p[1],
                            ]
                        })
                        .collect();
                    let envelope = bounds(&ring);
                    for shift in [-360.0, 0.0, 360.0] {
                        let query = AABB::from_corners(
                            [envelope.lower()[0] + shift, envelope.lower()[1]],
                            [envelope.upper()[0] + shift, envelope.upper()[1]],
                        );
                        candidates.extend(
                            centres
                                .locate_in_envelope_intersecting(&query)
                                .map(|entry| entry.row),
                        );
                    }
                }
            }
            for i in candidates {
                let site = &sites[i];
                if site.role != INDUSTRY
                    || site.source_type >= 10
                    || !spec.scope.as_ref().is_none_or(|s| s.mapped(site))
                    || !geometry::contains_parts(&parts, point(site))
                {
                    continue;
                }
                area_winners.entry(i).or_insert(observation);
            }
            continue;
        }
        let mut best: Option<usize> = None;
        for shift in [-360.0, 0.0, 360.0] {
            let p = [observation.longitude + shift, observation.latitude];
            for entry in tree.locate_in_envelope_intersecting(&AABB::from_point(p)) {
                let i = entry.row;
                let site = &sites[i];
                if !spec.scope.as_ref().is_none_or(|s| s.mapped(site))
                    || !geometry::contains(&rings[i], p)
                {
                    continue;
                }
                if best.is_none_or(|previous| {
                    let old = &sites[previous];
                    area(site) < area(old)
                        || area(site) == area(old)
                            && ((site.source_type >= 10 && old.source_type < 10)
                                || (site.source_type >= 10) == (old.source_type >= 10)
                                    && (geometry::distance(point(observation), point(site)), i)
                                        < (
                                            geometry::distance(point(observation), point(old)),
                                            previous,
                                        ))
                }) {
                    best = Some(i);
                }
            }
        }
        if let Some(best) = best
            && model.accepts(&sites[best], observation.activity)
        {
            operating_winners
                .entry(best)
                .and_modify(|old| old.merge(operating))
                .or_insert(operating);
            if classifiable
                && winners
                    .get(&best)
                    .is_none_or(|old| model.beats(observation, old))
            {
                winners.insert(best, observation);
            }
        }
    }
    for (i, evidence) in operating_winners {
        evidence.stamp_operation(&mut sites[i]);
    }

    // National area registers follow point registers, while stronger global evidence keeps priority.
    for (i, evidence) in area_winners {
        if model.accepts(&sites[i], evidence.activity)
            && winners
                .get(&i)
                .is_none_or(|old| old.source_id == evidence.source_id || model.beats(evidence, old))
        {
            winners.insert(i, evidence);
        }
    }
    for (i, evidence) in winners {
        stamp_classification(&mut sites[i], evidence);
    }
    suppress_duplicates(sites, &rings, model);
    model.infer_names(sites);
    Ok(())
}

fn suppress_duplicates(sites: &mut [Facility], rings: &[Ring], model: &Model) {
    let indices: Vec<_> = sites
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.role == INDUSTRY
                && s.classification == 1
                && s.source_type < 10
                && area(s) >= model.overlap_minimum_m2
        })
        .map(|(i, _)| i)
        .collect();
    let tree = RTree::bulk_load(
        indices
            .iter()
            .filter(|i| rings[**i].len() >= 3)
            .map(|&i| Indexed {
                bounds: bounds(&rings[i]),
                row: i,
            })
            .collect(),
    );
    let mut partners = HashMap::new();
    for &i in &indices {
        let a = &sites[i];
        let mut best: Option<(f64, usize)> = None;
        for shift in [-360.0, 0.0, 360.0] {
            let p = [a.longitude + shift, a.latitude];
            for entry in tree.locate_in_envelope_intersecting(&AABB::from_point(p)) {
                let j = entry.row;
                let b = &sites[j];
                if i == j
                    || a.country != b.country
                    || area(a).max(area(b)) / area(a).min(area(b)) > model.overlap_area_ratio
                    || !geometry::contains(&rings[i], point(b))
                    || !geometry::contains(&rings[j], point(a))
                {
                    continue;
                }
                let candidate = (geometry::distance(point(a), point(b)), j);
                if best.is_none_or(|old| candidate < old) {
                    best = Some(candidate);
                }
            }
        }
        if let Some((_, j)) = best {
            partners.insert(i, j);
        }
    }
    for (&i, &j) in &partners {
        if i < j && partners.get(&j) == Some(&i) {
            let loser = if model.beats(&sites[i], &sites[j]) {
                j
            } else {
                i
            };
            sites[loser].suppressed = 1;
        }
    }
}

pub fn validate_selection(squares: &[crate::dev4::Square]) -> Result<()> {
    ensure!(
        !squares.is_empty() && squares.iter().all(|s| s.x < 512 && s.y < 512),
        "invalid facility square selection"
    );
    Ok(())
}
