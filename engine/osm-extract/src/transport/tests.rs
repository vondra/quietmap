//! Tests for the transport provenance spill: local temporary directories only.

use super::{load_node_aliases, piece_tail, way_metres, year_sibling_path, TransportSpill};
use crate::finalize::write_railway_globals::write_railway_ways_in_batches;
use crate::finalize::write_source_pieces::write_source_pieces;
use crate::microsegment::{SourceInterval, SourcePosition};
use crate::spill::write_segment_row_prefix;
use anyhow::Result;
use arrow::array::{Array, Float64Array, Int32Array, Int64Array, ListArray};
use arrow::ipc::reader::FileReader;
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

static TEMP_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// Process-unique temporary directory removed on drop; no PBF or node-cache machinery.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "qm-transport-{tag}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn interval(start: (usize, f64), end: (usize, f64)) -> SourceInterval {
    SourceInterval {
        start: SourcePosition {
            vertex_index: start.0,
            fraction_to_next: start.1,
        },
        end: SourcePosition {
            vertex_index: end.0,
            fraction_to_next: end.1,
        },
        length_m: 12.3,
    }
}

/// One railway spill row of way 100 as Pass 2 writes it: the segment prefix plus the piece tail.
fn railway_spill_row(
    segment: i16,
    nodes: &[super::ResolvedNode],
    piece: &SourceInterval,
    metres: &[f64],
) -> Result<Vec<String>> {
    let mut line = Vec::new();
    let geometry = ([50.0, 14.0], [50.001, 14.001], piece.length_m);
    write_segment_row_prefix(
        &mut line,
        grid::square_of(50.0, 14.0),
        100,
        segment,
        &geometry,
    )?;
    let mut row: Vec<String> = String::from_utf8(line)?
        .split('\t')
        .map(str::to_owned)
        .collect();
    row.push(piece_tail(nodes, piece, Some(Some(metres))));
    Ok(row)
}

/// The identity key is the double, not its text: fractions and metres must survive the spill
/// line bit for bit, and the rows handed to the layer writer must stay in their own order.
#[test]
fn piece_tail_round_trips_fractions_metres_and_chain_exactly_through_spill_text() -> Result<()> {
    let directory = TempDir::new("tail");
    let nodes = [
        (7, Some([50.0000001, 14.0])),
        (3, Some([50.0012345, 14.0054321])),
        (9, Some([50.0031, 14.0071])),
    ];
    let metres = way_metres(&nodes).expect("complete way");
    let fractions = [1.0 / 3.0, 2.0 / 7.0, 1e-7, 0.1 + 0.2];
    let mut rows = Vec::new();
    // Descending segment order proves the pieces file sorts a permutation only.
    for (segment, fraction) in fractions.iter().enumerate().rev() {
        let piece = interval((0, *fraction), (1, fractions[0]));
        rows.push(railway_spill_row(segment as i16, &nodes, &piece, &metres)?);
    }
    let before = rows.clone();
    let path = directory.path().join("railways.pieces.arrow");
    write_source_pieces("railways", &rows, &HashMap::from([(7, 2)]), &path)?;
    assert_eq!(rows, before);

    let batch = FileReader::try_new(File::open(&path)?, None)?
        .next()
        .expect("one batch")?;
    let f64s = |name: &str| {
        batch
            .column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .clone()
    };
    let (start_fraction, first_m, from_m) = (
        f64s("start_fraction"),
        f64s("first_vertex_m"),
        f64s("from_m"),
    );
    for (segment, fraction) in fractions.iter().enumerate() {
        assert_eq!(start_fraction.value(segment).to_bits(), fraction.to_bits());
        assert_eq!(first_m.value(segment).to_bits(), metres[0].to_bits());
        let expected = metres[0] + fraction * (metres[1] - metres[0]);
        assert_eq!(from_m.value(segment).to_bits(), expected.to_bits());
    }
    let start_node = batch.column_by_name("start_node").unwrap();
    let start_node = start_node.as_any().downcast_ref::<Int64Array>().unwrap();
    assert_eq!(start_node.value(0), 2, "alias is baked into the node id");
    let chain = batch.column_by_name("chain_lat_e7").unwrap();
    let chain = chain.as_any().downcast_ref::<ListArray>().unwrap().value(0);
    let chain = chain.as_any().downcast_ref::<Int32Array>().unwrap();
    assert_eq!(chain.values(), &[500000001, 500012345, 500031000]);
    Ok(())
}

/// A silently zeroed value would become a wrong restored parent: each malformed class fails the
/// build and leaves neither a pieces file nor its temporary sibling.
#[test]
fn repeated_piece_and_malformed_tail_fail_without_writing_a_pieces_file() -> Result<()> {
    let directory = TempDir::new("malformed");
    let path = directory.path().join("railways.pieces.arrow");
    let nodes = [(7, Some([50.0, 14.0])), (3, Some([50.001, 14.001]))];
    let metres = way_metres(&nodes).expect("complete way");
    let row = railway_spill_row(0, &nodes, &interval((0, 0.0), (0, 0.5)), &metres)?;
    let mut without_chain = row.clone();
    let tail = without_chain.last_mut().expect("piece tail");
    *tail = tail.rsplit_once(',').expect("chain field").0.to_owned();
    for (rows, message) in [
        (vec![row.clone(), row], "repeated railways piece 100:0"),
        (vec![without_chain], "malformed piece tail"),
    ] {
        let error = write_source_pieces("railways", &rows, &HashMap::new(), &path).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    }
    Ok(())
}

/// The world file exceeds what a reader can hold as one buffer, so it must never be one batch.
#[test]
fn railway_ways_close_a_record_batch_at_the_list_value_bound() -> Result<()> {
    let directory = TempDir::new("batches");
    let way = |id: i64| format!("{id}\t1,500000000,140000000;2,500010000,140000000\t7\n");
    std::fs::write(
        directory.path().join(super::RAILWAY_WAYS_SPILL),
        [way(30), way(10), way(20)].concat(),
    )?;
    // Each way adds four list values (row, two nodes, one square): the second way closes a batch.
    write_railway_ways_in_batches(
        directory.path(),
        &directory.path().join("2026"),
        &HashMap::new(),
        5,
    )?;
    let batches = arrow::ipc::reader::StreamReader::try_new(
        File::open(directory.path().join("2026.railway-ways.arrow"))?,
        None,
    )?
    .collect::<Result<Vec<_>, _>>()?;
    let way_ids: Vec<Vec<i64>> = batches
        .iter()
        .map(|batch| {
            let ids = batch.column_by_name("way_id").unwrap();
            ids.as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .to_vec()
        })
        .collect();
    assert_eq!(way_ids, [vec![10, 20], vec![30]]);
    Ok(())
}

#[test]
fn aliases_are_sparse_minimum_rooted_and_separate_per_family() -> Result<()> {
    let spill = TempDir::new("aliases");
    let mut writer = TransportSpill::new(spill.path())?;
    writer.observe_way(
        "roads",
        &[
            (1, Some([50.0, 14.0])),
            (2, Some([50.0, 14.0])), // explicit zero-length hop aliases 1 and 2 within roads
            (3, Some([50.0, 14.002])),
            (4, None),                 // missing coordinate never bridges identities
            (5, Some([50.0, 14.002])), // equal to node 3 but not consecutive: no alias
        ],
    );
    writer.observe_way("roads", &[(2, Some([50.0, 14.0])), (9, Some([50.0, 14.0]))]);
    // A zero-length railway way emits no acoustic piece yet still aliases its node IDs.
    writer.observe_way(
        "railways",
        &[(2, Some([50.0, 14.0])), (7, Some([50.0, 14.0]))],
    );
    writer.finish()?;
    let aliases = load_node_aliases(spill.path())?;
    assert_eq!(aliases["roads"], HashMap::from([(2, 1), (9, 1)]));
    assert_eq!(aliases["railways"], HashMap::from([(7, 2)]));
    Ok(())
}

#[test]
fn year_sibling_path_handles_trailing_slash_and_dotted_names() -> Result<()> {
    for (prepared, sibling) in [
        (
            "/srv/qm/prepared/2026",
            "/srv/qm/prepared/2026.railway-ways.arrow",
        ),
        (
            "/srv/qm/prepared/2026/",
            "/srv/qm/prepared/2026.railway-ways.arrow",
        ),
        (
            "/srv/qm/prepared/v1.2",
            "/srv/qm/prepared/v1.2.railway-ways.arrow",
        ),
    ] {
        assert_eq!(
            year_sibling_path(Path::new(prepared), "railway-ways")?,
            PathBuf::from(sibling)
        );
    }
    assert!(year_sibling_path(Path::new("/srv/qm/prepared/.."), "railway-ways").is_err());
    Ok(())
}
