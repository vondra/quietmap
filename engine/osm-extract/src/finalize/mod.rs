//! Finalize: read spill buckets → group by hex_id → write Arrow IPC per hex directory,
//! into the PREPARED tree for every painter layer and into the OSM extract SOURCE tree
//! for `buildings`/`barriers` (see `cell_dir_for_source`).

use anyhow::Result;
use arrow::array::ArrayRef;
use arrow::datatypes::{Field, Schema};
use arrow::ipc::writer::FileWriter;
use h3o::{CellIndex, Resolution};
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::poi_join::{JoinStats, PoiIndex};

mod write_airport;
mod write_barriers;
mod write_buildings;
mod write_industrial;
mod write_leisure;
mod write_railways;
mod write_roads;

use write_airport::{write_airport_areas, write_airport_lines};
use write_barriers::write_barriers;
use write_buildings::write_buildings;
use write_industrial::write_industrial;
use write_leisure::write_leisure;
use write_railways::write_railways;
use write_roads::write_roads;

/// Per-file schema contract (settlement v2 phase 2). Bumped on `buildings.arrow`
/// because the v2 extract adds an `opening_hours` column and re-numbers
/// `building_type` (new SILENT/HOUSE/FOOD_RETAIL/HOSPITALITY classes); a v1
/// reader would mis-profile every building. `leisure.arrow` ships at its own v1.
/// Stamped into the arrow metadata; consumers fail loud on a mismatch
/// (Convention-B per-file contract, no global SCHEMA_VERSION bump).
pub const BUILDINGS_CONTRACT_V2: &str = "buildings_v2";
pub const LEISURE_CONTRACT_V1: &str = "leisure_v1";

/// Read all spill buckets and write final per-hex Arrow IPC files.
/// Returns number of distinct hexes written (a hex counts once even when its
/// painter layers and its OSM tables land in two different trees).
///
/// Each `(source, bucket)` pair is an independent unit of work. `spill.rs` assigns
/// every feature to `bucket = (hex_id >> 28) % num_buckets`, a pure function of
/// `hex_id`, so a given hex lands in exactly one bucket — two units never write the
/// same `{source}.arrow` path. The units therefore run across the rayon pool instead
/// of one core (the per-row Arrow encode was the single-thread bottleneck). The only
/// shared filesystem touch is `create_dir_all` when two sources hit the same hex dir,
/// which tolerates the concurrent-create race.
pub fn finalize(
    spill_dir: &Path,
    prepared_dir: &Path,
    osm_extract_dir: &Path,
    num_buckets: usize,
) -> Result<usize> {
    // `poi` is intentionally NOT a final source — it is the footprint-join
    // input consumed when finalizing `buildings` (settlement v2 phase 2).
    const SOURCES: [&str; 8] = [
        "roads",
        "railways",
        "airport_areas",
        "airport_lines",
        "buildings",
        "industrial",
        "barriers",
        "leisure",
    ];

    let units: Vec<(&str, usize)> = SOURCES
        .iter()
        .flat_map(|s| (0..num_buckets).map(move |b| (*s, b)))
        .filter(|(s, b)| spill_dir.join(format!("{}_{:03}.tsv", s, b)).exists())
        .collect();
    eprintln!(
        "  Finalizing {} spill buckets across rayon pool...",
        units.len()
    );

    // Shared across the parallel building buckets (settlement v2 phase 2).
    let join_stats = JoinStats::default();
    let unit_sets = units
        .par_iter()
        .map(|(source, bucket)| {
            let hexes = finalize_bucket(
                source,
                *bucket,
                spill_dir,
                prepared_dir,
                osm_extract_dir,
                &join_stats,
            )?;
            Ok((*source, hexes))
        })
        .collect::<Result<Vec<(&str, HashSet<String>)>>>()?;

    let (checked, reclassified) = join_stats.report();
    if checked > 0 {
        eprintln!(
            "  POI footprint join: {reclassified}/{checked} `building=yes` reclassified \
             ({:.1}%)",
            reclassified as f64 / checked as f64 * 100.0
        );
    }

    // Distinct hexes across all sources — a hex with both roads and buildings appears
    // in two bucket sets, so union before counting (matches the old shared-set count).
    // Per source as well as unioned: the fill below has to know which cells THIS run
    // gave rows to, not merely which files happen to exist.
    let mut written_by_source: HashMap<&str, HashSet<String>> = HashMap::new();
    let mut hex_dirs: HashSet<String> = HashSet::new();
    for (source, hexes) in unit_sets {
        hex_dirs.extend(hexes.iter().cloned());
        written_by_source.entry(source).or_default().extend(hexes);
    }
    let filled = write_empty_osm_tables_where_the_run_had_no_rows(
        osm_extract_dir,
        &hex_dirs,
        &written_by_source,
    )?;
    eprintln!("  {filled} empty OSM table(s) for cells this run gave no rows");
    Ok(hex_dirs.len())
}

/// Every cell this extract created carries BOTH OSM tables, 0-row where nothing
/// stands — the rule `structures.arrow` already follows. Without it an absent
/// `buildings.arrow` means two different things (this cell has no OSM buildings
/// / this cell's tree is broken or unmounted) and the structure builder cannot
/// tell them apart: it would write a valid-looking Overture-only table and erase
/// the emission stock. With it, absent means broken, and the builder fails.
///
/// The pair is RE-DERIVED from this run, never inherited: a cell the run gave no
/// building rows gets the empty table even when a previous extract left a
/// populated one there. Gap-filling alone could not express "these buildings were
/// demolished" — a wall or a house dropped from a newer OSM snapshot would
/// outlive every re-extract, because both output trees are reused in place.
///
/// The empty tables come from the SAME writers as the populated ones, so the
/// schema, the `buildings_v2` contract stamp and the metadata cannot drift; an
/// empty file is byte-identical for every cell (`spatially_batched` returns one
/// empty batch and no bbox key) and lands through the writers' tmp+rename, so a
/// reader never sees a half-written table.
fn write_empty_osm_tables_where_the_run_had_no_rows(
    osm_extract_dir: &Path,
    hex_dirs: &HashSet<String>,
    written_by_source: &HashMap<&str, HashSet<String>>,
) -> Result<usize> {
    let join_stats = JoinStats::default();
    let wrote = |source: &str, hex: &String| {
        written_by_source
            .get(source)
            .is_some_and(|hexes| hexes.contains(hex))
    };
    let mut written = 0usize;
    for hex_str in hex_dirs {
        let has_buildings = wrote("buildings", hex_str);
        let has_barriers = wrote("barriers", hex_str);
        if has_buildings && has_barriers {
            continue;
        }
        let dir = osm_extract_dir.join(hex_str);
        fs::create_dir_all(&dir)?;
        if !has_buildings {
            let hex = u64::from_str_radix(hex_str, 16)
                .map_err(|e| anyhow::anyhow!("hex dir {hex_str} is not hexadecimal: {e}"))?;
            write_buildings(
                &[],
                &dir.join("buildings.arrow"),
                hex,
                &PoiIndex::default(),
                &join_stats,
            )?;
            written += 1;
        }
        if !has_barriers {
            write_barriers(&[], &dir.join("barriers.arrow"))?;
            written += 1;
        }
    }
    Ok(written)
}

/// Sort one spill bucket by hex_id, group consecutive rows, and write one
/// `{source}.arrow` per hex directory. Returns the hex directories it created.
///
/// `sort` groups by hex_id on disk; its stdout streams straight into the encoder —
/// no intermediate `*_sorted.tsv`, so peak scratch stays at the spill size (not +1
/// sorted copy per concurrent unit, ~184 GB for planet roads) and we skip a
/// write+read round-trip. `-S` bounds per-sort RAM and `--parallel=1` keeps each
/// sort single-threaded because up to ~ncpu of these run at once (parallelism is
/// across buckets, not within one sort); the sort is not the bottleneck (the Arrow
/// encode is). A nonzero sort exit (ENOSPC, OOM-kill) aborts finalize — it must
/// never silently drop a whole bucket of features.
fn finalize_bucket(
    source: &str,
    bucket: usize,
    spill_dir: &Path,
    prepared_dir: &Path,
    osm_extract_dir: &Path,
    join_stats: &JoinStats,
) -> Result<HashSet<String>> {
    let mut hex_dirs = HashSet::new();
    let root_dir = cell_dir_for_source(source, prepared_dir, osm_extract_dir);
    let path = spill_dir.join(format!("{source}_{bucket:03}.tsv"));

    // Settlement v2 phase 2: the buildings unit consumes its bucket's POI nodes
    // for the footprint join. A POI inside a building shares its R4 hex → its
    // bucket, so the join is fully local to this unit (no cross-bucket lookup).
    let poi_index = if source == "buildings" {
        load_poi_bucket(spill_dir, bucket)?
    } else {
        PoiIndex::default()
    };

    let mut child = std::process::Command::new("sort")
        .args([
            "-t\t",
            "-k1,1n",
            "-T",
            spill_dir.to_str().unwrap_or("/tmp"),
            "-S",
            "1G",
            "--parallel=1",
        ])
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let reader = BufReader::with_capacity(1 << 20, child.stdout.take().expect("sort stdout piped"));
    let mut current_hex: u64 = 0;
    let mut current_rows: Vec<Vec<String>> = Vec::new();
    let mut corrupt_rows: u64 = 0;
    let mut first_corrupt_id: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        let fields: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();
        if fields.is_empty() {
            continue;
        }
        // A row whose id is not an R4 cell is a torn spill line, never a feature:
        // count it, drop it, and fail the bucket below. Never group it — the id
        // it would be grouped under is fiction.
        let Some(hex_id) = parse_res4_hex_id(&fields[0]) else {
            corrupt_rows += 1;
            first_corrupt_id.get_or_insert_with(|| fields[0].clone());
            continue;
        };

        if hex_id != current_hex && !current_rows.is_empty() {
            hex_dirs.insert(flush_hex(
                source,
                current_hex,
                &current_rows,
                root_dir,
                &poi_index,
                join_stats,
            )?);
            current_rows.clear();
        }
        current_hex = hex_id;
        current_rows.push(fields);
    }
    if !current_rows.is_empty() {
        hex_dirs.insert(flush_hex(
            source,
            current_hex,
            &current_rows,
            root_dir,
            &poi_index,
            join_stats,
        )?);
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("sort failed for {source} bucket {bucket} ({status})");
    }
    if corrupt_rows > 0 {
        anyhow::bail!(
            "{source} bucket {bucket}: {corrupt_rows} spilled row(s) carry a hex id that is not \
             an H3 resolution-4 cell (first: {:?}). The spill is truncated or interleaved — \
             re-run the extract for this input; finalize must not invent the cell those rows \
             would land in.",
            first_corrupt_id.unwrap_or_default()
        );
    }

    eprintln!("    {source} [{bucket:03}]: {} hexes", hex_dirs.len());
    Ok(hex_dirs)
}

/// Load `poi_<bucket>.tsv` (unsorted is fine — `PoiIndex` keys by hex). Absent
/// when a region has no function POIs → an empty index (the join is a no-op).
fn load_poi_bucket(spill_dir: &Path, bucket: usize) -> Result<PoiIndex> {
    let path = spill_dir.join(format!("poi_{bucket:03}.tsv"));
    if !path.exists() {
        return Ok(PoiIndex::default());
    }
    let reader = BufReader::with_capacity(1 << 20, File::open(&path)?);
    Ok(PoiIndex::from_lines(reader.lines().map_while(Result::ok)))
}

/// A spilled hex id, accepted only when it really is an H3 resolution-4 cell.
/// `spill.rs` writes nothing else — `h3_res4` drops a feature whose coordinates
/// resolve to no cell — so anything else is a torn or interleaved spill line.
/// The `parse().unwrap_or(0)` this replaces turned exactly that into cell
/// directories `000000000000000`, `000000000000001` and `000000000000021` in the
/// 2026-06-25 planet run: three inventions nothing downstream could tell from a
/// real cell, and a truncated id that happens to parse would have silently moved
/// real features into the wrong place.
fn parse_res4_hex_id(field: &str) -> Option<u64> {
    let raw: u64 = field.parse().ok()?;
    let cell = CellIndex::try_from(raw).ok()?;
    (cell.resolution() == Resolution::Four).then_some(raw)
}

/// The tree a source's `{source}.arrow` belongs in. Everything a painter reads
/// lands in the PREPARED cell; `buildings` and `barriers` are the raw OSM tables
/// only `scripts/structures/build-structures.py` consumes (it freezes them into
/// the cell's `structures.arrow`), so they land in the OSM extract SOURCE tree
/// instead — a prepared cell then holds exactly the painters' inputs, so the
/// ~128 GB of OSM buildings never travel with the cells that are painted.
fn cell_dir_for_source<'a>(
    source: &str,
    prepared_dir: &'a Path,
    osm_extract_dir: &'a Path,
) -> &'a Path {
    match source {
        "buildings" | "barriers" => osm_extract_dir,
        _ => prepared_dir,
    }
}

/// Write one hex's accumulated rows to `{source}.arrow` under `root_dir`;
/// returns the hex dir name.
fn flush_hex(
    source: &str,
    hex: u64,
    rows: &[Vec<String>],
    root_dir: &Path,
    poi_index: &PoiIndex,
    join_stats: &JoinStats,
) -> Result<String> {
    let hex_str = format!("{hex:015x}");
    let dir = root_dir.join(&hex_str);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{source}.arrow"));
    match source {
        "roads" => write_roads(rows, &path),
        "railways" => write_railways(rows, &path),
        "airport_areas" => write_airport_areas(rows, &path),
        "airport_lines" => write_airport_lines(rows, &path),
        "buildings" => write_buildings(rows, &path, hex, poi_index, join_stats),
        "industrial" => write_industrial(rows, &path),
        "barriers" => write_barriers(rows, &path),
        "leisure" => write_leisure(rows, &path),
        _ => Ok(()),
    }?;
    Ok(hex_str)
}

/// Stamp a per-file contract into an arrow's schema metadata (Convention-B).
fn schema_with_contract(fields: Vec<Field>, key: &str, value: &str) -> Schema {
    let mut md = HashMap::new();
    md.insert(key.to_string(), value.to_string());
    Schema::new(fields).with_metadata(md)
}

/// The ONE arrow-write path for all 8 layer writers: rows are spatially
/// sorted, chunked into record batches, and per-batch bboxes stamped into
/// schema metadata so the popup reader can skip out-of-reach batches without
/// decoding them. `row_bboxes` MUST be
/// parallel to the APPENDED rows — a writer that `continue`s a malformed TSV
/// row pushes no bbox for it.
pub(super) fn write_arrow_spatially_batched(
    path: &Path,
    schema: Schema,
    columns: Vec<ArrayRef>,
    row_bboxes: &[arrow_batching::RowBbox],
) -> Result<()> {
    let (schema, batches) = arrow_batching::spatially_batched(schema, columns, row_bboxes)?;
    // Sibling-temp + rename, mirroring aircraft-extract's write_record_batches:
    // a re-extract runs while Fastify serves the OLD files, and the popup's
    // lazy reader keeps mmaps open long past load time — truncating the live
    // inode via File::create(path) would SIGBUS a later first-touch decode
    // (Codex /gg 2026-07-10). rename() keeps the old inode alive for open
    // maps and swaps readers to the new file atomically.
    let tmp_path = path.with_extension("arrow.tmp");
    let file = File::create(&tmp_path)?;
    let mut writer = FileWriter::try_new(file, &schema)?;
    for batch in &batches {
        writer.write(batch)?;
    }
    writer.finish()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Row envelope of a ≤250 m microsegment (roads/railways/barriers/airport
/// lines): the segment's endpoint box.
pub(super) fn segment_row_bbox(
    s_lat: f64,
    s_lon: f64,
    e_lat: f64,
    e_lon: f64,
) -> arrow_batching::RowBbox {
    [
        s_lat.min(e_lat),
        s_lon.min(e_lon),
        s_lat.max(e_lat),
        s_lon.max(e_lon),
    ]
}

/// Row envelope of a polygon-or-point feature: the exact WKB footprint bbox
/// when present — centroid-only would under-prune large areas (an audible
/// plant edge can sit kilometres from its centroid) — else a degenerate
/// point box at the centroid.
pub(super) fn polygon_row_bbox(
    wkb_hex: &str,
    centroid_lat: f64,
    centroid_lon: f64,
) -> arrow_batching::RowBbox {
    if !wkb_hex.is_empty() {
        if let Some(fp) = noise_compute::wkb::WkbFootprint::parse(wkb_hex) {
            let (min_lat, max_lat, min_lon, max_lon) = fp.bbox();
            return [min_lat, min_lon, max_lat, max_lon];
        }
    }
    [centroid_lat, centroid_lon, centroid_lat, centroid_lon]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A spill row whose id is not an R4 cell must abort the bucket, never become
    /// a cell directory. The 2026-06-25 planet run wrote three of them and every
    /// consumer downstream — the structure builder, the cell inventory, the
    /// prepared identity manifest — treated them as real cells.
    #[test]
    fn a_spill_row_whose_hex_id_is_not_a_resolution_4_cell_aborts_the_bucket() {
        let root = std::env::temp_dir().join("osm-extract-finalize-hex-id-guard");
        let _ = fs::remove_dir_all(&root);
        let spill = root.join("spill");
        let prepared = root.join("prepared");
        let osm = root.join("osm-extract");
        fs::create_dir_all(&spill).unwrap();

        let dobris = u64::from_str_radix("841e309ffffffff", 16).unwrap();
        // A real H3 cell at the WRONG resolution is corruption too: R4 is the one
        // resolution spill.rs writes.
        let res7 = u64::from(
            CellIndex::try_from(dobris)
                .unwrap()
                .center_child(Resolution::Seven)
                .unwrap(),
        );
        let barrier_row = |id: String| {
            format!("{id}\t55\t0\t49.7800\t14.1700\t49.7801\t14.1702\t20.0\t3.0\t0\t2\n")
        };
        let mut tsv = File::create(spill.join("barriers_000.tsv")).unwrap();
        for id in [dobris.to_string(), "1".to_string(), res7.to_string()] {
            tsv.write_all(barrier_row(id).as_bytes()).unwrap();
        }
        drop(tsv);

        let err = finalize_bucket(
            "barriers",
            0,
            &spill,
            &prepared,
            &osm,
            &JoinStats::default(),
        )
        .expect_err("a hex id that is not an R4 cell must fail the bucket");
        let message = err.to_string();
        assert!(message.contains("2 spilled row(s)"), "{message}");
        assert!(message.contains("\"1\""), "{message}");
        assert!(
            !osm.join("000000000000001").exists(),
            "a torn line invented a cell directory"
        );
        assert!(
            !osm.join(format!("{res7:015x}")).exists(),
            "a res-7 id invented a cell directory"
        );
        // The one real row still belongs where it always did.
        assert!(osm.join("841e309ffffffff").join("barriers.arrow").exists());

        assert_eq!(parse_res4_hex_id(&dobris.to_string()), Some(dobris));
        assert_eq!(parse_res4_hex_id("not-a-number"), None);

        fs::remove_dir_all(&root).ok();
    }

    /// The pair is re-derived from the run, not inherited: a source that gave a
    /// cell no rows this time leaves the canonical EMPTY table there, replacing
    /// whatever a previous extract wrote. A source that DID write the cell this
    /// run is left exactly as it wrote it.
    #[test]
    fn a_source_with_no_rows_this_run_replaces_the_table_a_previous_run_left() {
        let root = std::env::temp_dir().join("osm-extract-finalize-empty-pair");
        let _ = fs::remove_dir_all(&root);
        let osm = root.join("osm-extract");

        // Two real R4 cell ids, proven so by the same guard finalize uses.
        let cells = ["841e309ffffffff", "840b26bffffffff"];
        for cell in cells {
            let id = u64::from_str_radix(cell, 16).unwrap();
            assert_eq!(parse_res4_hex_id(&id.to_string()), Some(id));
            fs::create_dir_all(osm.join(cell)).unwrap();
        }
        // cells[0]: this run wrote its barriers; a STALE buildings table from an
        // older extract is still on disk. cells[1]: nothing written this run.
        fs::write(
            osm.join(cells[0]).join("barriers.arrow"),
            b"written this run",
        )
        .unwrap();
        fs::write(
            osm.join(cells[0]).join("buildings.arrow"),
            b"last run's houses",
        )
        .unwrap();

        let hex_dirs: HashSet<String> = cells.iter().map(|c| c.to_string()).collect();
        let mut written_by_source: HashMap<&str, HashSet<String>> = HashMap::new();
        written_by_source.insert("barriers", HashSet::from([cells[0].to_string()]));
        let filled =
            write_empty_osm_tables_where_the_run_had_no_rows(&osm, &hex_dirs, &written_by_source)
                .unwrap();
        assert_eq!(filled, 3); // both cells' buildings + cells[1]'s barriers

        // What the run wrote stays untouched.
        assert_eq!(
            fs::read(osm.join(cells[0]).join("barriers.arrow")).unwrap(),
            b"written this run"
        );
        // What it did not write is the canonical empty table, stale bytes gone.
        let replaced = fs::read(osm.join(cells[0]).join("buildings.arrow")).unwrap();
        let fresh = fs::read(osm.join(cells[1]).join("buildings.arrow")).unwrap();
        assert_ne!(replaced, b"last run's houses");
        assert_eq!(replaced, fresh, "an empty table must not vary cell to cell");
        assert!(osm.join(cells[1]).join("barriers.arrow").exists());

        let reader = arrow::ipc::reader::FileReader::try_new(
            File::open(osm.join(cells[1]).join("buildings.arrow")).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(
            reader
                .schema()
                .metadata()
                .get("buildings_contract")
                .map(String::as_str),
            Some(BUILDINGS_CONTRACT_V2)
        );
        assert_eq!(
            reader.map(|b| b.unwrap().num_rows()).sum::<usize>(),
            0,
            "the empty table is not empty"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// End to end over the real `finalize`: a building demolished between two OSM
    /// snapshots must disappear from the extract tree. Both output trees are reused
    /// in place across runs, so a fill that only closed gaps would have kept the
    /// old house for ever — and the structure builder would keep painting it.
    #[test]
    fn a_re_extract_can_say_a_cell_lost_its_buildings_and_get_them_back() {
        let root = std::env::temp_dir().join("osm-extract-finalize-re-extract");
        let _ = fs::remove_dir_all(&root);
        let prepared = root.join("prepared");
        let osm = root.join("osm-extract");
        let cell = "841e309ffffffff";
        let id = u64::from_str_radix(cell, 16).unwrap();

        // TSV layouts per write_buildings / write_barriers / write_roads.
        let building = format!("{id}\t7\t49.7800\t14.1700\t11\t0\t9.5\t3\thouse\t\t\t0\t0\t\n");
        let barrier = format!("{id}\t55\t0\t49.7800\t14.1700\t49.7801\t14.1702\t20.0\t3.0\t0\t2\n");
        let road = format!("{id}\t9\t0\t49.7800\t14.1700\t49.7801\t14.1702\t20.0\n");

        let run = |name: &str, files: &[(&str, &String)]| {
            let spill = root.join(name);
            fs::create_dir_all(&spill).unwrap();
            for (source, body) in files {
                fs::write(spill.join(format!("{source}_000.tsv")), body.as_str()).unwrap();
            }
            finalize(&spill, &prepared, &osm, 1).unwrap()
        };

        // Snapshot 1: the cell has a house and a wall.
        assert_eq!(
            run(
                "spill1",
                &[("buildings", &building), ("barriers", &barrier)]
            ),
            1
        );
        let with_house = fs::read(osm.join(cell).join("buildings.arrow")).unwrap();
        let with_wall = fs::read(osm.join(cell).join("barriers.arrow")).unwrap();

        // Snapshot 2: both are gone from OSM; only a road is left in the cell.
        assert_eq!(run("spill2", &[("roads", &road)]), 1);
        let after = fs::read(osm.join(cell).join("buildings.arrow")).unwrap();
        assert_ne!(
            after, with_house,
            "a demolished building survived the re-extract"
        );
        assert_ne!(
            fs::read(osm.join(cell).join("barriers.arrow")).unwrap(),
            with_wall,
            "a removed wall survived the re-extract"
        );
        let reader = arrow::ipc::reader::FileReader::try_new(
            File::open(osm.join(cell).join("buildings.arrow")).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(reader.map(|b| b.unwrap().num_rows()).sum::<usize>(), 0);

        // Snapshot 3: the house and the wall are mapped again — byte for byte what
        // snapshot 1 produced, which is also the unchanged-re-extract case.
        assert_eq!(
            run(
                "spill3",
                &[("buildings", &building), ("barriers", &barrier)]
            ),
            1
        );
        assert_eq!(
            fs::read(osm.join(cell).join("buildings.arrow")).unwrap(),
            with_house
        );
        assert_eq!(
            fs::read(osm.join(cell).join("barriers.arrow")).unwrap(),
            with_wall
        );

        fs::remove_dir_all(&root).ok();
    }
}
