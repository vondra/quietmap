//! Dispatch manifest-selected preparation and native year-tree builders.
use crate::{
    aircraft, airport, barriers, cli::*, climate, dev4::Dev4, facilities, facility_inputs,
    obstacles, osm, osm_regions, rail_inputs, railways, rasters, road_counts, roads, ships,
    source_outlines, source_tables, sources, structure_inputs, structures, terrain, terrain_fine,
    traffic, weather, year,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) fn run(arguments: &[String]) -> Result<(), String> {
    let (command, rest) = arguments.split_first().ok_or(
        "usage: qm-build COMMAND --year MANIFEST --sources ROOT --out OUTPUT [--key value ...]",
    )?;
    let options = Arguments::parse(rest)?;
    let out = PathBuf::from(options.get("out")?);
    if command == "year" {
        return crate::pipeline::build(&options).map_err(|error| format!("{error:#}"));
    }
    if command == "input-kinds" {
        let text = year::Kind::typescript();
        return if out == Path::new("-") {
            print!("{text}");
            Ok(())
        } else {
            std::fs::write(out, text).map_err(|e| e.to_string())
        };
    }
    if command == "manifest" {
        let path = Path::new(options.get("year")?);
        let manifest = year::Manifest::read(path)?;
        let mut projection: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let admitted = manifest
            .inputs
            .iter()
            .filter_map(|input| match manifest.admitted(&input.id) {
                Ok(true) => Some(Ok(input.id.as_str())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<HashSet<_>, _>>()?;
        projection["inputs"]
            .as_array_mut()
            .ok_or("manifest inputs must be an array")?
            .retain(|input| input["id"].as_str().is_some_and(|id| admitted.contains(id)));
        let json = serde_json::to_string(&projection).map_err(|e| e.to_string())?;
        if out == Path::new("-") {
            println!("{json}");
            return Ok(());
        }
        return std::fs::write(out, json).map_err(|e| e.to_string());
    }
    if command == "complete" {
        let path = Path::new(options.get("year")?);
        let inputs = year::YearInputs::read(path, Path::new(options.get("sources")?))?;
        source_tables::SourceTables::read(&inputs)?;
        inputs.check_published_tables()?;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return tiles::mark_year_complete(&out, &bytes, options.get("note")?);
    }
    let inputs = year::YearInputs::read(
        Path::new(options.get("year")?),
        Path::new(options.get("sources")?),
    )?;
    match command.as_str() {
        "rail-movements" => rail_inputs::convert(&inputs, &out, Path::new(options.get("scratch")?)),
        "prepare-railways" => {
            railways::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
        }
        "tagged-speed-table" => {
            roads::tagged_speed_fit::fit(&inputs, &out).map_err(|e| e.to_string())
        }
        "road-observations" => road_counts::convert(&inputs, &out).map_err(|e| e.to_string()),
        "road-model-tables" => roads::calibration::build(&inputs, &out).map_err(|e| e.to_string()),
        "road-speed-table" => roads::speed_validity::build(&inputs, options.get("kind")?, &out)
            .map_err(|e| e.to_string()),
        "roads" => roads::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
            .map_err(|e| e.to_string()),
        "structure-height-model" => structures::height_calibration::fit(
            &inputs,
            &parse_squares(options.get("squares")?)?,
            &out,
        )
        .map_err(|e| e.to_string()),
        "structure-observations" => {
            structure_inputs::convert(&inputs, &parse_squares(options.get("squares")?)?, &out)
                .map_err(|e| e.to_string())
        }
        "structures" => {
            structures::prepare::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
                .map_err(|e| e.to_string())
        }
        "facility-observations" => {
            facility_inputs::convert(&inputs, &out).map_err(|e| e.to_string())
        }
        "facilities" => {
            facilities::prepare::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
                .map_err(|e| e.to_string())
        }
        "osm-regions" => osm_regions::convert(&inputs, &out).map_err(|error| error.to_string()),
        "osm" => osm::build(
            &inputs,
            &parse_squares(options.get("squares")?)?,
            &out,
            Path::new(options.get("scratch")?),
        )
        .map_err(|error| error.to_string()),
        "prepare-rasters" => {
            rasters::build(&inputs, &parse_squares(options.get("squares")?)?, &out)
        }
        "source-outlines" => source_outlines::build(
            &facilities::Dataset::open(&inputs)?,
            &parse_squares(options.get("squares")?)?,
            &out,
        ),
        "terrain" | "obstacles" | "sources" => {
            let dev4 = Dev4 {
                prepared: match command.as_str() {
                    "terrain" => PathBuf::new(),
                    "obstacles" => inputs.required(crate::year::Kind::Structures)?.into(),
                    _ => inputs.required(crate::year::Kind::Prepared)?.into(),
                },
                rasters: if command == "obstacles" {
                    PathBuf::new()
                } else {
                    inputs.required(crate::year::Kind::Rasters)?.into()
                },
            };
            let squares = parse_squares(options.get("squares")?)?;
            let kind = command.as_str();
            let started = std::time::Instant::now();
            let written = match kind {
                "terrain" => terrain::build(&dev4, &squares, &national_models(&inputs)?, &out)?,
                "obstacles" => obstacles::build(
                    &dev4,
                    &barriers::Barriers::from_year(&inputs)?,
                    &squares,
                    &out,
                )?,
                "sources" => sources::build(
                    (
                        &dev4,
                        &climate::Climate::load(inputs.required(crate::year::Kind::Climate)?)?,
                        &sources::tent::FreightNetwork::load(
                            inputs.required(crate::year::Kind::Tent)?,
                        )?,
                        &sources::turbines::Turbines::load(
                            inputs.required(crate::year::Kind::Turbines)?,
                        )?,
                        &sources::water::Water::open(inputs.required(crate::year::Kind::Water)?)?,
                        ships::input_root(&inputs)?,
                        &source_tables::SourceTables::read(&inputs)?,
                    ),
                    (
                        inputs.required(crate::year::Kind::AirportTraffic)?,
                        Some(inputs.required(crate::year::Kind::RoadTraffic)?),
                        Some(&sources::bus::BusRoutes::load(
                            inputs.required(crate::year::Kind::Bus)?,
                        )?),
                        sources::Places {
                            facilities: &facilities::Dataset::open(&inputs)?,
                            worship: Some(&sources::worship::WorshipSites::load(
                                inputs.required(crate::year::Kind::Worship)?,
                            )?),
                            venues: Some(&sources::venues::Venues::load(
                                inputs.required(crate::year::Kind::Venues)?,
                            )?),
                        },
                    ),
                    &squares,
                    (options.terrain(&inputs, &out)?, &out),
                )?,
                other => return Err(format!("unknown kind {other}")),
            };
            if kind == "terrain" {
                crate::terrain_completion::write(&inputs, &out, &squares)?;
            }
            eprintln!(
                "{kind}: {written} tiles in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        "ships" => {
            let count = ships::build(&inputs, &parse_squares(options.get("squares")?)?, &out)?;
            eprintln!("ships: {count} occupancy cells");
            Ok(())
        }
        "weather" => weather::build(&inputs, &out),
        "bus" | "worship" | "venues" => {
            crate::osm_auxiliary::build(command, &inputs, &out, Path::new(options.get("scratch")?))
        }
        "terrain-fine" => {
            let started = std::time::Instant::now();
            let models: Vec<&Path> = inputs
                .paths(crate::year::Kind::FineDem)?
                .iter()
                .map(PathBuf::as_path)
                .collect();
            let written = terrain_fine::build(
                &parse_squares(options.get("squares")?)?,
                &models,
                options.terrain(&inputs, &out)?,
                &out,
            )?;
            eprintln!(
                "terrain-fine: {written} tiles in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        "geoid" => aircraft::geoid::build(inputs.required(crate::year::Kind::Tiff)?, &out),
        "aircraft-segments" | "aircraft-shuffle" | "aircraft-boxes" | "aircraft-events"
        | "aircraft-tracks" => crate::commands_aircraft::run(command, &options, &inputs, &out),
        "aircraft-check" => crate::commands_aircraft_check::run(command, &options, &inputs, &out),
        "traffic" => {
            let dev4 = Dev4 {
                prepared: inputs.required(crate::year::Kind::Prepared)?.into(),
                rasters: PathBuf::new(),
            };
            let started = std::time::Instant::now();
            let written = traffic::build(
                &source_tables::TripRates::read(&inputs)?,
                &dev4,
                &parse_squares(options.get("squares")?)?,
                &out,
            )?;
            eprintln!(
                "traffic: {written} squares in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        "airport-traffic" => {
            let dev4 = Dev4 {
                prepared: inputs.required(crate::year::Kind::OsmGeography)?.into(),
                rasters: PathBuf::new(),
            };
            let written = airport::build(
                &dev4,
                inputs.required(crate::year::Kind::Segments)?,
                &inputs.manifest.window(),
                &parse_squares(options.get("squares")?)?,
                (options.terrain(&inputs, &out)?, &out),
            )?;
            eprintln!("airport traffic: {written} squares with traffic");
            Ok(())
        }
        other => Err(format!("unknown command {other}")),
    }
}
