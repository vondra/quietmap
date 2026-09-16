//! Admit aircraft source days before a world build using archive structure checks.

use crate::cli_validate::validated_days;
use aircraft_extract::source_adsb_tar::AdsbTarSource;
use anyhow::Result;
use std::path::Path;

/// Reuse Stage 0 day selection and TAR-tail admission without decoding the traces.
pub fn preflight_days(adsb_cache: &Path, days: &[String]) -> Result<()> {
    let days = validated_days(days.iter().cloned(), false)?;
    let source = AdsbTarSource::new(adsb_cache);
    for day in &days {
        source.require_archive_day(day)?;
    }
    eprintln!(
        "{} [preflight-days] admitted {} sampling day(s) under {}",
        aircraft_extract::progress::ts(),
        days.len(),
        adsb_cache.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_day(cache: &Path, day: &str, part: Option<&str>) {
        let dir = cache.join(day.split('-').next().unwrap()).join(day);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(part) = part {
            // A bare end-of-archive marker: structurally complete, zero traces.
            std::fs::write(dir.join(part), [0u8; 1024]).unwrap();
        }
    }

    #[test]
    fn missing_newest_monthly_sample_is_rejected_and_complete_windows_pass() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("adsbexchange");
        for day in ["2026-07-01", "2026-08-01"] {
            write_day(&cache, day, Some("subset.tar"));
        }
        preflight_days(&cache, &["2026-07-01".into(), "2026-08-01".into()]).unwrap();
        // The newest sample is absent: reject before any trace extraction.
        let error = preflight_days(&cache, &["2026-07-01".into(), "2026-09-01".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing ADS-B day 2026-09-01"), "{error}");
        // A day directory without any TAR part is an incomplete acquisition,
        // not an empty-flight day the producer would silently accept.
        write_day(&cache, "2026-09-01", None);
        let error = preflight_days(&cache, &["2026-09-01".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("no TAR archive in"), "{error}");
        // A truncated archive fails the structural admission before OSM runs.
        std::fs::write(
            cache.join("2026").join("2026-09-01").join("subset.tar"),
            [0u8; 512],
        )
        .unwrap();
        let error = preflight_days(&cache, &["2026-09-01".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("incomplete TAR byte length"), "{error}");
        // Split archives count as present parts.
        write_day(&cache, "2026-10-01", Some("subset.tar.aa"));
        preflight_days(&cache, &["2026-10-01".into()]).unwrap();
        // Malformed day ids never reach the filesystem.
        assert!(preflight_days(&cache, &["2026-13-01".into()]).is_err());
        assert!(preflight_days(&cache, &[]).is_err());
    }
}
