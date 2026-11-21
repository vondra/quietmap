//! Append the two directed sounding approaches of each crossing, merging shared soundings.
use super::{horn_geometry, horn_write, model::RailModel, types::Crossing};
use crate::osm::geometry::{Square, flat_dist};
use arrow_array::RecordBatch;
use arrow_select::concat::concat_batches;
use std::collections::HashMap;
use std::path::Path;

pub struct NeighborHorns<'a> {
    pub geography: &'a Path,
    pub all_crossings: &'a HashMap<[u16; 2], Vec<Crossing>>,
}

fn append_neighbor_approaches(
    approaches: &mut Vec<horn_geometry::Approach>,
    model: &RailModel,
    own_sounding: &[&Crossing],
    square: Square,
    neighbors: NeighborHorns<'_>,
    regions: Option<&crate::osm::geography::Regions>,
) -> Result<(), String> {
    let reach = horn_geometry::CROSS_SQUARE_HORN_REACH_M;
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (i32::from(square.x) + dx, i32::from(square.y) + dy);
            if !(0..512).contains(&nx) || !(0..512).contains(&ny) {
                continue;
            }
            let nb_key = [nx as u16, ny as u16];
            let Some(nb_list) = neighbors.all_crossings.get(&nb_key) else {
                continue;
            };
            let near: Vec<&Crossing> = nb_list
                .iter()
                .filter(|nb| {
                    nb.soundings.iter().any(|&s| s > 0.0)
                        && own_sounding
                            .iter()
                            .any(|own| flat_dist(own.lat, own.lon, nb.lat, nb.lon) <= reach)
                })
                .collect();
            if near.is_empty() {
                continue;
            }
            let path = neighbors
                .geography
                .join(format!("z9/{nx}/{ny}/railways.arrow"));
            let Some(table) = crate::dev4::read_table(&path)? else {
                continue;
            };
            let Some(first) = table.batches.first() else {
                continue;
            };
            let merged = concat_batches(&first.schema(), &table.batches)
                .map_err(|err| format!("concat neighbor horn railways: {err}"))?;
            let nb_rows = horn_geometry::base_rows(&merged)?;
            for crossing in near {
                if let Some((row, latitude, longitude)) =
                    horn_geometry::match_crossing(model, crossing, &nb_rows)
                {
                    approaches.extend(horn_geometry::build_approaches(
                        0, crossing, row, latitude, longitude, &nb_rows, regions, false,
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn append(
    base: &RecordBatch,
    model: &RailModel,
    crossings: &[Crossing],
    regions: Option<&crate::osm::geography::Regions>,
    square: Square,
    neighbors: Option<NeighborHorns<'_>>,
) -> Result<RecordBatch, String> {
    let own_sounding: Vec<&Crossing> = crossings
        .iter()
        .filter(|crossing| crossing.soundings.iter().any(|&s| s > 0.0))
        .collect();
    let sounding_count = own_sounding.len();
    if sounding_count == 0 {
        return Ok(base.clone());
    }
    let rows = horn_geometry::base_rows(base)?;
    let mut approaches = Vec::new();
    let mut unmatched = 0;
    for (index, crossing) in crossings.iter().enumerate() {
        if crossing.soundings.iter().all(|&s| s <= 0.0) {
            continue;
        }
        if let Some((row, latitude, longitude)) =
            horn_geometry::match_crossing(model, crossing, &rows)
        {
            approaches.extend(horn_geometry::build_approaches(
                index, crossing, row, latitude, longitude, &rows, regions, true,
            ));
        } else {
            unmatched += 1;
        }
    }
    if let Some(neighbors) = neighbors {
        append_neighbor_approaches(
            &mut approaches,
            model,
            &own_sounding,
            square,
            neighbors,
            regions,
        )?;
    }
    approaches.sort_by_key(|approach| (approach.synth_osm, approach.approach_idx));
    let pieces = horn_geometry::merge_overlaps(&approaches);
    if pieces.is_empty() {
        return Ok(base.clone());
    }
    let horns =
        horn_write::build_horn_batch(&base.schema(), base, crossings, &approaches, &pieces)?;
    eprintln!(
        "railways: {sounding_count} sounding crossings, {unmatched} unmatched, {} horn pieces",
        pieces.len()
    );
    concat_batches(&base.schema(), &[base.clone(), horns]).map_err(|error| error.to_string())
}
