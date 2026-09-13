//! Resolve source traffic and priors once: input, physical allocation, staged Arrow output.

mod allocation;
mod input;
mod spatial;
mod write;

use grid::Square;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn road_path(year: &Path, square: Square) -> PathBuf {
    year.join(grid::square_name(square)).join("roads.arrow")
}

fn squares(year: &Path) -> Result<Vec<Square>, String> {
    let mut result = Vec::new();
    for x in std::fs::read_dir(year.join("z9")).map_err(|e| e.to_string())? {
        let x = x.map_err(|e| e.to_string())?;
        let Ok(xi) = x.file_name().to_string_lossy().parse::<u16>() else { continue; };
        if xi >= 512 { return Err("invalid z9 x".to_owned()); }
        for y in std::fs::read_dir(x.path()).map_err(|e| e.to_string())? {
            let y = y.map_err(|e| e.to_string())?;
            let Ok(yi) = y.file_name().to_string_lossy().parse::<u16>() else { continue; };
            if yi >= 512 { return Err("invalid z9 y".to_owned()); }
            let square = Square { x: xi, y: yi };
            if road_path(year, square).is_file() { result.push(square); }
        }
    }
    result.sort_by_key(|s| (s.x, s.y));
    Ok(result)
}

fn verify_pieces(database: &Connection, square: Square, roads: &[input::Road]) -> Result<(), String> {
    let mut query = database.prepare_cached("SELECT p.way_id,p.segment_idx FROM source_pieces p JOIN source_ways w ON w.osm_id=p.way_id WHERE p.square=? AND w.family='roads'")
        .map_err(|e| e.to_string())?;
    let identities = query.query_map([grid::square_name(square)], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i16>(1)?)))
        .map_err(|e| e.to_string())?.collect::<Result<HashSet<_>, _>>().map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    for road in roads {
        let identity = (road.way_id, road.segment_idx);
        if !identities.contains(&identity) || !seen.insert(identity) {
            return Err(format!("missing or duplicate source road piece {}:{}", road.way_id, road.segment_idx));
        }
    }
    Ok(())
}

fn promote(year: &Path, staging: &Path, squares: &[Square]) -> Result<usize, String> {
    let mut count = 0;
    for square in squares {
        let source = road_path(staging, *square);
        if source.is_file() {
            let target = road_path(year, *square);
            std::fs::rename(&source, &target).map_err(|e| e.to_string())?;
            std::fs::File::open(target.parent().ok_or("road parent missing")?)
                .and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    std::fs::remove_dir_all(staging).map_err(|e| e.to_string())?;
    Ok(count)
}

pub fn finalize_year(year: &Path) -> Result<usize, String> {
    let squares = squares(year)?;
    let staging = year.join(".roads-finalize");
    // Nothing is promoted until every halo has read the immutable input. The
    // marker makes an interrupted promotion resume without re-reading a mixed
    // generation or allocating an already allocated neighboring carriageway.
    if staging.join("ready").is_file() { return promote(year, &staging, &squares); }
    if staging.exists() { std::fs::remove_dir_all(&staging).map_err(|e| e.to_string())?; }
    let parent = year.parent().ok_or("prepared year has no parent")?;
    let name = year.file_name().ok_or("prepared year has no name")?.to_string_lossy();
    let database = Connection::open_with_flags(parent.join(format!("{name}.transport.sqlite")), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let version: i64 = database.query_row("PRAGMA user_version", [], |row| row.get(0)).map_err(|e| e.to_string())?;
    if version != 2 { return Err("source transport topology schema must be 2".to_owned()); }
    let mut changed = 0;
    for (position, square) in squares.iter().enumerate() {
        let batches = input::load(&road_path(year, *square))?;
        let own = batches.iter().map(input::roads).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect::<Vec<_>>();
        if batches[0].schema().metadata().get(input::CONTRACT).map(String::as_str) == Some("1") { continue; }
        verify_pieces(&database, *square, &own)?;
        let mut neighbors = own.iter().filter(|r| r.direction != 0).cloned().collect::<Vec<_>>();
        if !neighbors.is_empty() {
            let corridor_key = |r: &input::Road| (r.class, r.country.country_iso, r.country.city_id, r.corridor.clone());
            let observation_key = |r: &input::Road| (r.class, r.country.country_iso, r.country.city_id, r.observation_source_id, r.observation.clone());
            let corridors = neighbors.iter().filter(|r| !r.corridor.is_empty()).map(corridor_key).collect::<HashSet<_>>();
            let observations = neighbors.iter().filter(|r| !r.observation.is_empty()).map(observation_key).collect::<HashSet<_>>();
            let center_x = neighbors[0].midpoint().0;
            for neighbor in grid::ring_squares(*square, 0.0) {
                if neighbor == *square { continue; }
                let path = road_path(year, neighbor);
                if !path.is_file() { continue; }
                for batch in input::load(&path)? {
                    for mut candidate in input::roads(&batch)? {
                        if candidate.direction == 0 { continue; }
                        let wrap = ((center_x - candidate.midpoint().0) / grid::EARTH_CIRCUMFERENCE_M).round()
                            * grid::EARTH_CIRCUMFERENCE_M;
                        candidate.start.0 += wrap;
                        candidate.end.0 += wrap;
                        if corridors.contains(&corridor_key(&candidate)) || observations.contains(&observation_key(&candidate)) {
                            neighbors.push(candidate);
                        }
                    }
                }
            }
        }
        let index = spatial::RoadIndex::new(neighbors);
        let target = road_path(&staging, *square);
        std::fs::create_dir_all(target.parent().ok_or("road staging parent missing")?).map_err(|e| e.to_string())?;
        write::stage(&target, &batches, &index)?;
        changed += 1;
        if position % 1000 == 0 { eprintln!("roads-finalize: {}/{} squares", position + 1, squares.len()); }
    }
    if changed == 0 { return Ok(0); }
    let marker = std::fs::File::create(staging.join("ready")).map_err(|e| e.to_string())?;
    marker.sync_all().map_err(|e| e.to_string())?;
    std::fs::File::open(&staging).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
    promote(year, &staging, &squares)
}

#[cfg(test)]
mod tests;
