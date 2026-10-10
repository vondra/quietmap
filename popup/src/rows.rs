//! The visitor's list: the heard contributors as rows, one per object, so a bar's guests and its
//! building, or a church's bells and its building, are one row opening to its parts (owner
//! 2026-10-10: "Jeden zdroj vydává hluk a po rozkliknutí z čeho"). A part names its object in its
//! display (`object`, the object's group key); any other contributor is a row of its own. The final
//! update ranks the rows by their own Nden, their parts sounding alone together; a partial update
//! has a row for each contributor, by Lden.

use crate::evaluate::Path;
use crate::loudness::{Curve, loudness, own_nden};
use crate::percentiles::{Line, distributions_of, heard};
use crate::update::Contributor;
use physics::bands::PERIODS;
use rayon::prelude::*;
use std::collections::BTreeMap;
use tiles::sources::Layer;

/// One row of the visitor's list.
pub struct Row {
    /// Its parts, the loudest alone first: that one names the row.
    pub parts: Vec<Contributor>,
    /// Its own Nden, its parts alone together (the final update's).
    pub nden_sone: Option<f64>,
    /// Its share of the click's loudness, its parts' together (the final update's).
    pub share: Option<f64>,
}

impl Row {
    /// A row of one contributor, as a partial update lists it.
    pub fn of(contributor: Contributor) -> Self {
        Row {
            parts: vec![contributor],
            nden_sone: None,
            share: None,
        }
    }

    /// The part that names the row.
    pub fn principal(&self) -> &Contributor {
        &self.parts[0]
    }

    /// What its parts deliver together per period.
    pub fn energy(&self) -> [f64; PERIODS] {
        std::array::from_fn(|period| self.parts.iter().map(|part| part.energy[period]).sum())
    }

    /// Its parts' sound paths together.
    pub fn path(&self) -> Path {
        let mut path = Path::default();
        for part in &self.parts {
            path.add(&part.path, 1.0);
        }
        path
    }

    /// The nearest of its parts (m).
    pub fn distance_m(&self) -> f64 {
        self.parts
            .iter()
            .map(|part| part.distance_m)
            .fold(f64::INFINITY, f64::min)
    }
}

/// The object a contributor is a part of: the one its display names, else its own group.
fn object(contributor: &Contributor, display: Option<&serde_json::Value>) -> u64 {
    display
        .and_then(|fields| fields.get("object"))
        .and_then(serde_json::Value::as_str)
        .and_then(|key| u64::from_str_radix(key, 16).ok())
        .unwrap_or(contributor.group_key)
}

/// The rows of the `heard` contributors (`fields` reads a display), the loudest first: each part
/// scored alone by its own Nden and how it is heard; a row of several parts by their lines and
/// steady sound together on the curves of its loudest part's layer (`curves`).
pub fn ranked_rows<'a>(
    heard_contributors: Vec<&'a Contributor>,
    fields: &dyn Fn(&Contributor) -> Option<serde_json::Value>,
    curves: &(dyn Fn(Layer) -> &'a [Curve; PERIODS] + Sync),
) -> Vec<Row> {
    // The display records are read one by one; the parts scored in parallel.
    let displays: Vec<(&Contributor, Option<serde_json::Value>)> = heard_contributors
        .into_iter()
        .map(|contributor| (contributor, fields(contributor)))
        .collect();
    let scored: Vec<(Contributor, Option<Line>, u64)> = displays
        .par_iter()
        .map(|(contributor, display)| {
            let line = Line::of(contributor, display.as_ref());
            let own = match &line {
                Some(line) => own_nden(Some(line), [0.0; PERIODS], curves(contributor.layer)),
                None => own_nden(None, contributor.energy, curves(contributor.layer)),
            };
            let mut part = (*contributor).clone();
            part.nden_sone = Some(own);
            part.heard = display.as_ref().and_then(|fields| heard(&part, fields));
            (part, line, object(contributor, display.as_ref()))
        })
        .collect();
    let mut objects: BTreeMap<u64, Vec<(Contributor, Option<Line>)>> = BTreeMap::new();
    for (part, line, object) in scored {
        objects.entry(object).or_default().push((part, line));
    }
    let mut rows: Vec<Row> = objects
        .into_values()
        .map(|mut parts| {
            let alone = |part: &Contributor| part.nden_sone.unwrap_or(0.0);
            parts.sort_by(|(a, _), (b, _)| {
                alone(b)
                    .total_cmp(&alone(a))
                    .then(a.group_key.cmp(&b.group_key))
            });
            let nden_sone = if parts.len() == 1 {
                parts[0].0.nden_sone
            } else {
                let steady = std::array::from_fn(|period| {
                    parts
                        .iter()
                        .filter(|(_, line)| line.is_none())
                        .map(|(part, _)| part.energy[period])
                        .sum()
                });
                let lines: Vec<Line> = parts.iter().filter_map(|(_, line)| line.clone()).collect();
                let distributions = distributions_of(&lines, steady);
                Some(loudness(&distributions, curves(parts[0].0.layer)).nden_sone)
            };
            Row {
                parts: parts.into_iter().map(|(part, _)| part).collect(),
                nden_sone,
                share: None,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.nden_sone
            .unwrap_or(0.0)
            .total_cmp(&a.nden_sone.unwrap_or(0.0))
            .then(a.principal().group_key.cmp(&b.principal().group_key))
    });
    rows
}
