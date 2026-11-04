//! Original road endpoint identities distinguish OSM nodes from exact fractional cuts of a way.

use crate::dev4::{Square, column, read_table, require_stamp};
use anyhow::{Result, anyhow, ensure};
use arrow_array::{Array, Float64Array, Int16Array, Int32Array, Int64Array, UInt16Array};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum Endpoint {
    Node(i64),
    Cut(i64, u16, u64),
}

#[derive(Clone, Copy)]
pub struct End {
    pub identity: Endpoint,
    pub home: Square,
}

pub fn squares(root: &Path) -> Result<BTreeMap<Square, PathBuf>> {
    let mut found = BTreeMap::new();
    for x in std::fs::read_dir(root.join("z9"))? {
        let x = x?;
        if !x.file_type()?.is_dir() {
            continue;
        }
        let x_axis: u32 = x
            .file_name()
            .to_str()
            .ok_or_else(|| anyhow!("invalid square name"))?
            .parse()?;
        for y in std::fs::read_dir(x.path())? {
            let y = y?;
            if !y.file_type()?.is_dir() {
                continue;
            }
            let y_axis: u32 = y
                .file_name()
                .to_str()
                .ok_or_else(|| anyhow!("invalid square name"))?
                .parse()?;
            ensure!(x_axis < 512 && y_axis < 512, "invalid road owner");
            let road = y.path().join("roads.arrow");
            if road.exists() {
                found.insert(
                    Square {
                        x: x_axis,
                        y: y_axis,
                    },
                    road,
                );
            }
        }
    }
    Ok(found)
}

pub fn read(path: &Path) -> Result<BTreeMap<(i64, i16), [End; 2]>> {
    let table = read_table(&path.with_file_name("roads.pieces.arrow"))
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow!("missing original road topology: {}", path.display()))?;
    require_stamp(&table, "transport_pieces_contract", "1").map_err(anyhow::Error::msg)?;
    let mut result = BTreeMap::new();
    for batch in table.batches {
        macro_rules! c {
            ($ty:ident,$name:expr) => {{
                let values = column::<$ty>(&batch, $name).map_err(anyhow::Error::msg)?;
                ensure!(
                    values.null_count() == 0,
                    "null source topology column {}",
                    $name
                );
                values
            }};
        }
        let ways = c!(Int64Array, "way_id");
        let segments = c!(Int16Array, "segment_idx");
        let nodes = [c!(Int64Array, "start_node"), c!(Int64Array, "end_node")];
        let vertices = [
            c!(UInt16Array, "start_vertex"),
            c!(UInt16Array, "end_vertex"),
        ];
        let fractions = [
            c!(Float64Array, "start_fraction"),
            c!(Float64Array, "end_fraction"),
        ];
        let eastings = [c!(Int32Array, "start_gx"), c!(Int32Array, "end_gx")];
        let northings = [c!(Int32Array, "start_gy"), c!(Int32Array, "end_gy")];
        for row in 0..batch.num_rows() {
            let mut ends = [End {
                identity: Endpoint::Node(0),
                home: Square { x: 0, y: 0 },
            }; 2];
            for side in 0..2 {
                let fraction = fractions[side].value(row);
                ensure!(
                    fraction.is_finite() && (0.0..1.0).contains(&fraction),
                    "invalid fractional road endpoint"
                );
                ensure!(nodes[side].value(row) != 0, "missing original OSM node");
                ends[side] = End {
                    identity: if fraction > 0.0 {
                        Endpoint::Cut(
                            ways.value(row),
                            vertices[side].value(row),
                            fraction.to_bits(),
                        )
                    } else {
                        Endpoint::Node(nodes[side].value(row))
                    },
                    home: Square {
                        x: (eastings[side].value(row).rem_euclid(1 << 30) >> 21) as u32,
                        y: (511 - (northings[side].value(row) >> 21)).clamp(0, 511) as u32,
                    },
                };
            }
            ensure!(
                result
                    .insert((ways.value(row), segments.value(row)), ends)
                    .is_none(),
                "duplicate source road piece"
            );
        }
    }
    Ok(result)
}
