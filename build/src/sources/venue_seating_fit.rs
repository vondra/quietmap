//! The venue-seating table a year run fits (`qm-build venue-seating-table`) from its dated seating
//! surveys and its own mapped places: each seated kind's outdoor seats where a terrace is mapped
//! (the surveyed median of the venues with seats) and where none is (the surveyed mean over every
//! venue, those without seats too), the median mapped beer garden, and the kind hosted by the most
//! buildings of bars and restaurants, whose hours a building without a mapped place keeps.

use super::building_rows::{host_places, read_batch};
use super::venues::{SEATED_KINDS, VenueKind, Venues};
use crate::dev4::{Dev4, Square, require_stamp};
use crate::structures::height_calibration::median;
use crate::year::{Kind, YearInputs};
use anyhow::{Context, Result, anyhow, ensure};
use physics::emission::settlement::HOSPITALITY;
use rayon::prelude::*;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Every kind, in the order the hosting counts keep them.
const KINDS: [VenueKind; 7] = [
    VenueKind::Bar,
    VenueKind::Pub,
    VenueKind::Nightclub,
    VenueKind::Biergarten,
    VenueKind::Restaurant,
    VenueKind::Cafe,
    VenueKind::FastFood,
];

fn kind_index(kind: VenueKind) -> usize {
    KINDS.iter().position(|&other| other == kind).unwrap_or(0)
}

/// A kind's name as the table writes it.
fn kind_name(kind: VenueKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{kind:?}"),
    }
}

/// The admitted seating surveys: the model observations the manifest's venue-seating entry
/// depends on, each `kind seats` per surveyed venue (`fetch/terrace_surveys.py`).
fn survey_paths(inputs: &YearInputs) -> Result<Vec<PathBuf>> {
    let manifest = &inputs.manifest;
    let entry = manifest
        .inputs
        .iter()
        .find(|input| input.kind == Kind::VenueSeating)
        .ok_or_else(|| anyhow!("year {}: no venue-seating declaration", manifest.year))?;
    let paths = entry
        .depends_on
        .iter()
        .filter(|id| {
            manifest
                .inputs
                .iter()
                .any(|input| input.id == **id && input.kind == Kind::ModelObservations)
        })
        .map(|id| inputs.by_id(id).map_err(anyhow::Error::msg))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !paths.is_empty(),
        "venue seating depends on no seating survey"
    );
    Ok(paths)
}

/// `(kind, seats)` of every surveyed venue.
pub(crate) fn parse_survey(text: &str) -> Result<Vec<(VenueKind, f64)>> {
    text.lines()
        .enumerate()
        .map(|(number, line)| {
            let (kind, seats) = line
                .split_once('\t')
                .ok_or_else(|| anyhow!("line {}: expected two fields", number + 1))?;
            let kind = VenueKind::from_amenity(kind)
                .ok_or_else(|| anyhow!("line {}: unknown kind {kind}", number + 1))?;
            let seats: f64 = seats
                .parse()
                .with_context(|| format!("line {}", number + 1))?;
            ensure!(seats >= 0.0, "line {}: negative seats", number + 1);
            Ok((kind, seats))
        })
        .collect()
}

/// Each seated kind's `[tagged, expected]` seats, as the table keeps them.
type Seats = [[f64; 2]; 6];
/// Each seated kind's `[venues, with seats]` surveyed.
type Surveyed = [[usize; 2]; 6];

/// Each seated kind's seats and survey counts.
pub(crate) fn seats(surveyed: &[(VenueKind, f64)]) -> Result<(Seats, Surveyed)> {
    let mut seats = [[0.0; 2]; 6];
    let mut counts = [[0; 2]; 6];
    for (index, &kind) in SEATED_KINDS.iter().enumerate() {
        let all: Vec<f64> = surveyed
            .iter()
            .filter(|(surveyed_kind, _)| *surveyed_kind == kind)
            .map(|&(_, seats)| seats)
            .collect();
        let mut with_seats: Vec<f64> = all.iter().copied().filter(|&seats| seats > 0.0).collect();
        ensure!(
            !with_seats.is_empty(),
            "no surveyed {kind:?} has outdoor seats"
        );
        seats[index] = [
            median(&mut with_seats),
            all.iter().sum::<f64>() / all.len() as f64,
        ];
        counts[index] = [all.len(), with_seats.len()];
    }
    Ok((seats, counts))
}

/// The buildings of bars, restaurants and cafés with a footprint in the squares with mapped places,
/// how many host one, and how many host each kind.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct Hosting {
    pub buildings: u64,
    pub hosting: u64,
    pub by_kind: [u64; 7],
}

impl Hosting {
    /// One square's buildings and their places, as the sources host them.
    pub(crate) fn add_square(
        &mut self,
        batches: &[arrow_array::RecordBatch],
        square: Square,
        sites: &[super::venues::VenueSite],
    ) -> Result<(), String> {
        let mut hospitality: HashMap<u64, u64> = HashMap::new();
        for batch in batches {
            for row in read_batch(batch, square)? {
                if row.class == HOSPITALITY && row.footprint_id != 0 {
                    let next = hospitality.len() as u64;
                    hospitality.entry(row.footprint_id).or_insert(next);
                }
            }
        }
        let placer = host_places(batches, (square, &hospitality), sites)?;
        let mut kinds: HashMap<u64, [bool; 7]> = HashMap::new();
        for (site, host) in sites.iter().zip(placer.hosts()) {
            if let Some(host) = host {
                kinds.entry(*host).or_default()[kind_index(site.kind)] = true;
            }
        }
        self.buildings += hospitality.len() as u64;
        self.hosting += kinds.len() as u64;
        for hosted in kinds.values() {
            for (count, &hosts) in self.by_kind.iter_mut().zip(hosted) {
                *count += u64::from(hosts);
            }
        }
        Ok(())
    }

    fn merge(&mut self, other: Hosting) {
        self.buildings += other.buildings;
        self.hosting += other.hosting;
        for (total, count) in self.by_kind.iter_mut().zip(other.by_kind) {
            *total += count;
        }
    }

    /// The kind most buildings host (the first of [`KINDS`] on a tie).
    pub(crate) fn predominant(&self) -> Result<VenueKind> {
        ensure!(
            self.hosting > 0,
            "no building of bars and restaurants hosts a mapped place"
        );
        let most = *self.by_kind.iter().max().unwrap_or(&0);
        let index = self.by_kind.iter().position(|&count| count == most);
        Ok(KINDS[index.unwrap_or(0)])
    }
}

pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "venue seating needs a new output path");
    let mut surveyed = Vec::new();
    for path in survey_paths(inputs)? {
        let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        surveyed.extend(parse_survey(&text).with_context(|| path.display().to_string())?);
    }
    let (seats, counts) = seats(&surveyed)?;
    let venues = Venues::load(inputs.required(Kind::Venues).map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    let mut gardens: Vec<f64> = venues
        .by_square()
        .flat_map(|(_, sites)| sites)
        .filter(|site| site.kind == VenueKind::Biergarten && site.area_m2 > 0.0)
        .map(|site| site.area_m2)
        .collect();
    ensure!(
        !gardens.is_empty(),
        "no beer garden is mapped as an outline"
    );
    let structures = Dev4 {
        prepared: inputs
            .required(Kind::Structures)
            .map_err(anyhow::Error::msg)?
            .into(),
        rasters: PathBuf::new(),
    };
    let squares: Vec<_> = venues.by_square().collect();
    let counted = squares
        .par_iter()
        .map(|&((x, y), sites)| -> Result<Option<Hosting>> {
            let square = Square { x, y };
            let Some(table) = structures
                .table(square, "structures.arrow")
                .map_err(anyhow::Error::msg)?
            else {
                return Ok(None);
            };
            let context = |error: String| anyhow!("structures.arrow of {square:?}: {error}");
            require_stamp(&table, "structures_contract", "structures_v5").map_err(context)?;
            let mut hosting = Hosting::default();
            hosting
                .add_square(&table.batches, square, sites)
                .map_err(context)?;
            Ok(Some(hosting))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut hosting = Hosting::default();
    let mut squares = 0;
    for square in counted.into_iter().flatten() {
        hosting.merge(square);
        squares += 1;
    }
    eprintln!("venue-seating-fit: {squares} squares with places and structures");
    let row = json!({
        "beer_garden_m2": median(&mut gardens),
        "seats": seats,
        "unmapped_kind": hosting.predominant()?,
    });
    std::fs::write(output, serde_json::to_vec_pretty(&json!({"rows": [row]}))?)?;
    let surveyed: serde_json::Map<_, _> = SEATED_KINDS
        .iter()
        .zip(counts)
        .map(|(&kind, count)| (kind_name(kind), json!(count)))
        .collect();
    let hosted: serde_json::Map<_, _> = KINDS
        .iter()
        .zip(hosting.by_kind)
        .map(|(&kind, count)| (kind_name(kind), json!(count)))
        .collect();
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&json!({
            "surveyed_venues_and_with_seats": surveyed,
            "beer_garden_outlines": gardens.len(),
            "squares_with_places_and_structures": squares,
            "hospitality_buildings": hosting.buildings,
            "hosting_a_place": hosting.hosting,
            "hosting_each_kind": hosted,
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "venue_seating_fit_tests.rs"]
mod tests;
