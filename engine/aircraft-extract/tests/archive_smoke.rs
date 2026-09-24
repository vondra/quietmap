//! Bounded real-archive smoke: read 16 trace entries, merge them with a copy of themselves, then run DEM segmentation.

use aircraft_extract::{source_adsb_tar::AdsbTarSource, stage_0::run_stage_0, stage_1::run_stage_1};
use std::fs::File;
use std::io::Read;

#[test]
#[ignore = "requires QM_ADSB_SMOKE_TAR and QM_PREPARED_DIR; reads a bounded archive prefix"]
fn real_archive_union_and_dem_segments() {
    let tar_path = std::env::var("QM_ADSB_SMOKE_TAR").expect("QM_ADSB_SMOKE_TAR");
    let prepared = std::env::var("QM_PREPARED_DIR").expect("QM_PREPARED_DIR");
    let temp = tempfile::tempdir().unwrap();
    let day = "2025-01-01";
    let day_dir = temp.path().join("source/2025").join(day);
    std::fs::create_dir_all(&day_dir).unwrap();
    let mut output = tar::Builder::new(File::create(day_dir.join("subset.tar")).unwrap());
    let mut input = tar::Archive::new(File::open(&tar_path).unwrap());
    let mut copied = 0;
    for entry in input.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        let name = path.to_string_lossy();
        if !name.contains("trace_full_") || !(name.ends_with(".json") || name.ends_with(".json.gz"))
        {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        let mut header = entry.header().clone();
        output
            .append_data(&mut header, path, bytes.as_slice())
            .unwrap();
        copied += 1;
        if copied == 16 {
            break;
        }
    }
    output.finish().unwrap();
    drop(output);
    assert_eq!(copied, 16);
    let source = AdsbTarSource::new(temp.path().join("source"));
    let flights_dir = temp.path().join("flights");
    let alone = run_stage_0(&source, None, day, &flights_dir, temp.path(), None).unwrap();
    let bytes = std::fs::read(flights_dir.join(format!("{day}.arrow"))).unwrap();
    let merged_dir = temp.path().join("merged");
    let merged = run_stage_0(&source, Some(&source), day, &merged_dir, temp.path(), None).unwrap();
    assert!(alone > 0);
    assert_eq!(merged, alone);
    assert_eq!(std::fs::read(merged_dir.join(format!("{day}.arrow"))).unwrap(), bytes);
    let rasters = raster_reader::RealRasters::new(std::path::Path::new(&prepared));
    let count = run_stage_1(&flights_dir, &temp.path().join("segments"), day, &rasters).unwrap();
    assert!(count > 0);
    eprintln!("real archive smoke: {copied} trace entries, {alone} flights, {count} DEM-classified segments");
}
