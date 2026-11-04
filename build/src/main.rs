//! Builds one manifest's inputs into the popup and painter's files.
//! `qm-build COMMAND --year years/YEAR.json --sources ROOT --out OUTPUT`.
//! `year` runs the chain; spatial steps take `--squares X:Y,X:Y` (or `all`, or `@FILE`).
//! The source inventory and flow dates live only in the
//! manifest. Completion verifies the required model inputs and binds the output to its manifest.

#[path = "../../fetch/structure_inputs.rs"]
mod structure_inputs;

mod aircraft;
mod airport;
mod barriers;
mod boxes;
mod cli;
mod climate;
mod commands;
mod commands_aircraft;
mod commands_aircraft_check;
mod commands_airspace;
mod dev4;
#[path = "../../fetch/era5.rs"]
mod era5;
mod facilities;
#[path = "../../fetch/facilities.rs"]
mod facility_inputs;
mod low_profile;
mod obstacles;
mod osm;
mod osm_auxiliary;
#[path = "../../fetch/osm_regions.rs"]
mod osm_regions;
mod period;
mod pipeline;
mod pipeline_files;
mod pipeline_plan;
mod pipeline_storage;
#[cfg(test)]
mod pipeline_tests;
#[path = "../../fetch/rail_inputs.rs"]
mod rail_inputs;
mod railways;
mod rasters;
mod rings;
#[path = "../../fetch/road_counts.rs"]
mod road_counts;
mod roads;
mod screening;
#[path = "../../fetch/ships.rs"]
mod ship_fetch;
mod ships;
mod source_outlines;
mod source_tables;
mod sources;
mod stage1;
mod structures;
mod terrain;
mod terrain_completion;
mod terrain_fine;
mod traffic;
mod weather;
mod year;

#[cfg(test)]
pub(crate) use cli::parse_squares;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = commands::run(&arguments) {
        eprintln!("qm-build: {error}");
        std::process::exit(1);
    }
}
