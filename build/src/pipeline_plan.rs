//! Ordered raw preparation stages and their spatial dependency rings.
use crate::{dev4::Square, year::Kind};
use std::collections::BTreeSet;

pub(crate) fn check_generated_aliases(
    manifest: &crate::year::Manifest,
    sources: &std::path::Path,
) -> anyhow::Result<()> {
    let produced = produced_kinds();
    let conflicts: Vec<_> = manifest
        .inputs
        .iter()
        .filter(|input| produced.contains(&input.kind))
        .filter(|input| {
            let alias = sources.join(&manifest.year).join(&input.id);
            alias.symlink_metadata().is_ok()
                || (input.kind == Kind::OsmGeography
                    && ["railway-ways", "train-routes"].iter().any(|family| {
                        crate::osm::year_sibling_path(&alias, family)
                            .is_ok_and(|path| path.symlink_metadata().is_ok())
                    }))
        })
        .map(|input| &input.id)
        .collect();
    anyhow::ensure!(
        conflicts.is_empty(),
        "stage only publisher inputs and models; the year runner produces {conflicts:?}"
    );
    Ok(())
}

pub(crate) fn check_publisher_inputs(inputs: &crate::year::YearInputs) -> anyhow::Result<()> {
    let produced = produced_kinds();
    let mut declared = BTreeSet::new();
    let mut admitted = BTreeSet::new();
    for input in &inputs.manifest.inputs {
        if produced.contains(&input.kind) {
            continue;
        }
        declared.insert(input.kind);
        if inputs
            .manifest
            .admitted(&input.id)
            .map_err(anyhow::Error::msg)?
        {
            admitted.insert(input.kind);
            inputs.by_id(&input.id).map_err(anyhow::Error::msg)?;
        }
    }
    for kind in declared.difference(&admitted) {
        anyhow::ensure!(
            inputs.manifest.unavailable.iter().any(|i| i.kind == *kind),
            "year {}: no admitted publisher input or declared absence for {kind}",
            inputs.manifest.year
        );
    }
    Ok(())
}

/// Kinds the year run produces: never staged, published from the run where they are tables.
pub(crate) fn produced_kinds() -> BTreeSet<Kind> {
    steps()
        .into_iter()
        .filter_map(|step| step.binding)
        .collect()
}

pub(crate) struct Step {
    pub name: &'static str,
    pub dependencies: &'static [&'static str],
    pub binding: Option<Kind>,
    pub radius: Option<usize>,
    pub native: bool,
}

pub(crate) fn steps() -> Vec<Step> {
    use Kind::*;
    [
        ("osm-regions", &[][..], Some(PrepareRegions), None, false),
        ("osm", &["osm-regions"], Some(OsmGeography), Some(3), false),
        (
            "structure-observations",
            &[],
            Some(StructureObservations),
            Some(2),
            false,
        ),
        (
            "structures",
            &["osm", "structure-observations"],
            Some(Structures),
            Some(2),
            false,
        ),
        // A fit of the year's own mapped roads, before the sources that read it.
        (
            "tagged-speed-table",
            &["osm", "structures"],
            Some(TaggedSpeeds),
            None,
            false,
        ),
        (
            "facility-observations",
            &["osm-regions"],
            Some(FacilityObservations),
            None,
            false,
        ),
        (
            "facilities",
            &["osm", "facility-observations"],
            Some(Facilities),
            Some(2),
            false,
        ),
        (
            "road-observations",
            &[],
            Some(RoadObservations),
            None,
            false,
        ),
        (
            "roads",
            &["osm", "structures", "road-observations"],
            None,
            Some(1),
            false,
        ),
        ("rail-movements", &[], Some(RailFlows), None, false),
        (
            "prepare-railways",
            &["osm", "rail-movements"],
            None,
            Some(1),
            false,
        ),
        ("ships", &[], Some(Ships), Some(0), false),
        ("prepare-rasters", &[], Some(Rasters), Some(3), false),
        ("weather", &[], Some(Weather), None, false),
        ("geoid", &[], Some(Geoid), None, false),
        ("bus", &[], Some(Bus), None, false),
        ("worship", &[], Some(Worship), None, false),
        ("venues", &[], Some(Venues), None, false),
        // A fit of the year's mapped places and dated seating surveys, before the sources.
        (
            "venue-seating-table",
            &["structures", "venues"],
            Some(VenueSeating),
            None,
            false,
        ),
        (
            "prepared",
            &["osm", "structures", "roads", "prepare-railways"],
            Some(Prepared),
            Some(1),
            false,
        ),
        (
            "terrain",
            &["prepare-rasters"],
            Some(Terrain),
            Some(2),
            true,
        ),
        ("terrain-fine", &["terrain"], None, Some(0), true),
        ("obstacles", &["structures"], None, Some(0), true),
        (
            "aircraft-segments",
            &["prepare-rasters", "geoid"],
            Some(Segments),
            None,
            false,
        ),
        (
            "aircraft-shuffle",
            &["aircraft-segments"],
            Some(Shuffled),
            Some(0),
            false,
        ),
        (
            "airport-traffic",
            &["osm", "aircraft-segments", "terrain"],
            Some(AirportTraffic),
            Some(0),
            false,
        ),
        // A fit of the year's own prepared railways to the national train-km.
        (
            "train-scale-table",
            &["prepared"],
            Some(TrainScales),
            None,
            false,
        ),
        ("traffic", &["prepared"], Some(RoadTraffic), Some(1), false),
        // Fits of the year's counted roads and the trips its buildings make, before the sources.
        (
            "building-road-models",
            &["prepared", "traffic"],
            Some(BuildingRoadModels),
            None,
            false,
        ),
        (
            "road-reference-counts",
            &["prepared", "traffic", "building-road-models"],
            Some(RoadReferenceCounts),
            None,
            false,
        ),
        (
            "sources",
            &[
                "prepared",
                "tagged-speed-table",
                "train-scale-table",
                "facilities",
                "traffic",
                "building-road-models",
                "road-reference-counts",
                "airport-traffic",
                "ships",
                "terrain",
                "bus",
                "worship",
                "venues",
                "venue-seating-table",
            ],
            None,
            Some(0),
            true,
        ),
        ("source-outlines", &["facilities"], None, Some(2), false),
        (
            "aircraft-boxes",
            &["aircraft-shuffle", "terrain", "weather"],
            None,
            Some(0),
            true,
        ),
        (
            "aircraft-events",
            &["aircraft-shuffle", "terrain", "weather"],
            None,
            Some(0),
            true,
        ),
        ("aircraft-tracks", &["aircraft-segments"], None, None, false),
    ]
    .into_iter()
    .map(|(name, dependencies, binding, radius, native)| Step {
        name,
        dependencies,
        binding,
        radius,
        native,
    })
    .collect()
}

pub(crate) fn context(squares: &[Square], radius: usize) -> Vec<Square> {
    let mut result: BTreeSet<_> = squares.iter().copied().collect();
    for _ in 0..radius {
        result = result
            .into_iter()
            .flat_map(Square::with_neighbours)
            .collect();
    }
    result.into_iter().collect()
}
