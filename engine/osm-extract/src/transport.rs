//! SQLite provenance store linking published transport pieces to original OSM ways, node chains, and identity aliases.

use crate::microsegment::SourceInterval;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
};

/// PRAGMA user_version stamped when the single write transaction commits; readers gate on it.
const SCHEMA_VERSION: i32 = 1;

pub struct TransportWriter {
    database: PathBuf,
    connection: Connection,
    /// Per-family sparse union-find (node → parent) fed only by explicit zero-length
    /// source hops; a set's root is its minimum node ID, so canonical identity is
    /// deterministic and `node_aliases` stays sparse.
    alias_parents: HashMap<String, HashMap<i64, i64>>,
}

impl TransportWriter {
    /// Opens `spill_dir/transport.sqlite`, creates the schema, and begins the single write transaction.
    pub fn new(spill_dir: &Path) -> Result<Self> {
        let database = spill_dir.join("transport.sqlite");
        let connection =
            Connection::open(&database).with_context(|| format!("open {}", database.display()))?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        connection
            .execute_batch(include_str!("transport.sql"))
            .with_context(|| format!("create transport schema in {}", database.display()))?;
        Ok(Self {
            connection,
            alias_parents: HashMap::new(),
            database,
        })
    }

    /// Records the complete original node chain and unions the IDs of consecutive
    /// distinct nodes whose coordinates are exactly equal (explicit zero-length hops).
    pub fn write_way(
        &mut self,
        way_id: i64,
        family: &str,
        nodes: &[(i64, Option<[f64; 2]>)],
    ) -> Result<()> {
        for adjacent in nodes.windows(2) {
            let (a, b) = (adjacent[0], adjacent[1]);
            // Missing coordinates must not bridge identities, so require a present pair.
            if a.0 != b.0 && a.1.is_some() && a.1 == b.1 {
                self.union_aliased(family, a.0, b.0);
            }
        }
        let chain: Vec<(String, Option<[f64; 2]>)> = nodes
            .iter()
            .map(|(node_id, coordinates)| (node_id.to_string(), *coordinates))
            .collect();
        self.connection
            .prepare_cached(
                "INSERT INTO source_ways(osm_id, family, nodes_json) VALUES (?1, ?2, ?3)",
            )?
            .execute(params![way_id, family, serde_json::to_string(&chain)?])
            .with_context(|| format!("store source way {way_id}"))?;
        Ok(())
    }

    /// Records one acoustic piece's square and its exact interval on the original vertex chain.
    pub fn write_piece(
        &mut self,
        way_id: i64,
        segment_idx: i16,
        square: &str,
        interval: &SourceInterval,
    ) -> Result<()> {
        self.connection
            .prepare_cached(
                "INSERT INTO source_pieces(way_id, segment_idx, square, start_vertex, start_fraction,
                     end_vertex, end_fraction) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                way_id,
                segment_idx,
                square,
                interval.start.vertex_index as i64,
                interval.start.fraction_to_next,
                interval.end.vertex_index as i64,
                interval.end.fraction_to_next,
            ])
            .with_context(|| format!("store piece {way_id}/{segment_idx}"))?;
        Ok(())
    }

    /// Materializes aliases, stamps the schema version, commits, closes, and syncs the bytes.
    pub fn finish(mut self) -> Result<()> {
        let mut aliases: Vec<(String, i64, i64)> = Vec::new();
        for (family, parents) in &mut self.alias_parents {
            let members: Vec<i64> = parents.keys().copied().collect();
            for node in members {
                let canonical = find_root(parents, node);
                if canonical != node {
                    aliases.push((family.clone(), node, canonical));
                }
            }
        }
        aliases.sort();
        for (family, node_id, canonical_node) in aliases {
            self.connection
                .prepare_cached(
                    "INSERT INTO node_aliases(family, node_id, canonical_node) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![family, node_id, canonical_node])?;
        }
        self.connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        self.connection.execute_batch("COMMIT")?;
        drop(self.connection);
        File::open(&self.database)
            .and_then(|file| file.sync_all())
            .with_context(|| format!("sync {}", self.database.display()))?;
        Ok(())
    }

    /// True only when the spill database exists, is readable, and carries the current schema version.
    pub fn is_complete(spill_dir: &Path) -> bool {
        Connection::open_with_flags(
            spill_dir.join("transport.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|connection| {
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
        })
        .map(|version| version == SCHEMA_VERSION)
        .unwrap_or(false)
    }

    /// Publishes the completed spill database as a sibling of the prepared directory.
    pub fn publish(spill_dir: &Path, output_directory: &Path) -> Result<()> {
        if !Self::is_complete(spill_dir) {
            bail!(
                "incomplete transport database {}; nothing published",
                spill_dir.join("transport.sqlite").display()
            );
        }
        let source = spill_dir.join("transport.sqlite");
        let target = output_path(output_directory)?;
        let temp = {
            let mut name = target
                .file_name()
                .expect("output_path yields a file name")
                .to_os_string();
            name.push(".copying");
            target.parent().unwrap_or(Path::new("")).join(name)
        };
        let published: Result<()> = (|| {
            std::fs::copy(&source, &temp)
                .with_context(|| format!("copy {} to {}", source.display(), temp.display()))?;
            File::open(&temp)
                .and_then(|file| file.sync_all())
                .with_context(|| format!("sync {}", temp.display()))?;
            std::fs::rename(&temp, &target)
                .with_context(|| format!("publish {}", target.display()))?;
            Ok(())
        })();
        if published.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        published
    }

    fn union_aliased(&mut self, family: &str, a: i64, b: i64) {
        let parents = self.alias_parents.entry(family.to_owned()).or_default();
        let root_a = find_root(parents, a);
        let root_b = find_root(parents, b);
        if root_a != root_b {
            let (minimum, other) = if root_a < root_b {
                (root_a, root_b)
            } else {
                (root_b, root_a)
            };
            parents.insert(other, minimum);
        }
    }
}

/// Sibling of the prepared year directory that carries the published database,
/// e.g. `output/prepared/2026` → `output/prepared/2026.transport.sqlite`.
pub fn output_path(prepared_directory: &Path) -> Result<PathBuf> {
    let file_name = prepared_directory
        .file_name()
        .with_context(|| {
            format!(
                "{} has no final directory name",
                prepared_directory.display()
            )
        })?
        .to_os_string();
    let mut published = file_name;
    published.push(".transport.sqlite");
    Ok(prepared_directory
        .parent()
        .unwrap_or(Path::new(""))
        .join(published))
}

/// Union-find root with path compression; an absent entry is its own root.
fn find_root(parents: &mut HashMap<i64, i64>, node: i64) -> i64 {
    let mut root = node;
    while let Some(&parent) = parents.get(&root) {
        root = parent;
    }
    let mut walked = node;
    while walked != root {
        let parent = parents[&walked];
        parents.insert(walked, root);
        walked = parent;
    }
    root
}

#[cfg(test)]
mod tests;
