//! Bounded real-archive smoke: read 16 trace entries, then run both class passes and DEM segmentation.

use aircraft_extract::{
    source::FlightSource,
    source_adsb_tar::{AdsbTarSource, ClassWindowFilter},
    stage_0::write_flights_at,
    stage_1::run_stage_1,
};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;

#[test]
#[ignore = "requires QM_ADSB_SMOKE_TAR and QM_PREPARED_DIR; reads a bounded archive prefix"]
fn real_archive_both_windows_and_dem_segments() {
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
    let source = temp.path().join("source");
    let all = AdsbTarSource::new(&source).read_day(day).unwrap();
    let ga = AdsbTarSource::new(&source)
        .with_class_filter(ClassWindowFilter::GaOnly)
        .read_day(day)
        .unwrap();
    let non_ga = AdsbTarSource::new(&source)
        .with_class_filter(ClassWindowFilter::NonGa)
        .read_day(day)
        .unwrap();
    assert!(!all.is_empty());
    let all_ids: BTreeSet<_> = all.iter().map(|f| f.flight_id).collect();
    let ga_ids: BTreeSet<_> = ga.iter().map(|f| f.flight_id).collect();
    let airline_ids: BTreeSet<_> = non_ga.iter().map(|f| f.flight_id).collect();
    assert!(ga_ids.is_disjoint(&airline_ids));
    assert_eq!(
        ga_ids.union(&airline_ids).copied().collect::<BTreeSet<_>>(),
        all_ids
    );
    let flights_dir = temp.path().join("flights");
    write_flights_at(&flights_dir.join(format!("{day}.arrow")), &all).unwrap();
    let rasters = raster_reader::RealRasters::new(std::path::Path::new(&prepared));
    let count = run_stage_1(&flights_dir, &temp.path().join("segments"), day, &rasters).unwrap();
    assert!(count > 0);
    eprintln!("real archive smoke: {copied} trace entries, {} flights = {} GA + {} non-GA, {count} DEM-classified segments", all.len(), ga.len(), non_ga.len());
}
