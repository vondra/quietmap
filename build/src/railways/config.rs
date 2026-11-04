//! Manifest-selected source objects and publisher adapters; no edition or path selection in code.
use crate::year::{Kind, Manifest};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RailPlan {
    pub objects: Vec<RailObject>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RailObject {
    Gtfs {
        input: String,
        source_id: u16,
        country: String,
        directories: Vec<String>,
        #[serde(default)]
        ignore_feed_window: bool,
        rail_route_types: Vec<u16>,
        tram_route_types: Vec<u16>,
        #[serde(default)]
        excluded_route_names: Vec<String>,
        #[serde(default)]
        excluded_agency_names: Vec<String>,
    },
    FraCrossings {
        input: String,
        source_id: u16,
    },
    TcCrossings {
        input: String,
        source_id: u16,
    },
    Czptt {
        input: String,
        stations: String,
        source_id: u16,
        country: String,
    },
    PreparedMovements {
        input: String,
    },
    Geojson {
        input: String,
        source_id: u16,
        country: String,
        category: String,
        rail_types: Vec<u8>,
        radius_m: f64,
        fields: std::collections::BTreeMap<String, String>,
        accepted: std::collections::BTreeMap<String, Vec<String>>,
        excluded: std::collections::BTreeMap<String, Vec<String>>,
    },
}

impl RailObject {
    pub fn input(&self) -> (&str, Kind) {
        match self {
            Self::Gtfs { input, .. } => (input, Kind::PrepareTimetable),
            Self::FraCrossings { input, .. } | Self::TcCrossings { input, .. } => {
                (input, Kind::PrepareCrossings)
            }
            Self::Czptt { input, .. } => (input, Kind::PrepareRailNetwork),
            Self::PreparedMovements { input } | Self::Geojson { input, .. } => {
                (input, Kind::PrepareRailNetwork)
            }
        }
    }
}

impl RailPlan {
    pub fn validate(&self, manifest: &Manifest) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        let mut dependencies = std::collections::BTreeSet::new();
        for object in &self.objects {
            let (id, kind) = object.input();
            if !seen.insert(id)
                || !manifest
                    .inputs
                    .iter()
                    .any(|input| input.id == id && input.kind == kind)
            {
                return Err(format!("railways: repeated or undeclared input {id}"));
            }
            if manifest.admitted(id)? {
                dependencies.insert(id);
                if let RailObject::Czptt { stations, .. } = object {
                    dependencies.insert(stations.as_str());
                }
            }
            if let RailObject::FraCrossings { source_id, .. }
            | RailObject::TcCrossings { source_id, .. } = object
                && *source_id == 0
            {
                return Err(format!("{id}: crossing source must be attributed"));
            }
            if let RailObject::Geojson {
                source_id,
                country,
                category,
                rail_types,
                radius_m,
                ..
            } = object
                && (*source_id == 0
                    || country.len() != 2
                    || !country.bytes().all(|byte| byte.is_ascii_uppercase())
                    || category.is_empty()
                    || rail_types.is_empty()
                    || rail_types.iter().any(|kind| *kind > 5)
                    || !radius_m.is_finite()
                    || *radius_m <= 0.0)
            {
                return Err(format!("{id}: invalid spatial rail source"));
            }
            if let RailObject::Czptt {
                stations,
                source_id,
                country,
                ..
            } = object
                && (*source_id == 0
                    || country.len() != 2
                    || !country.bytes().all(|byte| byte.is_ascii_uppercase())
                    || !manifest.inputs.iter().any(|input| {
                        input.id == *stations && input.kind == Kind::PrepareRailNetwork
                    }))
            {
                return Err(format!(
                    "{id}: invalid CIS station input or source identity"
                ));
            }
            if let RailObject::Gtfs {
                source_id,
                country,
                directories,
                rail_route_types,
                tram_route_types,
                ..
            } = object
            {
                if *source_id == 0
                    || country.len() != 2
                    || !country.bytes().all(|c| c.is_ascii_uppercase())
                    || directories.is_empty()
                    || rail_route_types
                        .iter()
                        .any(|kind| tram_route_types.contains(kind))
                {
                    return Err(format!("{id}: invalid GTFS identity or route families"));
                }
                for directory in directories {
                    if directory.is_empty()
                        || std::path::Path::new(directory).components().any(|part| {
                            !matches!(
                                part,
                                std::path::Component::Normal(_) | std::path::Component::CurDir
                            )
                        })
                    {
                        return Err(format!(
                            "{id}: GTFS directory must stay inside its selected object"
                        ));
                    }
                }
            }
        }
        let streams: Vec<_> = manifest
            .inputs
            .iter()
            .filter(|input| input.kind == Kind::RailFlows)
            .collect();
        if streams.len() != 1
            || streams[0]
                .depends_on
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
                != dependencies
        {
            return Err("rail movement dependencies must name exactly the admitted publisher objects and station lookup".into());
        }
        Ok(())
    }
}
