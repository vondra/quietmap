//! The visitor's rows join an object's parts for display, keeping each part's sound computation.

use crate::evaluate::Path;
use crate::loudness::{Curve, loudness, own_nden};
use crate::percentiles::{Line, Resolution, distributions_of, heard};
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
            path.add(&part.path);
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

/// The object a row stands for.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Object {
    /// The object a part's display names, or a contributor's own group.
    Group(u64),
    /// A road or railway by its layer and name, else its number: OpenStreetMap tags its carriageways
    /// and stretches apart, and the builder groups by name, number and class together (Ruzyně's "K
    /// Letišti" twice, one spelled with a no-break space; "Pražský okruh" with and without its 7).
    Named(Layer, String),
}

/// The object a contributor is a part of: the one its display names; a road's or railway's of its
/// name, else its number; else its own group.
fn object(contributor: &Contributor, display: &serde_json::Value) -> Object {
    if let Some(key) = display
        .get("object")
        .and_then(serde_json::Value::as_str)
        .and_then(|key| u64::from_str_radix(key, 16).ok())
    {
        return Object::Group(key);
    }
    if matches!(contributor.layer, Layer::Road | Layer::Railway) {
        let field = |name: &str| {
            display
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|text| !text.is_empty())
        };
        if let Some(name) = field("name").or_else(|| field("ref")) {
            return Object::Named(contributor.layer, name);
        }
    }
    Object::Group(contributor.group_key)
}

/// The rows of the `heard` contributors (`fields` reads a display), the loudest first: each part
/// scored alone by its own Nden and how it is heard; a row of several parts by their lines and
/// steady sound together on the curves of its loudest part's layer (`curves`).
pub fn ranked_rows<'a>(
    heard_contributors: Vec<&'a Contributor>,
    fields: &dyn Fn(&crate::percentiles::Group) -> Result<serde_json::Value, String>,
    curves: &(dyn Fn(Layer) -> &'a [Curve; PERIODS] + Sync),
) -> Result<Vec<Row>, String> {
    // The display records are read one by one; the parts scored in parallel.
    let displays: Vec<(&Contributor, serde_json::Value)> = heard_contributors
        .into_iter()
        .map(|contributor| Ok((contributor, fields(&contributor.group())?)))
        .collect::<Result<_, String>>()?;
    let scored: Vec<(Contributor, Option<Line>, Object)> = displays
        .par_iter()
        .map(|(contributor, display)| {
            let line = Line::of(
                &contributor.group(),
                &crate::percentiles::Passes::read(contributor.layer, display),
            );
            let own = match &line {
                Some(line) => own_nden(Some(line), [0.0; PERIODS], curves(contributor.layer)),
                None => own_nden(None, contributor.energy, curves(contributor.layer)),
            };
            let mut part = (*contributor).clone();
            part.nden_sone = Some(own);
            part.heard = heard(&part, display);
            (part, line, object(contributor, display))
        })
        .collect();
    let mut objects: BTreeMap<Object, Vec<(Contributor, Option<Line>)>> = BTreeMap::new();
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
                let distributions = distributions_of(&lines, steady, Resolution::CLICK);
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
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::percentiles::tests::contributor;
    use serde_json::json;

    #[test]
    fn a_road_is_one_row_by_its_name_else_its_number_whatever_its_tags() {
        let road = contributor(1, Layer::Road, 60.0, 100.0);
        let other = contributor(2, Layer::Road, 60.0, 100.0);
        // Ruzyně: "K Letišti" with a no-break space on one carriageway, a plain one on the other.
        assert_eq!(
            object(&road, &json!({"name": "K\u{a0}Letišti", "ref": ""})),
            object(&other, &json!({"name": "K  Letišti", "ref": ""})),
        );
        // A road named on one carriageway and numbered on both is its name; one without a name is its number.
        assert_eq!(
            object(&road, &json!({"name": "Pražský okruh", "ref": "7"})),
            Object::Named(Layer::Road, "Pražský okruh".into())
        );
        assert_eq!(
            object(&road, &json!({"name": "", "ref": "7"})),
            Object::Named(Layer::Road, "7".into())
        );
        // Neither: its own group; a venue's part: the object its display names.
        assert_eq!(
            object(&road, &json!({"name": "", "ref": ""})),
            Object::Group(1)
        );
        let guests = contributor(3, Layer::Building, 60.0, 100.0);
        assert_eq!(
            object(&guests, &json!({"name": "Bar", "object": "ff"})),
            Object::Group(255)
        );
    }

    #[test]
    fn tram_tracks_join_only_when_the_data_names_or_numbers_the_line() {
        let track = contributor(1, Layer::Railway, 60.0, 100.0);
        let other_track = contributor(2, Layer::Railway, 60.0, 100.0);
        for identity in [json!({"name": "Tram line"}), json!({"ref": "7"})] {
            assert_eq!(object(&track, &identity), object(&other_track, &identity));
        }
        // An empty name and ref identify neither a route nor a street; adjacent tracks may diverge.
        let unnamed = json!({"name": "", "ref": "", "rail_type": "tram"});
        assert_ne!(object(&track, &unnamed), object(&other_track, &unnamed));
    }
}
