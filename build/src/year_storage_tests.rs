//! Admitted data cannot cross years through a reused root, alias or byte-for-byte copy.
use super::*;
use serde_json::json;
use std::path::PathBuf;

fn temporary_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qm-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_year(root: &Path, year: &str, observed: &str, shared: bool) -> YearInputs {
    let value = json!({"year":year,"as_of":"2003-01-01","built":false,
        "flows":{"start":"2000-01-01","end":"2001-01-01"},"inputs":[{
        "id":"counts","kind":"prepare-road-counts","region":"world","publisher":"Test",
        "url":"https://example.org/counts","observed":{"start":observed,"end":observed},
        "published":null,"public_ok":true,"licence":"CC0","description":"Counted vehicles.","depends_on":[]}],
        "unavailable":[],"shared":if shared {vec!["counts"]} else {vec![]}});
    let manifest = root.join(format!("{year}.json"));
    std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    YearInputs::read(&manifest, root).unwrap()
}

fn assert_distinct_inputs(left: &YearInputs, right: &YearInputs) -> Result<(), String> {
    for input in &left.manifest.inputs {
        let Some(other) = right
            .manifest
            .inputs
            .iter()
            .find(|other| other.id == input.id)
        else {
            continue;
        };
        if input.observed == other.observed
            || (left.manifest.shared.contains(&input.id)
                && right.manifest.shared.contains(&input.id))
        {
            continue;
        }
        let left_path = left
            .paths(input.kind)?
            .iter()
            .find(|path| path.file_name().unwrap() == input.id.as_str())
            .unwrap();
        let right_path = right
            .paths(other.kind)?
            .iter()
            .find(|path| path.file_name().unwrap() == input.id.as_str())
            .unwrap();
        let same_path = left_path.canonicalize().unwrap() == right_path.canonicalize().unwrap();
        let same_bytes = tiles::manifest_hash(&std::fs::read(left_path).unwrap())
            == tiles::manifest_hash(&std::fs::read(right_path).unwrap());
        if same_path || same_bytes {
            return Err(format!(
                "{}: different observations resolve to identical paths or bytes without shared declaration",
                input.id
            ));
        }
    }
    Ok(())
}

#[test]
fn different_observations_cannot_share_paths_or_bytes_without_a_declaration() {
    let root = temporary_root("independent-years");
    for year in ["2000", "2001"] {
        std::fs::create_dir_all(root.join(year)).unwrap();
    }
    std::fs::write(root.join("2000/counts"), b"earlier counts").unwrap();
    std::fs::write(root.join("2001/counts"), b"later counts").unwrap();
    let a = write_year(&root, "2000", "1999", false);
    let b = write_year(&root, "2001", "2000", false);
    assert_ne!(
        a.required(Kind::PrepareRoadCounts).unwrap(),
        b.required(Kind::PrepareRoadCounts).unwrap()
    );
    assert_distinct_inputs(&a, &b).unwrap();
    assert!(a.national_dem_paths().unwrap().is_empty());
    std::fs::copy(root.join("2000/counts"), root.join("2001/counts")).unwrap();
    assert!(assert_distinct_inputs(&a, &b).is_err());
    std::fs::remove_file(root.join("2001/counts")).unwrap();
    std::os::unix::fs::symlink(root.join("2000/counts"), root.join("2001/counts")).unwrap();
    assert!(assert_distinct_inputs(&a, &b).is_err());
    let a = write_year(&root, "2000", "1999", true);
    let b = write_year(&root, "2001", "2000", true);
    assert_distinct_inputs(&a, &b).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn every_required_model_kind_refuses_missing_and_empty_inputs() {
    let root = temporary_root("required-models");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
    std::fs::create_dir_all(root.join("2000")).unwrap();
    let manifest_path = root.join("year.json");
    std::fs::copy(fixture.join("model-year.json"), &manifest_path).unwrap();
    let manifest = Manifest::read(&manifest_path).unwrap();
    for input in &manifest.inputs {
        std::fs::copy(
            fixture.join("2000").join(&input.id),
            root.join("2000").join(&input.id),
        )
        .unwrap();
    }
    for input in &manifest.inputs {
        let mut missing: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.join("model-year.json")).unwrap())
                .unwrap();
        missing["inputs"]
            .as_array_mut()
            .unwrap()
            .retain(|row| row["id"] != input.id);
        std::fs::write(&manifest_path, serde_json::to_vec(&missing).unwrap()).unwrap();
        let inputs = YearInputs::read(&manifest_path, &root).unwrap();
        let error = match crate::source_tables::SourceTables::read(&inputs) {
            Ok(_) => panic!("missing kind {} silently defaulted", input.kind),
            Err(error) => error,
        };
        assert!(error.contains(&input.kind.to_string()), "{error}");
        std::fs::copy(fixture.join("model-year.json"), &manifest_path).unwrap();
        let path = root.join("2000").join(&input.id);
        let bytes = std::fs::read(&path).unwrap();
        for empty in [false, true] {
            if empty {
                std::fs::write(&path, br#"{"rows":[]}"#).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let inputs = YearInputs::read(&manifest_path, &root).unwrap();
            let error = match crate::source_tables::SourceTables::read(&inputs) {
                Ok(_) => panic!("{} silently defaulted", input.kind),
                Err(e) => e,
            };
            assert!(error.contains(&input.kind.to_string()), "{error}");
            std::fs::write(&path, &bytes).unwrap();
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// A table the year run fits is admitted through the inputs it is fitted from, not derived apart.
#[test]
fn a_historical_model_not_yet_derived_is_refused_with_its_pending_derivation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let inputs =
        YearInputs::read(&root.join("../years/2024.json"), &root.join("test-data")).unwrap();
    let produced = crate::pipeline_plan::produced_kinds();
    for &kind in crate::source_tables::MODEL_KINDS
        .iter()
        .filter(|kind| !produced.contains(kind))
    {
        if let Err(error) = inputs.paths(kind)
            && error.contains("no admitted input")
        {
            assert!(error.contains("To be derived for 2024"), "{error}");
        }
    }
}

#[test]
fn subdivision_zero_is_rejected_and_reference_provenance_comes_from_the_row() {
    let root = temporary_root("subdivision-rows");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
    std::fs::create_dir_all(root.join("2000")).unwrap();
    let manifest = fixture.join("model-year.json");
    for kind in [
        Kind::SubdivisionGrids,
        Kind::RoadReferenceCounts,
        Kind::RegionalVehicleShares,
    ] {
        std::fs::copy(
            fixture.join("2000").join(kind.to_string()),
            root.join("2000").join(kind.to_string()),
        )
        .unwrap();
    }
    let inputs = YearInputs::read(&manifest, &root).unwrap();
    let count_path = root.join("2000/road-reference-counts");
    let mut counts: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&count_path).unwrap()).unwrap();
    counts["rows"][1]["source_id"] = json!(42);
    std::fs::write(&count_path, serde_json::to_vec(&counts).unwrap()).unwrap();
    let roads = crate::source_tables::RegionalRoads::read(&inputs).unwrap();
    assert_eq!(
        roads
            .reference_count(*b"TH", "4169", 9.47, 99.95)
            .unwrap()
            .4,
        42
    );
    for kind in [Kind::RoadReferenceCounts, Kind::RegionalVehicleShares] {
        let path = root.join("2000").join(kind.to_string());
        let original = std::fs::read(&path).unwrap();
        let mut rows: serde_json::Value = serde_json::from_slice(&original).unwrap();
        rows["rows"][0]["subdivision"] = json!(0);
        std::fs::write(&path, serde_json::to_vec(&rows).unwrap()).unwrap();
        assert!(crate::source_tables::RegionalRoads::read(&inputs).is_err());
        std::fs::write(path, original).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completed_inputs_must_match_the_published_model_values() {
    let root = temporary_root("published-tables");
    std::fs::create_dir_all(root.join("2000")).unwrap();
    std::fs::create_dir_all(root.join("tables/2000")).unwrap();
    let inputs = write_year(&root, "2000", "1999", false);
    let actual = root.join("2000/counts");
    let published = root.join("tables/2000/prepare-road-counts.json");
    std::fs::write(&actual, b"original").unwrap();
    std::fs::write(&published, b"original").unwrap();
    inputs
        .check_published_table(Kind::PrepareRoadCounts, &published)
        .unwrap();
    std::fs::write(&actual, b"different").unwrap();
    assert!(
        inputs
            .check_published_table(Kind::PrepareRoadCounts, &published)
            .unwrap_err()
            .contains("prepare-road-counts")
    );
    std::fs::remove_dir_all(root).unwrap();
}
