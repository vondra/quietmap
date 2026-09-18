//! TEST-ONLY: rebuild the transport store files from a retired `transport.sqlite` subset through the
//! production spill text and Arrow writers, for equivalence canaries without a world re-extract.

use anyhow::{ensure, Context, Result};
use osm_extract::finalize::write_railway_globals::{write_railway_ways, write_train_routes};
use osm_extract::finalize::write_source_pieces::write_source_pieces;
use osm_extract::microsegment::{SourceInterval, SourcePosition};
use osm_extract::spill::{spill_key, write_segment_row_prefix};
use osm_extract::transport::{
    load_node_aliases, piece_tail, way_metres, ResolvedNode, TrainRouteRecord, TransportSpill,
    NODE_ALIASES_SPILL,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

/// The store kept no piece length; this repeats `microsegment::split_range`: a fractional piece
/// is an equal part of one over-long hop, any other piece sums its hops left to right.
fn extractor_length_m(nodes: &[ResolvedNode], start: SourcePosition, end: SourcePosition) -> f32 {
    let hop = |vertex: usize| {
        let (a, b) = (nodes[vertex].1.unwrap(), nodes[vertex + 1].1.unwrap());
        grid::geo::flat_dist(a[0], a[1], b[0], b[1])
    };
    if start.fraction_to_next > 0.0 || end.fraction_to_next > 0.0 {
        let vertex = if start.fraction_to_next > 0.0 {
            start.vertex_index
        } else {
            end.vertex_index
        };
        let whole_hop = hop(vertex);
        return (whole_hop / (whole_hop / 250.0).ceil()) as f32;
    }
    (start.vertex_index..end.vertex_index).map(hop).sum::<f64>() as f32
}

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    ensure!(
        arguments.len() == 3,
        "usage: transport_sqlite_to_files SUBSET_TRANSPORT_SQLITE PREPARED_YEAR_DIR"
    );
    let output = PathBuf::from(&arguments[2]);
    let store = Connection::open_with_flags(&arguments[1], OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let spill_dir = output.with_extension("transport-conversion-spill");
    std::fs::create_dir_all(&spill_dir)?;
    let mut spill = TransportSpill::new(&spill_dir)?;

    let mut rows: BTreeMap<(String, String), Vec<Vec<String>>> = BTreeMap::new();
    let mut ways =
        store.prepare("SELECT osm_id, family, nodes_json FROM source_ways ORDER BY osm_id")?;
    let mut pieces = store.prepare(
        "SELECT segment_idx, square, start_vertex, start_fraction, end_vertex, end_fraction
         FROM source_pieces WHERE way_id = ? ORDER BY segment_idx",
    )?;
    let mut way_rows = ways.query([])?;
    while let Some(way) = way_rows.next()? {
        let (way_id, family): (i64, String) = (way.get(0)?, way.get(1)?);
        let chain: Vec<(String, Option<[f64; 2]>)> =
            serde_json::from_str(&way.get::<_, String>(2)?)?;
        let nodes: Vec<ResolvedNode> = chain
            .into_iter()
            .map(|(id, coordinates)| Ok((id.parse()?, coordinates)))
            .collect::<Result<_>>()?;
        let railway = family == "railways";
        let metres = railway.then(|| way_metres(&nodes));
        let mut squares = BTreeSet::new();
        let mut piece_rows = pieces.query([way_id])?;
        while let Some(piece) = piece_rows.next()? {
            let position = |vertex: usize, fraction: usize| -> Result<SourcePosition> {
                Ok(SourcePosition {
                    vertex_index: piece.get(vertex)?,
                    fraction_to_next: piece.get(fraction)?,
                })
            };
            let (start, end) = (position(2, 3)?, position(4, 5)?);
            let interval = SourceInterval {
                start,
                end,
                length_m: extractor_length_m(&nodes, start, end),
            };
            let square_name: String = piece.get(1)?;
            let square = grid::parse_square_name(&square_name)
                .with_context(|| format!("square {square_name}"))?;
            let geometry = interval.geometry(|vertex| nodes[vertex].1.expect("resolved node"));
            let mut prefix = Vec::new();
            write_segment_row_prefix(&mut prefix, square, way_id, piece.get(0)?, &geometry)?;
            let mut row: Vec<String> = String::from_utf8(prefix)?
                .split('\t')
                .map(str::to_owned)
                .collect();
            row.push(piece_tail(
                &nodes,
                &interval,
                metres.as_ref().map(|metres| metres.as_deref()),
            ));
            rows.entry((family.clone(), square_name))
                .or_default()
                .push(row);
            squares.insert(spill_key(square));
        }
        if railway {
            spill.write_railway_way(way_id, &nodes, &squares)?;
        }
    }
    let mut routes = store.prepare("SELECT osm_id, members_json FROM source_train_routes")?;
    let mut route_rows = routes.query([])?;
    while let Some(route) = route_rows.next()? {
        spill.write_train_route(&TrainRouteRecord {
            osm_id: route.get(0)?,
            members: serde_json::from_str(&route.get::<_, String>(1)?)?,
        })?;
    }
    spill.finish()?;
    // The world union-find is not reproducible from a subset, so the stored aliases replace its file.
    let mut aliases = store.prepare("SELECT family, node_id, canonical_node FROM node_aliases")?;
    let alias_text: Vec<String> = aliases
        .query_map([], |row| {
            Ok(format!(
                "{}\t{}\t{}\n",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    std::fs::write(spill_dir.join(NODE_ALIASES_SPILL), alias_text.concat())?;

    let aliases = load_node_aliases(&spill_dir)?;
    let no_aliases = HashMap::new();
    let alias_of = |family: &str| aliases.get(family).unwrap_or(&no_aliases);
    let (mut files, mut pieces_written, mut baked) = (0, 0, 0);
    let mut spill_tail_bytes: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for ((family, square), rows) in &rows {
        let directory = output.join(square);
        std::fs::create_dir_all(&directory)?;
        write_source_pieces(
            family,
            rows,
            alias_of(family),
            &directory.join(format!("{family}.pieces.arrow")),
        )?;
        files += 1;
        pieces_written += rows.len();
        let tail = spill_tail_bytes.entry(family).or_default();
        tail.0 += rows.len();
        // One tab plus the tail field is all a transport row adds to the spill.
        tail.1 += rows
            .iter()
            .map(|row| 1 + row[row.len() - 1].len())
            .sum::<usize>();
        baked += rows
            .iter()
            .flat_map(|row| {
                row[row.len() - 1]
                    .split(',')
                    .skip(4)
                    .take(2)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|node| alias_of(family).contains_key(&node.parse::<i64>().unwrap()))
            .count();
    }
    write_railway_ways(&spill_dir, &output, alias_of("railways"))?;
    write_train_routes(&spill_dir, &output)?;
    std::fs::remove_dir_all(&spill_dir)?;
    println!(
        "{{\"pieces_files\":{files},\"pieces\":{pieces_written},\"aliased_piece_ends\":{baked},\"spill_tail_rows_and_bytes\":\"{spill_tail_bytes:?}\"}}"
    );
    Ok(())
}
