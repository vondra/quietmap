//! Dated carriageway priors and access/lane factors supplied by the year manifest.

use super::input::Road;
use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub continuity_classes: Vec<u8>,
    pub speed_transitions: super::speed_transitions::Parameters,
    pub demand: crate::traffic::preparation_model::DemandModel,
    pub world: [[f64; 4]; 13],
    #[serde(default)]
    pub observation_shares: std::collections::BTreeMap<u16, [[f64; 4]; 13]>,
    #[serde(default)]
    pub weekday_to_annual_factor: Option<f64>,
    pub carriageways: Vec<[[[f64; 2]; 3]; 2]>,
    pub one_way_section_classes: Vec<u8>,
    pub regions: Vec<RegionalPrior>,
    pub access: [f64; 9],
    pub untagged_track_access: f64,
    pub lanes: Vec<LaneFactor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalPrior {
    pub country: u16,
    pub city: Option<u16>,
    pub classes: Vec<[f64; 4]>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneFactor {
    pub class: u8,
    pub one_way: bool,
    pub lanes: u8,
    pub factor: f64,
}

pub enum TrafficDefault {
    SectionBothDirections([f64; 4]),
    Carriageway([f64; 4]),
}

impl Model {
    pub fn read(inputs: &crate::year::YearInputs) -> Result<Self> {
        let path = inputs
            .required(crate::year::Kind::RoadPreparationModel)
            .map_err(anyhow::Error::msg)?;
        let model: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.weekday_to_annual_factor
                .is_none_or(|v| v.is_finite() && v > 0.0),
            "invalid weekday conversion"
        );
        self.demand.validate()?;
        self.speed_transitions.validate()?;
        ensure!(
            self.continuity_classes.iter().all(|c| *c < 13),
            "invalid continuity classes"
        );
        ensure!(
            self.carriageways.len() <= 13,
            "too many carriageway classes"
        );
        for classes in std::iter::once(self.world.as_slice())
            .chain(self.regions.iter().map(|r| r.classes.as_slice()))
            .chain(self.observation_shares.values().map(|r| r.as_slice()))
        {
            ensure!(
                classes.len() <= 13
                    && classes
                        .iter()
                        .all(|v| v.iter().all(|x| x.is_finite() && *x >= 0.0)
                            && v.iter().sum::<f64>() > 0.0),
                "invalid road class prior"
            );
        }
        ensure!(
            self.carriageways
                .iter()
                .flatten()
                .flatten()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.0),
            "invalid fitted carriageway prior"
        );
        ensure!(
            self.access
                .iter()
                .chain([&self.untagged_track_access])
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "invalid access factor"
        );
        ensure!(
            self.lanes
                .iter()
                .all(|r| r.class < 13 && r.lanes > 2 && r.factor.is_finite() && r.factor > 0.0),
            "invalid lane factor"
        );
        let mut regions = std::collections::BTreeSet::new();
        ensure!(
            self.regions
                .iter()
                .all(|r| regions.insert((r.country, r.city))),
            "duplicate regional prior"
        );
        Ok(())
    }

    pub fn native_class_priors(&self) -> [[f64; 3]; 2] {
        std::array::from_fn(|row| {
            let class = row + 3;
            self.carriageways
                .get(class)
                .map_or([self.world[class].iter().sum(); 3], |priors| {
                    priors[1].map(|cell| cell[1])
                })
        })
    }

    pub fn default_traffic(&self, road: &Road) -> TrafficDefault {
        let one_way = road.direction != 0 && !road.roundabout;
        for city in [Some(road.country.1), None] {
            if let Some(counts) = self
                .regions
                .iter()
                .find(|r| r.country == road.country.0 && r.city == city)
                .and_then(|r| r.classes.get(usize::from(road.class)))
            {
                return TrafficDefault::SectionBothDirections(*counts);
            }
        }
        let world = self.world[usize::from(road.class)];
        let Some(priors) = self.carriageways.get(usize::from(road.class)) else {
            return TrafficDefault::SectionBothDirections(world);
        };
        let place = usize::from(road.built_up.min(2));
        let section = one_way && self.one_way_section_classes.contains(&road.class);
        let [per_lane, untagged] = priors[usize::from(!one_way || section)][place];
        let total = if !section && per_lane > 0.0 && (1..=6).contains(&road.lanes) {
            f64::from(road.lanes) * per_lane
        } else {
            untagged
        };
        let factor = total / world.iter().sum::<f64>();
        let counts = world.map(|value| value * factor);
        if section {
            TrafficDefault::SectionBothDirections(counts)
        } else {
            TrafficDefault::Carriageway(counts)
        }
    }

    pub fn access_factor(&self, road: &Road) -> f64 {
        if road.count_origin_measured {
            1.0
        } else if road.class == 8 && road.access == 0 {
            self.untagged_track_access
        } else {
            self.access[usize::from(road.access)]
        }
    }

    pub fn lane_factor(&self, road: &Road) -> f64 {
        self.lanes
            .iter()
            .filter(|r| {
                r.class == road.class && r.one_way == (road.direction != 0) && road.lanes >= r.lanes
            })
            .max_by_key(|r| r.lanes)
            .map_or(1.0, |r| r.factor)
    }
}
