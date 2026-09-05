//! Typed Arrow column accessors + integer grid decoders for prepared files.
//!
//! Coordinates on disk are snapped z30 cells; these helpers turn them into
//! lon/lat floats at the read edge (one conversion per value, at load).

use arrow::array::*;
use arrow::record_batch::RecordBatch;
use grid::poly::meters_to_lonlat;
use grid::{grid_to_meters, poly};

pub fn col_i64<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int64Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_i32<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int32Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_i16<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int16Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_f64<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Float64Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_f32<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Float32Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u8<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt8Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u16<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt16Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u32<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt32Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_bool<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a BooleanArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_str<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_binary<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a BinaryArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}

/// Grid cell to lon/lat degrees.
pub fn grid_cell_lonlat(gx: i32, gy: i32) -> (f64, f64) {
    let (x, y) = grid_to_meters(gx, gy);
    meters_to_lonlat(x, y)
}

/// Decode a `geom` column value to its ring. `None` = null or truncated
/// (caller stores null geometry, same as a point). A 2-point value is a
/// wall segment, not a ring — only barrier readers use it.
pub fn decode_geom(bytes: Option<&[u8]>) -> Option<Vec<(i32, i32)>> {
    poly::decode_grid_poly(bytes?)
}

/// The lon/lat polygon of a decoded ring (matching/display geometry).
pub fn ring_lonlat(ring: &[(i32, i32)]) -> Vec<(f64, f64)> {
    ring.iter()
        .map(|&(gx, gy)| {
            let (lon, lat) = grid_cell_lonlat(gx, gy);
            (lon, lat)
        })
        .collect()
}

/// All parts and holes reach the existing obstacle kernel under one row identity.
pub fn polygons_wkb(polygons: &poly::GridPolygons) -> Vec<u8> {
    let mut wkb = vec![1];
    wkb.extend_from_slice(&6u32.to_le_bytes());
    wkb.extend_from_slice(&(polygons.len() as u32).to_le_bytes());
    for rings in polygons {
        wkb.push(1);
        wkb.extend_from_slice(&3u32.to_le_bytes());
        wkb.extend_from_slice(&(rings.len() as u32).to_le_bytes());
        for ring in rings {
            let close = ring.first() != ring.last();
            wkb.extend_from_slice(&((ring.len() + usize::from(close)) as u32).to_le_bytes());
            for &(gx, gy) in ring.iter().chain(ring.first().filter(|_| close)) {
                let (lon, lat) = grid_cell_lonlat(gx, gy);
                wkb.extend_from_slice(&lon.to_le_bytes());
                wkb.extend_from_slice(&lat.to_le_bytes());
            }
        }
    }
    wkb
}
