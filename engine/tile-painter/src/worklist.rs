//! World-scale work-list: enumerate populated source R4s from the
//! prepared `h3r4/` tree, dilate them to the output-region set, and
//! resolve the single build-wide `n_days` from arrow metadata.
//!
//! Two R4 sets, deliberately distinct:
//! - `source_r4s` — R4s that own aircraft arrows; the per-region source
//!   load keys.
//! - `output_r4s` — `union(grid_disk(1))` over the sources; the regions
//!   whose tiles we actually build. The dilation captures receiver tiles
//!   in an otherwise-empty neighbour R4 that still hears an edge source
//!   (≤16 km spill) — building only populated R4s would silently drop
//!   them. Each output region later loads its OWN grid_disk(1) sources.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;

use anyhow::{bail, Context, Result};
use arrow::ipc::reader::FileReader;
use h3o::{CellIndex, Resolution};

use crate::r4_source_cache::SourceSel;

/// Arrow files whose presence marks an R4 as a source, in `SourceSel`
/// field order (cruise, airborne, traffic). cruise covers the most R4s;
/// airborne fewer; airport_traffic only near airports.
const SOURCE_ARROWS: [&str; 3] = ["cruise.arrow", "airborne.arrow", "airport_traffic.arrow"];

/// The subset of [`SOURCE_ARROWS`] the build's `--source` selection wants
/// — so a single-layer build neither scans nor n_days-checks arrows it
/// will never load (e.g. `--source cruise` ignores a stale airborne).
fn selected_arrows(sel: SourceSel) -> Vec<&'static str> {
    SOURCE_ARROWS
        .iter()
        .zip([sel.cruise, sel.airborne, sel.traffic])
        .filter_map(|(&f, on)| on.then_some(f))
        .collect()
}

pub struct WorkList {
    /// R4 hexes that own source arrows — the per-region load keys.
    pub source_r4s: Vec<u64>,
    /// R4 hexes whose tiles we build = `union(grid_disk(1))` over sources.
    pub output_r4s: Vec<u64>,
}

impl WorkList {
    /// Walk `h3r4_dir` once, keeping subdirs that name a valid
    /// `Resolution::Four` cell (rejects scratch / all-zero dirs, mirroring
    /// `aircraft-extract`'s `wipe`) and hold ≥1 arrow of the selected
    /// `--source` layers, then dilate to the output-region set. IO errors
    /// fail loud rather than silently dropping a region (a dropped source
    /// R4 = missing output tiles).
    pub fn scan(h3r4_dir: &Path, sel: SourceSel) -> Result<Self> {
        let mut source: BTreeSet<u64> = BTreeSet::new();
        for entry in std::fs::read_dir(h3r4_dir)
            .with_context(|| format!("read_dir {}", h3r4_dir.display()))?
        {
            let entry = entry.with_context(|| format!("dir entry in {}", h3r4_dir.display()))?;
            let path = entry.path();
            let ft = entry
                .file_type()
                .with_context(|| format!("file_type {}", path.display()))?;
            if !ft.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(r4) = u64::from_str_radix(name, 16) else {
                continue;
            };
            let Ok(cell) = CellIndex::try_from(r4) else {
                continue;
            };
            if cell.resolution() != Resolution::Four {
                continue;
            }
            // On-disk R4 dirs are canonical `r4_hex_str` = `{:015x}`
            // (extract `geo.rs`); `resolve_n_days` + the source loaders
            // rebuild paths the same way, so a non-canonical name
            // (uppercase / padded) would round-trip to a missing path and
            // be silently skipped downstream. Drop it here instead.
            if name != format!("{r4:015x}") {
                continue;
            }
            if has_source_arrow(&path, sel)? {
                source.insert(r4);
            }
        }
        if source.is_empty() {
            bail!("no populated source R4s under {}", h3r4_dir.display());
        }
        let mut output: BTreeSet<u64> = BTreeSet::new();
        for &r4 in &source {
            let cell = CellIndex::try_from(r4).expect("validated on insert");
            for n in cell.grid_disk::<Vec<_>>(1) {
                output.insert(u64::from(n));
            }
        }
        Ok(Self {
            source_r4s: source.into_iter().collect(),
            output_r4s: output.into_iter().collect(),
        })
    }
}

fn has_source_arrow(dir: &Path, sel: SourceSel) -> Result<bool> {
    for f in selected_arrows(sel) {
        let path = dir.join(f);
        if path
            .try_exists()
            .with_context(|| format!("stat {}", path.display()))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Does ANY of `source_r4s` carry a `sel`-selected arrow? A cluster chunk can hold one
/// source (e.g. road) but not another (airborne) — building the absent one must be a no-op,
/// NOT the fatal "no source arrows" that `resolve_n_days` raises, else a job that bundles two
/// sources (the GPU `line` job = road/rail + airborne) loses BOTH when one is empty.
/// Callers check this first and exit 0 when false.
pub fn any_source_arrow(h3r4_dir: &Path, source_r4s: &[u64], sel: SourceSel) -> Result<bool> {
    for &r4 in source_r4s {
        if has_source_arrow(&h3r4_dir.join(format!("{r4:015x}")), sel)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolve the single build-wide `n_days` by reading the `n_days`
/// metadata stamped on every source R4's arrows and asserting they all
/// agree. The extract writes one window into every stage output, so a
/// disagreement means mixed/stale shards (e.g. a 14-day R4 beside
/// full-year ones) that would seam ~10·log10(ratio) dB across the map —
/// bail with the offending values rather than divide by a wrong window.
/// Every present source arrow MUST carry a valid `n_days` (missing,
/// unparseable, or 0 is fatal): a metadata-less shard sitting beside
/// valid ones is exactly the silent mismatch this guard exists to catch.
pub fn resolve_n_days(h3r4_dir: &Path, source_r4s: &[u64], sel: SourceSel) -> Result<u16> {
    let mut seen: BTreeMap<u16, u64> = BTreeMap::new();
    for &r4 in source_r4s {
        let dir = h3r4_dir.join(format!("{r4:015x}"));
        for f in selected_arrows(sel) {
            let path = dir.join(f);
            if !path
                .try_exists()
                .with_context(|| format!("stat {}", path.display()))?
            {
                continue;
            }
            seen.entry(read_n_days(&path)?).or_insert(r4);
        }
    }
    match seen.len() {
        0 => bail!(
            "no source arrows found for {} source R4(s)",
            source_r4s.len()
        ),
        1 => Ok(*seen.keys().next().expect("len checked == 1")),
        _ => {
            let detail = seen
                .iter()
                .map(|(nd, r4)| format!("{nd} (e.g. {r4:015x})"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "inconsistent n_days across source arrows: {detail} — \
                 re-extract / re-stamp before building"
            );
        }
    }
}

/// Resolve the build-wide GA full-year hybrid weight LUT from the
/// `sample_days_by_class` metadata stamped on the source arrows. Reads the
/// vector off every present
/// source arrow, asserts they all agree (a disagreement = mixed/stale
/// shards that would seam GA weighting across the map), and parses it
/// against `n_days`. FAILS LOUD when any GA-stamped source arrow lacks
/// the stamp — the arrows predate the hybrid contract and must be
/// re-extracted (owner directive 2026-06-12; no uniform fallback). The
/// schema_check contract gate (airborne v4 / airport_traffic v9) catches
/// this first at load time; this is the build-orchestration safety net.
pub fn resolve_class_weights(
    h3r4_dir: &Path,
    source_r4s: &[u64],
    sel: SourceSel,
    n_days: u16,
) -> Result<noise_compute::emission::aircraft::ClassWeights> {
    use noise_compute::emission::aircraft::ClassWeights;
    // cruise.arrow carries no sample_days vector (airline-only window),
    // so skip it — only airborne + airport_traffic stamp the GA vector.
    let ga_selected: [&str; 2] = ["airborne.arrow", "airport_traffic.arrow"];
    let want: Vec<&str> = selected_arrows(sel)
        .into_iter()
        .filter(|f| ga_selected.contains(f))
        .collect();
    let mut seen: BTreeMap<String, u64> = BTreeMap::new();
    for &r4 in source_r4s {
        let dir = h3r4_dir.join(format!("{r4:015x}"));
        for f in &want {
            let path = dir.join(f);
            if !path
                .try_exists()
                .with_context(|| format!("stat {}", path.display()))?
            {
                continue;
            }
            seen.entry(read_sample_days_vector(&path)?).or_insert(r4);
        }
    }
    match seen.len() {
        // No GA-stamped source arrow present (e.g. a cruise-only build):
        // nothing to weight, the uniform LUT is correct.
        0 => Ok(ClassWeights::uniform()),
        1 => ClassWeights::parse(Some(seen.keys().next().expect("len 1")), n_days)
            .map_err(|e| anyhow::anyhow!(e)),
        _ => {
            let detail = seen
                .iter()
                .map(|(v, r4)| format!("{v:?} (e.g. {r4:015x})"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "inconsistent sample_days_by_class across source arrows: {detail} — \
                 re-extract / re-merge before building"
            )
        }
    }
}

/// Read the `sample_days_by_class` schema metadata string from one arrow.
fn read_sample_days_vector(path: &Path) -> Result<String> {
    use noise_compute::emission::aircraft::SAMPLE_DAYS_BY_CLASS_KEY;
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader =
        FileReader::try_new(file, None).with_context(|| format!("arrow ipc {}", path.display()))?;
    reader
        .schema()
        .metadata()
        .get(SAMPLE_DAYS_BY_CLASS_KEY)
        .cloned()
        .with_context(|| {
            format!(
                "{} missing {SAMPLE_DAYS_BY_CLASS_KEY} metadata — re-extract aircraft pipeline \
                 (arrows predate the GA 365-day hybrid contract)",
                path.display()
            )
        })
}

/// Read the `n_days` schema metadata from one arrow. `File` is passed
/// straight to the IPC reader (footer seek, no row decode, no mmap).
fn read_n_days(path: &Path) -> Result<u16> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader =
        FileReader::try_new(file, None).with_context(|| format!("arrow ipc {}", path.display()))?;
    let raw = reader
        .schema()
        .metadata()
        .get("n_days")
        .with_context(|| format!("{} missing n_days metadata", path.display()))?
        .clone();
    let n: u16 = raw
        .parse()
        .with_context(|| format!("{} has unparseable n_days {raw:?}", path.display()))?;
    if n == 0 {
        bail!("{} has n_days=0 (invalid divisor)", path.display());
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use arrow::array::UInt32Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::ipc::writer::FileWriter;
    use arrow::record_batch::RecordBatch;

    // Real R4 cells (LKPR + Dobříš, per CLAUDE.md reference table).
    const LKPR: &str = "841e355ffffffff";
    const DOBRIS: &str = "841e309ffffffff";

    /// Write a minimal one-row arrow; `n_days = None` omits the metadata
    /// key entirely (to exercise the strict missing-metadata guard).
    fn write_arrow(path: &Path, n_days: Option<&str>) {
        let md = match n_days {
            Some(v) => HashMap::from([("n_days".to_string(), v.to_string())]),
            None => HashMap::new(),
        };
        let schema = Arc::new(Schema::new_with_metadata(
            vec![Field::new("x", DataType::UInt32, false)],
            md,
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(UInt32Array::from(vec![1u32]))],
        )
        .unwrap();
        let mut w = FileWriter::try_new(File::create(path).unwrap(), &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn scan_keeps_sources_rejects_scratch_and_dilates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lkpr = u64::from_str_radix(LKPR, 16).unwrap();
        let dobris = u64::from_str_radix(DOBRIS, 16).unwrap();

        // Valid R4 with a source arrow → a source.
        let d = root.join(LKPR);
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("airborne.arrow"), b"x").unwrap();
        // Valid R4 dir but no source arrow → not a source.
        std::fs::create_dir(root.join(DOBRIS)).unwrap();
        // Hex-named scratch dir that is not an R4 cell → ignored.
        std::fs::create_dir(root.join("deadbeef")).unwrap();

        let wl = WorkList::scan(root, SourceSel::ALL).unwrap();
        assert_eq!(wl.source_r4s, vec![lkpr]);
        assert!(!wl.source_r4s.contains(&dobris));
        // output = the 1-ring around the single source.
        let ring = CellIndex::try_from(lkpr)
            .unwrap()
            .grid_disk::<Vec<_>>(1)
            .len();
        assert_eq!(wl.output_r4s.len(), ring);
        assert!(wl.output_r4s.contains(&lkpr));
    }

    #[test]
    fn scan_respects_source_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lkpr = u64::from_str_radix(LKPR, 16).unwrap();
        let d = root.join(LKPR);
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("airborne.arrow"), b"x").unwrap(); // only airborne

        // --source cruise: an airborne-only R4 is not a cruise source → bail.
        let cruise = SourceSel {
            cruise: true,
            airborne: false,
            traffic: false,
        };
        assert!(WorkList::scan(root, cruise).is_err());

        // --source airborne: it IS a source.
        let air = SourceSel {
            cruise: false,
            airborne: true,
            traffic: false,
        };
        assert_eq!(WorkList::scan(root, air).unwrap().source_r4s, vec![lkpr]);
    }

    #[test]
    fn resolve_n_days_agrees_then_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = u64::from_str_radix(LKPR, 16).unwrap();
        let b = u64::from_str_radix(DOBRIS, 16).unwrap();
        for r4 in [a, b] {
            let d = root.join(format!("{r4:015x}"));
            std::fs::create_dir(&d).unwrap();
            write_arrow(&d.join("cruise.arrow"), Some("365"));
        }
        assert_eq!(resolve_n_days(root, &[a, b], SourceSel::ALL).unwrap(), 365);

        // A second arrow on one R4 disagreeing must bail.
        write_arrow(
            &root.join(format!("{b:015x}")).join("airborne.arrow"),
            Some("14"),
        );
        assert!(resolve_n_days(root, &[a, b], SourceSel::ALL).is_err());
    }

    #[test]
    fn resolve_n_days_rejects_missing_and_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = u64::from_str_radix(LKPR, 16).unwrap();
        let d = root.join(format!("{a:015x}"));
        std::fs::create_dir(&d).unwrap();

        // Present arrow with no n_days key → fatal, not silently skipped.
        write_arrow(&d.join("cruise.arrow"), None);
        assert!(resolve_n_days(root, &[a], SourceSel::ALL).is_err());

        // n_days=0 is an invalid divisor → fatal.
        write_arrow(&d.join("cruise.arrow"), Some("0"));
        assert!(resolve_n_days(root, &[a], SourceSel::ALL).is_err());
    }
}
