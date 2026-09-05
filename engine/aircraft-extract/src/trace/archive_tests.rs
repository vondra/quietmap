//! Alternative TAR exports select whole aircraft-day traces before native flight identity and Arrow output.

use super::tests::{day_dir_with_tar, gz};
use super::*;
use crate::source_adsb_tar::AdsbTarSource;

fn trace(icao: &str, callsign: &str, latitudes: &[f64]) -> Vec<u8> {
    let points: Vec<_> = latitudes
        .iter()
        .enumerate()
        .map(|(i, lat)| {
            serde_json::json!([
                i * 10, lat, 14.0, 1000.0, 100.0, 90.0, 0, 0,
                {"flight": callsign}
            ])
        })
        .collect();
    gz(&serde_json::json!({
        "icao": icao, "t": "C172", "timestamp": 1711065600.0, "trace": points
    })
    .to_string())
}

#[test]
fn alternative_exports_preserve_one_whole_trace_and_fail_closed_before_day_write() {
    let complete = day_dir_with_tar(&[
        (
            "trace_full_abc123.json",
            &trace("abc123", "INVALID_COUNT", &[50.0, 91.0, 50.1, 92.0]),
        ),
        (
            "trace_full_def456.json",
            &trace("def456", "LATER_EXPORT", &[51.0, 51.1]),
        ),
        (
            "trace_full_a04608.json",
            &trace("a04608", "UNIQUE_C172", &[52.0, 52.1]),
        ),
        (
            "trace_full_bbb222.json",
            &trace("bbb222", "WHOLE_COMPLETE", &[58.0, 58.1, 58.2]),
        ),
        (
            "trace_full_~aabbcc.json",
            &trace("~aabbcc", "ANON_LATER", &[53.0, 53.1]),
        ),
        (
            "trace_full_unknown_a.json",
            &trace("", "UNKNOWN_A", &[54.0, 54.1]),
        ),
        (
            "trace_full_ffffff_a.json",
            &trace("ffffff", "RESERVED_A", &[56.0, 56.1]),
        ),
    ]);
    let split = day_dir_with_tar(&[
        (
            "trace_full_abc123.json",
            &trace("abc123", "WHOLE_SPLIT", &[50.2, 50.3, 50.4]),
        ),
        (
            "trace_full_def456.json",
            &trace("def456", "SPLIT_TIE", &[51.2, 51.3]),
        ),
        (
            "trace_full_bbb222.json",
            &trace("bbb222", "SHORT_SPLIT", &[58.3, 58.4]),
        ),
        (
            "trace_full_~aabbcc.json",
            &trace("~aabbcc", "ANON_SPLIT", &[53.2, 53.3]),
        ),
        (
            "trace_full_unknown_b.json",
            &trace("", "UNKNOWN_B", &[55.0, 55.1]),
        ),
        (
            "trace_full_ffffff_b.json",
            &trace("ffffff", "RESERVED_B", &[57.0, 57.1]),
        ),
    ]);
    let complete_bytes = std::fs::read(complete.path().join("subset.tar")).unwrap();
    let split_bytes = std::fs::read(split.path().join("subset.tar")).unwrap();
    for reverse_creation in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("2024-03-22");
        std::fs::create_dir(&day).unwrap();
        let cut = split_bytes.len() / 2;
        let mut files = [
            ("export.tar", complete_bytes.as_slice()),
            ("export.tar.aa", &split_bytes[..cut]),
            ("export.tar.ab", &split_bytes[cut..]),
        ];
        if reverse_creation {
            files.reverse();
        }
        for (name, bytes) in files {
            std::fs::write(day.join(name), bytes).unwrap();
        }

        let selected = read_day_traces(&day).unwrap();
        assert_eq!(
            selected.len(),
            9,
            "unknown identities must not collapse together"
        );
        let shared = selected.iter().find(|tr| tr.icao24 == "abc123").unwrap();
        assert_eq!(
            shared.points.iter().map(|p| p.lat).collect::<Vec<_>>(),
            [50.2, 50.3, 50.4]
        );
        assert_eq!(shared.callsigns[0].value, "WHOLE_SPLIT");
        assert_eq!(
            selected
                .iter()
                .find(|tr| tr.icao24 == "def456")
                .unwrap()
                .callsigns[0]
                .value,
            "SPLIT_TIE"
        );
        assert_eq!(
            selected
                .iter()
                .find(|tr| tr.icao24 == "~aabbcc")
                .unwrap()
                .callsigns[0]
                .value,
            "ANON_SPLIT"
        );

        let output = root.path().join("flights");
        let sources: Vec<Box<dyn crate::source::FlightSource>> = vec![Box::new(
            AdsbTarSource::new(root.path())
                .with_class_filter(crate::source_adsb_tar::ClassWindowFilter::GaOnly),
        )];
        let count = crate::stage_0::run_stage_0(&sources, "2024-03-22", &output).unwrap();
        let flights = crate::stage_1::read_flights(&output.join("2024-03-22.arrow")).unwrap();
        assert_eq!(count, 9);
        let mut callsigns: Vec<_> = flights.iter().map(|f| f.callsign.as_str()).collect();
        callsigns.sort_unstable();
        assert_eq!(
            callsigns,
            [
                "ANON_SPLIT",
                "RESERVED_A",
                "RESERVED_B",
                "SPLIT_TIE",
                "UNIQUE_C172",
                "UNKNOWN_A",
                "UNKNOWN_B",
                "WHOLE_COMPLETE",
                "WHOLE_SPLIT"
            ]
        );

        std::fs::remove_file(day.join("export.tar.ab")).unwrap();
        let failed_output = root.path().join("failed");
        assert!(crate::stage_0::run_stage_0(&sources, "2024-03-22", &failed_output).is_err());
        assert!(!failed_output.join("2024-03-22.arrow").exists());
    }
}
