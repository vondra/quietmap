//! Tests for the transport provenance store: local temporary directories only.

use super::TransportWriter;
use crate::{
    microsegment::{SourceInterval, SourcePosition},
    transport::output_path,
};
use anyhow::Result;
use rusqlite::Connection;
use std::{
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
        length_m: 0.0,
    }
}

#[test]
fn ways_pieces_and_sparse_aliases_roundtrip_exactly_across_families() -> Result<()> {
    let spill = TempDir::new("roundtrip");
    let mut writer = TransportWriter::new(spill.path())?;
    writer.write_way(
        100,
        "roads",
        &[
            (1, Some([50.0, 14.0])),
            (2, Some([50.0, 14.0])), // explicit zero-length hop aliases 1 and 2 within roads
            (3, Some([50.0, 14.002])),
            (4, None),                 // missing coordinate never bridges identities
            (5, Some([50.0, 14.002])), // equal to node 3 but not consecutive: no alias
            (6, Some([50.0, 14.003])),
        ],
    )?;
    writer.write_piece(100, 0, "z9/275/173", &interval((1, 1.0 / 3.0), (2, 0.0)))?;
    writer.write_piece(100, 1, "z9/276/173", &interval((4, 0.0), (5, 0.0)))?;
    writer.write_way(
        101,
        "roads",
        &[(2, Some([50.0, 14.0])), (9, Some([50.0, 14.0]))],
    )?;
    // A zero-length railway way emits no acoustic piece yet still aliases its node IDs.
    writer.write_way(
        200,
        "railways",
        &[(2, Some([50.0, 14.0])), (7, Some([50.0, 14.0]))],
    )?;
    writer.finish()?;

    let connection = Connection::open(spill.path().join("transport.sqlite"))?;
    let ways = {
        let mut statement = connection
            .prepare("SELECT osm_id, family, nodes_json FROM source_ways ORDER BY osm_id")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    assert_eq!(ways.len(), 3);
    assert_eq!((ways[0].0, ways[0].1.as_str()), (100, "roads"));
    assert_eq!(
        serde_json::from_str::<Vec<(String, Option<[f64; 2]>)>>(&ways[0].2)?,
        vec![
            ("1".to_string(), Some([50.0, 14.0])),
            ("2".to_string(), Some([50.0, 14.0])),
            ("3".to_string(), Some([50.0, 14.002])),
            ("4".to_string(), None),
            ("5".to_string(), Some([50.0, 14.002])),
            ("6".to_string(), Some([50.0, 14.003])),
        ]
    );
    assert_eq!((ways[1].0, ways[1].1.as_str()), (101, "roads"));
    assert_eq!((ways[2].0, ways[2].1.as_str()), (200, "railways"));

    let pieces = {
        let mut statement = connection.prepare(
            "SELECT way_id, segment_idx, square, start_vertex, start_fraction, end_vertex, end_fraction
             FROM source_pieces ORDER BY way_id, segment_idx",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i16>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, f64>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    assert_eq!(
        pieces,
        vec![
            (100, 0, "z9/275/173".to_string(), 1, 1.0 / 3.0, 2, 0.0),
            (100, 1, "z9/276/173".to_string(), 4, 0.0, 5, 0.0),
        ]
    );

    let aliases = {
        let mut statement = connection.prepare(
            "SELECT family, node_id, canonical_node FROM node_aliases ORDER BY family, node_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    assert_eq!(
        aliases,
        vec![
            ("railways".to_string(), 7, 2),
            ("roads".to_string(), 2, 1),
            ("roads".to_string(), 9, 1),
        ]
    );
    Ok(())
}

#[test]
fn is_complete_is_true_only_after_finish_commits() -> Result<()> {
    assert!(!TransportWriter::is_complete(TempDir::new("empty").path()));
    let spill = TempDir::new("complete");
    let mut abandoned = TransportWriter::new(spill.path())?;
    abandoned.write_way(
        1,
        "roads",
        &[(1, Some([50.0, 14.0])), (2, Some([50.0, 14.001]))],
    )?;
    assert!(!TransportWriter::is_complete(spill.path()));
    drop(abandoned); // dropping mid-transaction rolls back
    assert!(!TransportWriter::is_complete(spill.path()));

    let mut writer = TransportWriter::new(spill.path())?;
    writer.write_way(
        1,
        "roads",
        &[(1, Some([50.0, 14.0])), (2, Some([50.0, 14.001]))],
    )?;
    writer.finish()?;
    assert!(TransportWriter::is_complete(spill.path()));
    Ok(())
}

#[test]
fn publish_places_exact_bytes_beside_prepared_and_keeps_previous_target_on_refusal() -> Result<()> {
    let output = TempDir::new("publish");
    let prepared = output.path().join("prepared").join("2026");
    std::fs::create_dir_all(&prepared)?;
    let target = output.path().join("prepared").join("2026.transport.sqlite");
    std::fs::write(&target, b"previous topology")?;

    let incomplete = TempDir::new("publish-incomplete");
    let mut abandoned = TransportWriter::new(incomplete.path())?;
    abandoned.write_way(6, "roads", &[(1, Some([50.0, 14.0]))])?;
    drop(abandoned);
    assert!(TransportWriter::publish(incomplete.path(), &prepared).is_err());
    assert_eq!(std::fs::read(&target)?, b"previous topology");
    assert!(!output
        .path()
        .join("prepared")
        .join("2026.transport.sqlite.copying")
        .exists());

    let spill = TempDir::new("publish-source");
    let mut writer = TransportWriter::new(spill.path())?;
    writer.write_way(
        5,
        "railways",
        &[(1, Some([50.0, 14.0])), (2, Some([50.0, 14.001]))],
    )?;
    writer.finish()?;
    TransportWriter::publish(spill.path(), &prepared)?;
    assert_eq!(
        std::fs::read(&target)?,
        std::fs::read(spill.path().join("transport.sqlite"))?
    );
    assert_eq!(std::fs::read_dir(&prepared)?.count(), 0);
    Ok(())
}

#[test]
fn output_path_derives_a_sibling_for_trailing_slash_and_dotted_names() -> Result<()> {
    assert_eq!(
        output_path(Path::new("/srv/qm/output/prepared/2026"))?,
        PathBuf::from("/srv/qm/output/prepared/2026.transport.sqlite")
    );
    assert_eq!(
        output_path(Path::new("/srv/qm/output/prepared/2026/"))?,
        PathBuf::from("/srv/qm/output/prepared/2026.transport.sqlite")
    );
    assert_eq!(
        output_path(Path::new("/srv/qm/output/prepared/v1.2"))?,
        PathBuf::from("/srv/qm/output/prepared/v1.2.transport.sqlite")
    );
    assert!(output_path(Path::new("/srv/qm/output/prepared/..")).is_err());
    Ok(())
}
