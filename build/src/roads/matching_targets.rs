//! Index route identities and enforce the same declared area and reference support on every candidate.

use super::{
    input::Road,
    observation::Observation,
    observation_target::{IdentityField, ReferenceRule},
};
use anyhow::Result;
use geo::{Contains, LineString, MultiPolygon, Point, Polygon};
use regex::{Regex, RegexSet};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};
use unicode_normalization::UnicodeNormalization;

pub(crate) fn name(value: &str) -> String {
    value
        .nfkd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

struct Rule {
    row: usize,
    mask: u16,
    priority: u16,
    first_match: Option<Regex>,
}
pub(super) type IdentityScore = (u16, Reverse<usize>);
struct Patterns {
    rules: Vec<Rule>,
    expressions: RegexSet,
}
impl Patterns {
    fn new(rows: Vec<(Rule, String)>) -> Result<Self> {
        let expressions = RegexSet::new(rows.iter().map(|r| &r.1))?;
        Ok(Self {
            rules: rows.into_iter().map(|r| r.0).collect(),
            expressions,
        })
    }
    fn matches(&self, value: &str, class: u8, scores: &mut HashMap<usize, IdentityScore>) {
        for index in self.expressions.matches(value) {
            let rule = &self.rules[index];
            if rule.mask & (1 << class) != 0 {
                let offset = rule
                    .first_match
                    .as_ref()
                    .and_then(|pattern| pattern.find(value))
                    .map_or(0, |m| m.start());
                let candidate = (rule.priority, Reverse(offset));
                scores
                    .entry(rule.row)
                    .and_modify(|score| *score = (*score).max(candidate))
                    .or_insert(candidate);
            }
        }
    }
}

pub struct Query {
    raw_references: Vec<String>,
    class: u8,
    scores: HashMap<usize, IdentityScore>,
    whole_reference: String,
    references: Vec<String>,
    street_name: String,
}
pub struct Targets {
    country: HashMap<u16, Vec<usize>>,
    reference: HashMap<(u16, String), Vec<usize>>,
    names: HashMap<u16, Patterns>,
    references: HashMap<u16, Patterns>,
    known_references: HashSet<(u16, String)>,
    observed_references: Vec<HashSet<String>>,
    coverage: Vec<MultiPolygon<f64>>,
    rewrites: HashMap<u16, Vec<(Regex, String)>>,
}

impl Targets {
    pub fn new(rows: &[Observation]) -> Result<Self> {
        let mut result = Self {
            country: HashMap::new(),
            reference: HashMap::new(),
            names: HashMap::new(),
            references: HashMap::new(),
            known_references: HashSet::new(),
            observed_references: Vec::new(),
            coverage: Vec::new(),
            rewrites: HashMap::new(),
        };
        let mut names = HashMap::<u16, Vec<(Rule, String)>>::new();
        let mut references = HashMap::<u16, Vec<(Rule, String)>>::new();
        for (index, row) in rows.iter().enumerate() {
            row.target.validate()?;
            if !row.target.reference_rewrites.is_empty() {
                if let Some(existing) = result.rewrites.get(&row.source_id) {
                    anyhow::ensure!(
                        existing.len() == row.target.reference_rewrites.len()
                            && existing
                                .iter()
                                .zip(&row.target.reference_rewrites)
                                .all(|((pattern, replacement), r)| pattern.as_str() == r.pattern
                                    && replacement == &r.replacement),
                        "one source has conflicting reference syntax"
                    );
                } else {
                    result.rewrites.insert(
                        row.source_id,
                        row.target
                            .reference_rewrites
                            .iter()
                            .map(|r| Ok((Regex::new(&r.pattern)?, r.replacement.clone())))
                            .collect::<Result<_>>()?,
                    );
                }
            }
            let normalized = row
                .reference
                .split(';')
                .filter(|s| !s.is_empty())
                .map(super::matching::canonical_reference)
                .collect::<HashSet<_>>();
            result
                .known_references
                .extend(normalized.iter().map(|r| (row.source_id, r.clone())));
            result.observed_references.push(normalized);
            let ring = |points: &[[f64; 2]]| {
                LineString::from(points.iter().map(|p| (p[0], p[1])).collect::<Vec<_>>())
            };
            result.coverage.push(MultiPolygon::new(
                row.target
                    .coverage
                    .iter()
                    .map(|polygon| {
                        Polygon::new(
                            ring(&polygon[0]),
                            polygon[1..].iter().map(|r| ring(r)).collect(),
                        )
                    })
                    .collect(),
            ));
            for identity in &row.target.identities {
                let target = if identity.field == IdentityField::Name {
                    &mut names
                } else {
                    &mut references
                };
                target.entry(row.country).or_default().push((
                    Rule {
                        row: index,
                        mask: identity.road_class_mask,
                        priority: identity.priority,
                        first_match: identity
                            .prefer_first_match
                            .then(|| Regex::new(&identity.pattern))
                            .transpose()?,
                    },
                    identity.pattern.clone(),
                ));
            }
            if !row.geometry.is_empty() || !row.target.identities.is_empty() {
                continue;
            }
            if row.reference_required {
                for reference in &result.observed_references[index] {
                    result
                        .reference
                        .entry((row.country, reference.clone()))
                        .or_default()
                        .push(index);
                }
            } else {
                result.country.entry(row.country).or_default().push(index);
            }
        }
        result.names = names
            .into_iter()
            .map(|(country, rows)| Ok((country, Patterns::new(rows)?)))
            .collect::<Result<_>>()?;
        result.references = references
            .into_iter()
            .map(|(country, rows)| Ok((country, Patterns::new(rows)?)))
            .collect::<Result<_>>()?;
        Ok(result)
    }

    pub fn query(&self, road: &Road) -> Query {
        let whole_reference = super::matching::canonical_reference(&road.reference);
        let references = whole_reference.split(';').map(str::to_owned).collect();
        let mut scores = HashMap::new();
        for country in [road.country.0, 0] {
            if let Some(patterns) = self.names.get(&country) {
                patterns.matches(&name(&road.name), road.class, &mut scores);
            }
            if let Some(patterns) = self.references.get(&country) {
                patterns.matches(&whole_reference, road.class, &mut scores);
            }
        }
        Query {
            street_name: road
                .name
                .nfkc()
                .flat_map(char::to_lowercase)
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            raw_references: road
                .reference
                .split(';')
                .map(|r| r.trim().to_owned())
                .collect(),
            class: road.class,
            scores,
            whole_reference,
            references,
        }
    }

    pub fn candidates(&self, road: &Road, query: &Query, output: &mut Vec<usize>) {
        output.extend(query.scores.keys().copied());
        output.extend(
            self.country
                .get(&road.country.0)
                .into_iter()
                .flatten()
                .copied(),
        );
        for reference in &query.references {
            output.extend(
                self.reference
                    .get(&(road.country.0, reference.clone()))
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }
    }

    pub fn accepts(
        &self,
        index: usize,
        row: &Observation,
        query: &Query,
        midpoint: [f64; 2],
    ) -> Option<(IdentityScore, f64)> {
        if let Some(proximity) = &row.target.proximity
            && (proximity.allowed_classes & (1 << query.class) == 0
                || (proximity.compatible_classes & (1 << query.class) == 0
                    && !proximity.street_names.contains(&query.street_name)))
        {
            return None;
        }
        if !row.target.coverage.is_empty()
            && !self.coverage[index].contains(&Point::new(midpoint[0], midpoint[1]))
        {
            return None;
        }
        let priority = if row.target.identities.is_empty() {
            (0, Reverse(0))
        } else {
            *query.scores.get(&index)?
        };
        if !row.reference_required {
            return Some((priority, row.search_radius_m));
        }
        let observed = &self.observed_references[index];
        let accepted = match row.target.reference_rule {
            ReferenceRule::Whole => observed.contains(&query.whole_reference),
            ReferenceRule::AnyToken => query.references.iter().any(|r| observed.contains(r)),
            ReferenceRule::FirstValidToken => query
                .raw_references
                .iter()
                .find_map(|reference| {
                    self.rewrites[&row.source_id]
                        .iter()
                        .find_map(|(pattern, replacement)| {
                            pattern.is_match(reference).then(|| {
                                pattern
                                    .replace(reference, replacement.as_str())
                                    .into_owned()
                            })
                        })
                })
                .is_some_and(|r| observed.contains(&super::matching::canonical_reference(&r))),
        };
        if accepted {
            return Some((priority, row.search_radius_m));
        }
        let fallback = row.target.reference_fallback.as_ref()?;
        if fallback.road_class_mask & (1 << query.class) == 0
            || fallback.reference_sources.iter().any(|source| {
                self.known_references
                    .contains(&(*source, query.whole_reference.clone()))
            })
        {
            return None;
        }
        Some((priority, fallback.search_radius_m))
    }
}
