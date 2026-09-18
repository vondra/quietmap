//! Transport provenance in the spill: piece tails on segment rows, railway way chains, train routes and node aliases.

use crate::microsegment::SourceInterval;
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const RAILWAY_WAYS_SPILL: &str = "railway_ways.tsv";
pub const TRAIN_ROUTES_SPILL: &str = "train_routes.tsv";
pub const NODE_ALIASES_SPILL: &str = "node_aliases.tsv";

pub type ResolvedNode = (i64, Option<[f64; 2]>);
/// Per family: aliased node id → canonical (minimum) node id of its zero-length-hop set.
pub type NodeAliases = HashMap<String, HashMap<i64, i64>>;

/// Owned train-route membership extracted while the PBF relation is still alive.
pub struct TrainRouteRecord {
    pub osm_id: i64,
    pub members: Vec<(String, String, String)>,
}

impl TrainRouteRecord {
    /// `None` when the relation is not `route=train`.
    pub fn from_relation(relation: &osmpbf::Relation<'_>) -> Result<Option<Self>> {
        if !relation
            .tags()
            .any(|(key, value)| key == "route" && value == "train")
        {
            return Ok(None);
        }
        let mut members = Vec::new();
        for member in relation.members() {
            let kind = match member.member_type {
                osmpbf::RelMemberType::Node => "n",
                osmpbf::RelMemberType::Way => "w",
                osmpbf::RelMemberType::Relation => "r",
            };
            members.push((
                kind.to_string(),
                member.member_id.to_string(),
                member.role()?.to_owned(),
            ));
        }
        Ok(Some(Self {
            osm_id: relation.id(),
            members,
        }))
    }
}

/// The node cache holds `i32` e7 degrees, so rounding recovers the stored integer exactly;
/// truncation would land one unit (1.1 cm) low for about half of all coordinates.
pub fn coordinate_e7(degrees: f64) -> i32 {
    let e7 = (degrees * 1e7).round() as i32;
    assert!(
        e7 as f64 / 1e7 == degrees,
        "coordinate {degrees} is not an exact e7 value"
    );
    e7
}

/// Whole-way cumulative metres; `None` when a coordinate is missing or the way has under two nodes.
pub fn way_metres(nodes: &[ResolvedNode]) -> Option<Vec<f64>> {
    let points: Vec<[f64; 2]> = nodes.iter().map(|node| node.1).collect::<Option<_>>()?;
    (points.len() >= 2).then(|| grid::geo::cumulative_flat_metres(0.0, &points))
}

/// One comma-separated spill field per piece, so finalize holds one extra string per row:
/// `start_vertex,start_fraction,end_vertex,end_fraction,start_node,end_node`, and for railways
/// `,first_vertex_m,from_m,to_m,lat_e7;lon_e7;…` (metres empty on an incomplete way).
/// Node ids are raw; aliases are known only after Pass 2 and are applied in finalize.
pub fn piece_tail(
    nodes: &[ResolvedNode],
    interval: &SourceInterval,
    railway_metres: Option<Option<&[f64]>>,
) -> String {
    let (start, end) = (interval.start, interval.end);
    assert!(
        end.vertex_index <= u16::MAX as usize,
        "source vertex {} exceeds UInt16",
        end.vertex_index
    );
    let mut tail = format!(
        "{},{},{},{},{},{}",
        start.vertex_index,
        start.fraction_to_next,
        end.vertex_index,
        end.fraction_to_next,
        nodes[start.vertex_index].0,
        nodes[end.vertex_index].0
    );
    let Some(metres) = railway_metres else {
        return tail;
    };
    match metres {
        Some(metres) => {
            let at = |vertex: usize, fraction: f64| {
                if fraction == 0.0 {
                    metres[vertex]
                } else {
                    metres[vertex] + fraction * (metres[vertex + 1] - metres[vertex])
                }
            };
            write!(
                tail,
                ",{},{},{},",
                metres[start.vertex_index],
                at(start.vertex_index, start.fraction_to_next),
                at(end.vertex_index, end.fraction_to_next)
            )
        }
        None => write!(tail, ",,,,"),
    }
    .expect("write to string");
    let last_vertex = end.vertex_index + usize::from(end.fraction_to_next > 0.0);
    for (index, node) in nodes[start.vertex_index..=last_vertex].iter().enumerate() {
        let [lat, lon] = node.1.expect("a piece spans resolved nodes only");
        let separator = if index == 0 { "" } else { ";" };
        write!(
            tail,
            "{separator}{};{}",
            coordinate_e7(lat),
            coordinate_e7(lon)
        )
        .expect("write to string");
    }
    tail
}

pub struct TransportSpill {
    spill_dir: PathBuf,
    railway_ways: BufWriter<File>,
    train_routes: BufWriter<File>,
    /// Per-family sparse union-find (node → parent) fed only by explicit zero-length
    /// source hops; a set's root is its minimum node ID, so canonical identity is deterministic.
    alias_parents: HashMap<String, HashMap<i64, i64>>,
}

impl TransportSpill {
    pub fn new(spill_dir: &Path) -> Result<Self> {
        let create = |name: &str| -> Result<BufWriter<File>> {
            let path = spill_dir.join(name);
            Ok(BufWriter::with_capacity(
                1 << 20,
                File::create(&path).with_context(|| format!("create {}", path.display()))?,
            ))
        };
        Ok(Self {
            spill_dir: spill_dir.to_path_buf(),
            railway_ways: create(RAILWAY_WAYS_SPILL)?,
            train_routes: create(TRAIN_ROUTES_SPILL)?,
            alias_parents: HashMap::new(),
        })
    }

    /// Unions the IDs of consecutive distinct nodes whose coordinates are exactly equal.
    pub fn observe_way(&mut self, family: &str, nodes: &[ResolvedNode]) {
        for adjacent in nodes.windows(2) {
            let (a, b) = (adjacent[0], adjacent[1]);
            // Missing coordinates must not bridge identities, so require a present pair.
            if a.0 != b.0 && a.1.is_some() && a.1 == b.1 {
                self.union_aliased(family, a.0, b.0);
            }
        }
    }

    /// `way_id \t id,lat_e7,lon_e7;… \t square_key;…` — called after the way's pieces are known.
    pub fn write_railway_way(
        &mut self,
        way_id: i64,
        nodes: &[ResolvedNode],
        piece_squares: &BTreeSet<u32>,
    ) -> Result<()> {
        let out = &mut self.railway_ways;
        write!(out, "{way_id}\t")?;
        for (index, (node_id, coordinates)) in nodes.iter().enumerate() {
            let separator = if index == 0 { "" } else { ";" };
            match coordinates {
                Some([lat, lon]) => write!(
                    out,
                    "{separator}{node_id},{},{}",
                    coordinate_e7(*lat),
                    coordinate_e7(*lon)
                )?,
                None => write!(out, "{separator}{node_id},,")?,
            }
        }
        write!(out, "\t")?;
        for (index, square) in piece_squares.iter().enumerate() {
            write!(out, "{}{square}", if index == 0 { "" } else { ";" })?;
        }
        writeln!(out)?;
        Ok(())
    }

    /// Retains every train-route member in source order, including repeats and unresolved references.
    pub fn write_train_route(&mut self, route: &TrainRouteRecord) -> Result<()> {
        writeln!(
            self.train_routes,
            "{}\t{}",
            route.osm_id,
            serde_json::to_string(&route.members)?
        )?;
        Ok(())
    }

    /// Materializes aliases and syncs all three files; the caller writes the spill completion marker after this.
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
        let mut alias_file = BufWriter::new(File::create(self.spill_dir.join(NODE_ALIASES_SPILL))?);
        for (family, node_id, canonical_node) in aliases {
            writeln!(alias_file, "{family}\t{node_id}\t{canonical_node}")?;
        }
        for file in [
            &mut alias_file,
            &mut self.railway_ways,
            &mut self.train_routes,
        ] {
            file.flush()?;
            file.get_ref().sync_all()?;
        }
        Ok(())
    }

    fn union_aliased(&mut self, family: &str, a: i64, b: i64) {
        let parents = self.alias_parents.entry(family.to_owned()).or_default();
        let root_a = find_root(parents, a);
        let root_b = find_root(parents, b);
        if root_a != root_b {
            parents.insert(root_a.max(root_b), root_a.min(root_b));
        }
    }
}

pub fn load_node_aliases(spill_dir: &Path) -> Result<NodeAliases> {
    let path = spill_dir.join(NODE_ALIASES_SPILL);
    let mut aliases = NodeAliases::new();
    let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<&str> = line.split('\t').collect();
        ensure!(fields.len() == 3, "malformed node alias row {line:?}");
        aliases
            .entry(fields[0].to_owned())
            .or_default()
            .insert(fields[1].parse()?, fields[2].parse()?);
    }
    Ok(aliases)
}

/// Sibling of the prepared year directory, e.g. `prepared/2026` + `railway-ways` →
/// `prepared/2026.railway-ways.arrow`.
pub fn year_sibling_path(prepared_directory: &Path, kind: &str) -> Result<PathBuf> {
    let Some(file_name) = prepared_directory.file_name() else {
        bail!(
            "{} has no final directory name",
            prepared_directory.display()
        );
    };
    let mut sibling = file_name.to_os_string();
    sibling.push(format!(".{kind}.arrow"));
    Ok(prepared_directory
        .parent()
        .unwrap_or(Path::new(""))
        .join(sibling))
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
