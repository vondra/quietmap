//! The visitor's rows join an object's parts for display, keeping each part's sound computation.

use crate::evaluate::Path;
use crate::loudness::{Curve, loudness, own_nden};
use crate::percentiles::{Line, Resolution, distributions_of, heard};
use crate::update::Contributor;
use physics::bands::PERIODS;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use tiles::sources::Layer;

/// One row of the visitor's list.
pub struct Row {
    /// Its parts, the largest share first ([`rank_by_share`]): that one names the row.
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

/// The rows of the `heard` contributors, each object's parts joined (`fields` reads a display,
/// one by one), unscored; and the displays by group key, for the rows the list sends
/// ([`score_rows`]).
pub fn heard_rows(
    heard_contributors: Vec<&Contributor>,
    fields: &dyn Fn(&crate::percentiles::Group) -> Result<serde_json::Value, String>,
) -> Result<(Vec<Row>, HashMap<u64, serde_json::Value>), String> {
    let mut objects: BTreeMap<Object, Vec<Contributor>> = BTreeMap::new();
    let mut displays = HashMap::new();
    for contributor in heard_contributors {
        let display = fields(&contributor.group())?;
        objects
            .entry(object(contributor, &display))
            .or_default()
            .push(contributor.clone());
        displays.insert(contributor.group_key, display);
    }
    let rows = objects
        .into_values()
        .map(|parts| Row {
            parts,
            nden_sone: None,
            share: None,
        })
        .collect();
    Ok((rows, displays))
}

/// Scores the rows the list sends, in parallel: each part alone by its own Nden and how it is
/// heard; a row of several parts by their lines and steady sound together on the curves of the
/// layer of its part loudest alone (`curves`).
pub fn score_rows<'a>(
    rows: &mut [Row],
    displays: &HashMap<u64, serde_json::Value>,
    curves: &(dyn Fn(Layer) -> &'a [Curve; PERIODS] + Sync),
) {
    rows.par_iter_mut().for_each(|row| {
        let mut lines = Vec::new();
        let mut steady = [0.0; PERIODS];
        for part in &mut row.parts {
            let display = &displays[&part.group_key];
            let line = Line::of(
                &part.group(),
                &crate::percentiles::Passes::read(part.layer, display),
            );
            part.nden_sone = Some(match &line {
                Some(line) => own_nden(Some(line), [0.0; PERIODS], curves(part.layer)),
                None => own_nden(None, part.energy, curves(part.layer)),
            });
            part.heard = heard(part, display);
            match line {
                Some(line) => lines.push(line),
                None => (0..PERIODS).for_each(|period| steady[period] += part.energy[period]),
            }
        }
        row.nden_sone = if row.parts.len() == 1 {
            row.parts[0].nden_sone
        } else {
            let alone = |part: &&Contributor| part.nden_sone.unwrap_or(0.0);
            let loudest = row
                .parts
                .iter()
                .max_by(|a, b| alone(a).total_cmp(&alone(b)));
            let distributions = distributions_of(&lines, steady, Resolution::CLICK);
            loudest.map(|part| loudness(&distributions, curves(part.layer)).nden_sone)
        };
    });
}

/// Gives the `rows` their parts' `shares` of the place's loudness, in the order of their parts, and
/// ranks rows and parts by them: the share is the number a row shows and the list is cut by. A
/// row's own Nden, alone, is no rank: a steady sound under louder ones keeps little of it in the
/// mix (a Prague villa street, 2026-10-17: a road and the flights 1.2 sone alone each, 6 and 12 % of
/// the place), and ranked by it the list folded a lane of 5 % under rows of 2 %.
pub fn rank_by_share(rows: &mut [Row], shares: &[f64]) {
    let share = |part: &Contributor| part.share.unwrap_or(0.0);
    let mut shares = shares.iter();
    for row in rows.iter_mut() {
        for (part, &part_share) in row.parts.iter_mut().zip(&mut shares) {
            part.share = Some(part_share);
        }
        row.parts.sort_by(|a, b| {
            share(b)
                .total_cmp(&share(a))
                .then(a.group_key.cmp(&b.group_key))
        });
        row.share = Some(row.parts.iter().map(share).sum());
    }
    rows.sort_by(|a, b| {
        (b.share.unwrap_or(0.0))
            .total_cmp(&a.share.unwrap_or(0.0))
            .then(a.principal().group_key.cmp(&b.principal().group_key))
    });
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

    /// A Prague villa street's rows: the road and the flights alike alone, the flights twice the share;
    /// the lane, quieter alone, as large a share as the road. The list ranks by share, and so
    /// does a row's parts.
    #[test]
    fn rows_rank_by_their_share_of_the_place_not_by_their_loudness_alone() {
        let row = |parts: Vec<(u64, f64)>| Row {
            parts: parts
                .into_iter()
                .map(|(key, alone)| {
                    let mut part = contributor(key, Layer::Road, 40.0, 100.0);
                    part.nden_sone = Some(alone);
                    part
                })
                .collect(),
            nden_sone: None,
            share: None,
        };
        let mut rows = vec![
            row(vec![(1, 1.22)]),
            row(vec![(3, 0.90), (2, 0.50)]),
            row(vec![(4, 1.16)]),
        ];
        rank_by_share(&mut rows, &[0.055, 0.002, 0.050, 0.117]);
        let order: Vec<u64> = rows.iter().map(|row| row.principal().group_key).collect();
        assert_eq!(order, [4, 1, 2]);
        assert_eq!(
            rows[2]
                .parts
                .iter()
                .map(|part| part.group_key)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert!((rows[2].share.unwrap() - 0.052).abs() < 1e-12);
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
