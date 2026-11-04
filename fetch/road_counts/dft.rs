//! DfT point histories select the latest admitted observation, retaining estimated years and minor-road support.

use super::Source;
use crate::{
    roads::{
        observation::{CountBasis, Observation},
        observation_target::{DistanceRule, ReferenceRule, WayAssignment},
    },
    year::YearInputs,
};
use anyhow::{Result, anyhow, ensure};
use chrono::Datelike;
use serde::Deserialize;
use std::{collections::BTreeMap, io::Read, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    pub source: Source,
    excluded_manual_years: Vec<i32>,
    maximum_age_years: i32,
    major_radius_m: f64,
    minor_assignment: WayAssignment,
    slip_maximum_length_km: f64,
    slip_junction_pattern: String,
    slip_total_ratio: f64,
    slip_neighbor_radius_m: f64,
}

struct Point {
    row: Observation,
    rank: u8,
    year: i32,
    length: Option<f64>,
    named_slip: bool,
}

pub(super) fn convert(
    recipe: &Recipe,
    inputs: &YearInputs,
    path: &Path,
) -> Result<Vec<Observation>> {
    parse(recipe, inputs, std::fs::File::open(path)?)
}

fn parse(recipe: &Recipe, inputs: &YearInputs, reader: impl Read) -> Result<Vec<Observation>> {
    let input = recipe.source.input(inputs)?;
    let (start, end) = input.observed.bounds().map_err(anyhow::Error::msg)?;
    ensure!(
        recipe.maximum_age_years > 0,
        "invalid DfT historical window"
    );
    let slip = regex::Regex::new(&recipe.slip_junction_pattern)?;
    let mut latest = BTreeMap::<String, Point>::new();
    for record in csv::Reader::from_reader(reader).deserialize::<BTreeMap<String, String>>() {
        let record = record?;
        let text = |key| record.get(key).map_or("", String::as_str).trim();
        let Some(year) = text("year").parse::<i32>().ok() else {
            continue;
        };
        let from = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
            .ok_or_else(|| anyhow!("invalid DfT observation year"))?;
        let to = chrono::NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
        if from < start || to > end || year <= end.year() - recipe.maximum_age_years {
            continue;
        }
        let rank = match text("road_category") {
            "TM" | "PM" => 0,
            "TA" => 1,
            "PA" => 2,
            "MB" => 3,
            "MCU" => 5,
            _ => continue,
        };
        let estimated = text("estimation_method") != "Counted";
        if rank == 5 && (estimated || recipe.excluded_manual_years.contains(&year)) {
            continue;
        }
        let id = text("count_point_id");
        if id.is_empty() || latest.get(id).is_some_and(|r| r.year >= year) {
            continue;
        }
        let coordinate = |key| text(key).parse::<f64>().ok().filter(|v| v.is_finite());
        let (Some(longitude), Some(latitude)) = (coordinate("longitude"), coordinate("latitude"))
        else {
            continue;
        };
        if !crate::roads::observation_target::coordinate(&[longitude, latitude]) {
            continue;
        }
        let count = |key| -> Result<Option<f64>> {
            let raw = text(key);
            if raw.is_empty() {
                return Ok(None);
            }
            let n: f64 = raw.parse()?;
            ensure!(
                n.is_finite() && n >= 0.0 && n.fract() == 0.0,
                "invalid DfT vehicle count"
            );
            Ok(Some(n))
        };
        let Some(total) = count("all_motor_vehicles")?.filter(|v| *v > 0.0) else {
            continue;
        };
        let rigid = count("HGVs_2_rigid_axle")?;
        let lorries = count("all_HGVs")?;
        let buses = count("buses_and_coaches")?;
        if rigid.zip(lorries).is_some_and(|(r, h)| r > h) {
            continue;
        }
        let counts = [
            count("cars_and_taxis")?
                .zip(count("LGVs")?)
                .map(|(a, b)| a + b),
            buses.zip(rigid).map(|(b, r)| b + r),
            lorries.zip(rigid).map(|(h, r)| h - r),
            count("two_wheeled_motor_vehicles")?,
        ];
        if counts.iter().flatten().sum::<f64>() > total + 2.0 {
            continue;
        }
        let mut row = recipe
            .source
            .observation(inputs, id.into(), CountBasis::BothDirections)?;
        row.observed_start = from.to_string();
        row.observed_end = to.to_string();
        row.reference = text("road_name").into();
        row.geometry = vec![vec![[longitude, latitude]]];
        row.total = Some(total);
        row.vehicle_classes = counts;
        if rigid.is_none() {
            row.vehicle_groups = buses
                .zip(lorries)
                .into_iter()
                .map(|(b, h)| crate::roads::observation_groups::VehicleGroup {
                    classes: 6,
                    count: b + h,
                })
                .collect();
        }
        row.estimated_classes = if estimated { 15 } else { 0 };
        row.measured = !estimated;
        row.target.reference_rule = ReferenceRule::Whole;
        row.reference_required = rank != 5;
        row.search_radius_m = if rank == 5 {
            recipe.minor_assignment.clearance_m
        } else {
            recipe.major_radius_m
        };
        row.target.distance_rule = if rank == 5 {
            DistanceRule::PointToPiece
        } else {
            DistanceRule::GreatCircle
        };
        row.target.way_assignment = (rank == 5).then(|| recipe.minor_assignment.clone());
        latest.insert(
            id.into(),
            Point {
                row,
                rank,
                year,
                length: coordinate("link_length_km"),
                named_slip: slip.is_match(text("start_junction_road_name"))
                    != slip.is_match(text("end_junction_road_name")),
            },
        );
    }
    let mut points = latest.into_values().collect::<Vec<_>>();
    let mut by_reference = BTreeMap::<String, Vec<usize>>::new();
    for (i, p) in points.iter().enumerate().filter(|(_, p)| p.rank <= 3) {
        by_reference
            .entry(p.row.reference.clone())
            .or_default()
            .push(i);
    }
    let mut ramps = vec![false; points.len()];
    for indices in by_reference.values() {
        for &i in indices {
            let point = &points[i];
            if !point
                .length
                .is_some_and(|v| v <= recipe.slip_maximum_length_km)
            {
                continue;
            }
            let mut neighbors = indices
                .iter()
                .copied()
                .filter(|j| {
                    *j != i
                        && point.rank <= 1
                        && points[*j]
                            .length
                            .is_some_and(|v| v > recipe.slip_maximum_length_km)
                        && crate::roads::flat_distance(
                            point.row.geometry[0][0],
                            points[*j].row.geometry[0][0],
                        ) < recipe.slip_neighbor_radius_m
                })
                .map(|j| points[j].row.total.unwrap())
                .collect::<Vec<_>>();
            neighbors.sort_by(f64::total_cmp);
            let median = if neighbors.is_empty() {
                0.0
            } else {
                (neighbors[(neighbors.len() - 1) / 2] + neighbors[neighbors.len() / 2]) / 2.0
            };
            ramps[i] =
                point.named_slip || point.row.total.unwrap() < recipe.slip_total_ratio * median;
        }
    }
    for (point, ramp) in points.iter_mut().zip(&ramps) {
        if *ramp {
            point.row.count_basis = CountBasis::Directional;
            let mut assignment = recipe.minor_assignment.clone();
            // A known slip is expected beside a mainline; its link-class mask provides the distinction.
            assignment.require_class_clearance = false;
            point.row.target.way_assignment = Some(assignment);
        }
        point.row.road_class_mask = (0u8..13)
            .filter(|c| {
                let mapped = if *c <= 4 {
                    *c
                } else if *c >= 10 {
                    *c - 10
                } else {
                    6
                };
                (*c >= 10) == *ramp && mapped.abs_diff(point.rank) <= 1
            })
            .map(|c| 1 << c)
            .sum();
    }
    ensure!(!points.is_empty(), "DfT input has no admitted observations");
    Ok(points.into_iter().map(|point| point.row).collect())
}
