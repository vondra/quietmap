//! Stage 2C — ground operations.
//!
//! Writes `airport_traffic.arrow` per R4: sparse per-microsegment
//! per-period traffic counters with raw Σ over n_days of linear
//! Z-weighted band energy (v6 convention; consumer divides via
//! `period_leq(_, n_days_f, period_seconds)`). Every microsegment a
//! rotation's leg crossed receives both proportional band energy and
//! the rotation's `flight_id` (touch semantics). Consults OSM
//! `airport_areas.arrow` for nearest-aerodrome identity and reads
//! `airport_lines.arrow` per R4 for the aeroway microsegment graph.
//! See `airport_traffic_writer.rs`.

use std::path::Path;

use anyhow::Result;
use noise_compute::types::AirportArea;

use crate::progress::ts;
use crate::scope::ScopeBbox;

pub mod airport_summary_reduce;
pub mod airport_traffic;
pub mod airport_traffic_writer;

/// Filename of the global airport summary sidecar emitted by Stage 2C
/// reduce. Lives at `<aircraft_dir>/airport_summary.arrow` where
/// `aircraft_dir` is typically `data/prepared/{year}/aircraft/`.
pub const AIRPORT_SUMMARY_FILENAME: &str = "airport_summary.arrow";

/// Run Stage 2C against the per-R4 ground shards under
/// `segments_by_r4_dir/<R4>/ground.arrow`. `airport_areas` is the
/// global aerodrome identity set (aerodromes straddle R4 boundaries
/// so per-line resolution must see the whole set). When `scope` is
/// set, R4 cells outside its bbox+buffer are skipped — see
/// [`run_stage_2a`](crate::stage_2a::run_stage_2a) for rationale.
///
/// `aircraft_summary_dir` is where the global `airport_summary.arrow`
/// sidecar is written after the per-R4 pass. The popup
/// reads from a single canonical location instead of merging per-R4
/// duplicates. When `None`, defaults to `<h3r4_dir>/../aircraft/`.
pub fn run_stage_2c(
    segments_by_r4_dir: &Path,
    airport_areas: &[AirportArea],
    h3r4_dir: &Path,
    n_days: u16,
    // GA-class window (0 = single-window extract); see
    // [`airport_traffic_writer::run_airport_traffic`].
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    // Pre-check input schema BEFORE any destructive operation. The wipe
    // below removes stale airport_traffic.arrow files; if the writer
    // then fails on input schema mismatch (e.g. shuffle produced
    // pre-bump ground.arrow shards), the popup is left without a fresh
    // file and without the prior one. Catching the mismatch up front
    // keeps the existing output intact for the operator to recover.
    precheck_segments_schema(segments_by_r4_dir)?;

    // Wipe stale airport_traffic.arrow from in-scope R4s before workers
    // write fresh files. R4s that have no traffic this run would otherwise
    // retain a prior-run file (possibly older schema) and the popup
    // reader would fatal-fail on schema_version mismatch.
    let wiped = crate::wipe::wipe_stale_arrows_for_scope(h3r4_dir, "airport_traffic.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2c] wiped {wiped} stale airport_traffic.arrow file(s) before write",
            ts()
        );
    }
    let traffic_n = airport_traffic_writer::run_airport_traffic(
        segments_by_r4_dir,
        airport_areas,
        h3r4_dir,
        n_days,
        ga_n_days,
        scope,
    )?;

    // Reduce phase — walk per-R4 airport_summary_parts and union per
    // airport_key. Always run even when traffic_n == 0 so the popup
    // gets a (possibly empty) `airport_summary.arrow` to load (popup
    // §4.3 requires the file to exist when airport_traffic.arrow is
    // present anywhere in the grid disk).
    let parts_root = h3r4_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("h3r4_dir has no parent for airport_summary_parts"))?
        .join("airport_summary_parts");
    let summary_dir = h3r4_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("h3r4_dir has no parent for aircraft sidecar dir"))?
        .join("aircraft");
    std::fs::create_dir_all(&summary_dir)?;
    let summary_path = summary_dir.join(AIRPORT_SUMMARY_FILENAME);
    let _ = airport_summary_reduce::run_airport_summary_reduce(&parts_root, &summary_path)?;
    // Best-effort cleanup: parts/ is intermediate scratch.
    let _ = std::fs::remove_dir_all(&parts_root);
    Ok(traffic_n)
}

/// Open the first `<R4>/ground.arrow` shard under `segments_by_r4_dir`
/// to confirm the input is readable with the current `SCHEMA_VERSION`.
/// `read_record_batches` runs `assert_schema_version` internally; the
/// full `run_airport_traffic` reader would catch the same mismatch
/// later but only after wipe has already destroyed the previous
/// output. This precheck reads one file (microseconds), so a stale
/// shuffle or corrupt shard aborts the run before any destination
/// file is touched.
///
/// Returns `Ok(())` when the directory exists but holds no shards —
/// that's a valid empty-run case (writer emits zero rows). A missing
/// directory is treated as caller misconfiguration and raises; the
/// orchestrator always creates `segments_by_r4_dir` (even when empty)
/// at shuffle time.
fn precheck_segments_schema(segments_by_r4_dir: &Path) -> Result<()> {
    let entries = std::fs::read_dir(segments_by_r4_dir).map_err(|e| {
        anyhow::anyhow!(
            "Stage 2C precheck: cannot read {}: {e}. Pass the same \
             `segments_by_r4` path that `shuffle` wrote to.",
            segments_by_r4_dir.display()
        )
    })?;
    for entry in entries {
        let ground = entry?.path().join("ground.arrow");
        if !ground.is_file() {
            continue;
        }
        crate::arrow_io::read_record_batches(&ground).map_err(|e| {
            anyhow::anyhow!(
                "Stage 2C precheck failed on {}: {e}. The shard is either \
                 from an older `aircraft-extract` binary (stale schema) or \
                 corrupt — re-run shuffle with the current binary.",
                ground.display()
            )
        })?;
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::read_airport_summary;
    use crate::flight::{FlightSegment, Phase};

    /// End-to-end Stage 2C v5 pipeline: writer + reduce → loadable
    /// airport_summary.arrow. Uses a single arrival rotation crossing
    /// one runway microsegment; asserts the global UNION carries the
    /// arrival count = 1.
    #[test]
    fn run_stage_2c_produces_airport_summary_arrow() {
        use crate::airport_io::AERODROME_AEROWAY_TYPE;
        use crate::arrow_io::write_segments;
        use crate::geo::{lat_lon_to_cell, r4_hex_str};
        use crate::stage_2c::airport_traffic_writer::tests::write_real_airport_lines_arrow;
        use h3o::Resolution;
        let tmp = tempfile::tempdir().unwrap();
        let by_r4_dir = tmp.path().join("segments_by_r4");
        let h3r4_dir = tmp.path().join("h3r4");
        let lat = 50.0_f64;
        let lon0 = 14.0;
        let lon1 = 14.001;
        let mid_lat = lat;
        let mid_lon = (lon0 + lon1) * 0.5;
        let r4 = u64::from(lat_lon_to_cell(mid_lat, mid_lon, Resolution::Four).expect("valid r4"));
        let r4_h3r4_dir = h3r4_dir.join(r4_hex_str(r4));
        let r4_input_dir = by_r4_dir.join(r4_hex_str(r4));
        std::fs::create_dir_all(&r4_h3r4_dir).unwrap();
        std::fs::create_dir_all(&r4_input_dir).unwrap();
        write_real_airport_lines_arrow(
            &r4_h3r4_dir.join("airport_lines.arrow"),
            &[airport_traffic_writer::tests::FakeRealLine {
                osm_id: 42,
                segment_idx: 0,
                start_lat: lat,
                start_lon: lon0,
                end_lat: lat,
                end_lon: lon1,
                length_m: 100.0,
                aeroway_type: 0,
            }],
        );
        let leg = FlightSegment {
            flight_id: 0xDEAD_BEEF_u64,
            callsign: "TEST1".to_string(),
            aircraft_type: *b"B738",
            profile_idx: 23,
            source_id: 0,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 0,
            date_id: 0,
            phase: Phase::Ground,
            flags: 0, // is_departure=0 → arrival
            start_lat: lat as f32,
            start_lon: lon0 as f32,
            start_alt_m: 0.0,
            end_lat: lat as f32,
            end_lon: lon1 as f32,
            end_alt_m: 0.0,
            speed_kt: 90.0,
            length_m: 100.0,
            agl_avg_m: 0.0,
            start_elev_m: 0.0,
            end_elev_m: 0.0,
        };
        let aerodrome = AirportArea::new(
            1,
            AERODROME_AEROWAY_TYPE,
            "Test".to_string(),
            "LKTEST".to_string(),
            mid_lat,
            mid_lon,
            String::new(),
            100_000_000.0,
        );
        write_segments(&r4_input_dir.join("ground.arrow"), &[leg]).unwrap();

        let n = run_stage_2c(
            &by_r4_dir,
            std::slice::from_ref(&aerodrome),
            &h3r4_dir,
            1,
            0,
            None,
        )
        .unwrap();
        assert!(n > 0);

        // airport_summary.arrow must exist at
        // <tmp>/aircraft/airport_summary.arrow.
        let summary_path = tmp.path().join("aircraft").join(AIRPORT_SUMMARY_FILENAME);
        assert!(
            summary_path.exists(),
            "airport_summary.arrow must exist at {}",
            summary_path.display()
        );
        let rows = read_airport_summary(&summary_path).unwrap();
        // One airport, one arrival fid → UNION count = 1.
        let lktest = rows
            .iter()
            .find(|r| r.airport_key == "LKTEST")
            .expect("LKTEST row");
        assert_eq!(lktest.airport_unique_arr_count, 1);
        assert_eq!(lktest.airport_unique_dep_count, 0);
        // Runway ops_kind = index 0.
        assert_eq!(lktest.airport_unique_ops_count_per_kind[0], 1);

        // airport_summary_parts scratch dir must be cleaned up.
        let parts = tmp.path().join("airport_summary_parts");
        assert!(
            !parts.exists(),
            "airport_summary_parts/ must be cleaned up after run"
        );
    }

    /// Regression for the wipe-on-scope bug: a stale
    /// `airport_traffic.arrow` from a previous run that landed in an
    /// in-scope R4 with NO current ground traffic must be deleted
    /// before `run_stage_2c` returns. Mirrors the LKPR-14d reproducer
    /// that motivated the wipe.
    #[test]
    fn run_stage_2c_wipes_in_scope_stale_airport_traffic() {
        use crate::geo::r4_hex_str;
        use crate::scope::ScopeBbox;
        use h3o::{LatLng, Resolution};
        let tmp = tempfile::tempdir().unwrap();
        let by_r4_dir = tmp.path().join("segments_by_r4");
        let h3r4_dir = tmp.path().join("h3r4");
        // Praha R4 — in-scope. No ground.arrow input → writer does
        // not emit a fresh airport_traffic.arrow for this run.
        let r4 = u64::from(LatLng::new(50.10, 14.26).unwrap().to_cell(Resolution::Four));
        let r4_dir = h3r4_dir.join(r4_hex_str(r4));
        std::fs::create_dir_all(&r4_dir).unwrap();
        let stale = r4_dir.join("airport_traffic.arrow");
        std::fs::write(&stale, b"stale-prev-run").unwrap();
        std::fs::create_dir_all(&by_r4_dir).unwrap();
        // Praha scope.
        let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
        let n = run_stage_2c(&by_r4_dir, &[], &h3r4_dir, 1, 0, Some(&scope)).unwrap();
        assert_eq!(n, 0, "no ground segments → no R4 written");
        assert!(
            !stale.exists(),
            "stale airport_traffic.arrow must be wiped from in-scope R4"
        );
    }

    /// Regression for the wipe-before-error fragility: when
    /// `segments_by_r4_dir` carries a shard with a wrong-schema
    /// `ground.arrow`, the precheck must reject the run BEFORE the
    /// destructive wipe runs. Otherwise a stale shuffle leaves the
    /// popup with neither old nor new airport_traffic.arrow files.
    #[test]
    fn run_stage_2c_aborts_on_stale_input_before_wipe() {
        use crate::geo::r4_hex_str;
        use crate::scope::ScopeBbox;
        use h3o::{LatLng, Resolution};
        let tmp = tempfile::tempdir().unwrap();
        let by_r4_dir = tmp.path().join("segments_by_r4");
        let h3r4_dir = tmp.path().join("h3r4");
        let r4 = u64::from(LatLng::new(50.10, 14.26).unwrap().to_cell(Resolution::Four));
        // Pre-populate a fresh-looking airport_traffic.arrow in
        // h3r4_dir to confirm precheck keeps it intact on rejection.
        let h3r4_r4 = h3r4_dir.join(r4_hex_str(r4));
        std::fs::create_dir_all(&h3r4_r4).unwrap();
        let prior = h3r4_r4.join("airport_traffic.arrow");
        std::fs::write(&prior, b"prior-good-output").unwrap();
        // Corrupt ground.arrow shard (will fail the schema check).
        let by_r4_r4 = by_r4_dir.join(r4_hex_str(r4));
        std::fs::create_dir_all(&by_r4_r4).unwrap();
        std::fs::write(by_r4_r4.join("ground.arrow"), b"not-an-arrow-file").unwrap();
        let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
        let result = run_stage_2c(&by_r4_dir, &[], &h3r4_dir, 1, 0, Some(&scope));
        assert!(result.is_err(), "precheck must reject corrupt shard");
        assert!(
            prior.exists(),
            "precheck must abort before wipe — prior airport_traffic.arrow lost"
        );
    }

    /// Symmetric counterexample: a stale `airport_traffic.arrow`
    /// inside an OUT-OF-scope R4 must survive. Partial reextracts
    /// must not touch other regions' data.
    #[test]
    fn run_stage_2c_leaves_out_of_scope_stale_airport_traffic() {
        use crate::geo::r4_hex_str;
        use crate::scope::ScopeBbox;
        use h3o::{LatLng, Resolution};
        let tmp = tempfile::tempdir().unwrap();
        let by_r4_dir = tmp.path().join("segments_by_r4");
        let h3r4_dir = tmp.path().join("h3r4");
        // Gran Canaria R4 — out of Praha scope.
        let r4 = u64::from(
            LatLng::new(27.93, -15.39)
                .unwrap()
                .to_cell(Resolution::Four),
        );
        let r4_dir = h3r4_dir.join(r4_hex_str(r4));
        std::fs::create_dir_all(&r4_dir).unwrap();
        let stale = r4_dir.join("airport_traffic.arrow");
        std::fs::write(&stale, b"stale-prev-run").unwrap();
        std::fs::create_dir_all(&by_r4_dir).unwrap();
        let praha = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
        let _ = run_stage_2c(&by_r4_dir, &[], &h3r4_dir, 1, 0, Some(&praha)).unwrap();
        assert!(
            stale.exists(),
            "out-of-scope R4 file must survive a scoped reextract"
        );
    }
}
