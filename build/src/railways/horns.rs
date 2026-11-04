//! Append the two directed sounding approaches of each crossing, merging shared soundings.
use super::{horn_geometry, horn_write, types::Crossing};
use arrow_array::RecordBatch;
use arrow_select::concat::concat_batches;

pub fn append(base: &RecordBatch, crossings: &[Crossing]) -> Result<RecordBatch, String> {
    let (rows, types) = horn_geometry::base_rows(base)?;
    let mut approaches = Vec::new();
    let mut unmatched = 0;
    for (index, crossing) in crossings.iter().enumerate() {
        if let Some((row, latitude, longitude)) =
            horn_geometry::match_crossing(crossing, &rows, &types)
        {
            approaches.extend(horn_geometry::build_approaches(
                index, crossing, row, latitude, longitude, &rows,
            ));
        } else {
            unmatched += 1;
        }
    }
    approaches.sort_by_key(|approach| {
        (
            crossings[approach.crossing].synth_osm,
            approach.approach_idx,
        )
    });
    let pieces = horn_geometry::merge_overlaps(&approaches);
    let horns =
        horn_write::build_horn_batch(&base.schema(), base, crossings, &approaches, &pieces)?;
    eprintln!(
        "railways: {} crossings, {unmatched} unmatched, {} horn pieces",
        crossings.len(),
        pieces.len()
    );
    concat_batches(&base.schema(), &[base.clone(), horns]).map_err(|error| error.to_string())
}
