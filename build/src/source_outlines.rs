//! Mapped industrial boundaries, indexed by the source's id for display when its popup row opens.

use crate::dev4::{Dev4, Square, z30_corner_degrees};
use crate::sources::industry::read_rows;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::Path;

/// The complete input ring as GeoJSON, closed and continuous across the antimeridian.
fn geometry(ring: &[(i32, i32)]) -> Option<Value> {
    let mut coordinates: Vec<[f64; 2]> = ring
        .iter()
        .map(|&(x, y)| {
            let (lat, lon) = z30_corner_degrees(x, y);
            [lon, lat]
        })
        .collect();
    coordinates.dedup();
    let first = *coordinates.first()?;
    if coordinates.last() != Some(&first) {
        coordinates.push(first);
    }
    if coordinates.len() < 4 {
        return None;
    }
    for point in &mut coordinates {
        point[0] += ((first[0] - point[0]) / 360.0).round() * 360.0;
    }
    Some(json!({"type": "Polygon", "coordinates": [coordinates]}))
}

/// One display database per year, made from the same industrial input as the sound sources.
/// Repeated copies of a ring are kept once; distinct parts of one source keep their whole rings.
pub fn build(dev4: &Dev4, squares: &[Square], out: &Path) -> Result<(), String> {
    if !dev4.prepared.join("z9").is_dir() {
        return Err("source outlines: prepared input has no z9 directory".into());
    }
    let partial = out.with_extension("sqlite.partial");
    for path in [out, partial.as_path()] {
        if path.exists() {
            return Err(format!("outline output already exists: {}", path.display()));
        }
    }
    let mut database = Connection::open(&partial).map_err(|error| error.to_string())?;
    database
        .execute_batch(
            // Larger pages keep common rings inline instead of in mostly empty overflow pages.
            "PRAGMA page_size=16384;
             CREATE TABLE outlines (id TEXT NOT NULL, geometry TEXT NOT NULL,
                        PRIMARY KEY (id, geometry)) WITHOUT ROWID;",
        )
        .map_err(|error| error.to_string())?;
    let transaction = database.transaction().map_err(|error| error.to_string())?;
    let mut read = 0usize;
    let mut written = 0usize;
    {
        let mut insert = transaction
            .prepare("INSERT OR IGNORE INTO outlines (id, geometry) VALUES (?1, ?2)")
            .map_err(|error| error.to_string())?;
        for &square in squares {
            let Some(table) = dev4.table(square, "industrial.arrow")? else {
                continue;
            };
            let mut rows = Vec::new();
            read_rows(&table, &mut rows)
                .map_err(|error| format!("industrial.arrow of {square:?}: {error}"))?;
            read += rows.len();
            for row in rows {
                if let Some(geometry) = geometry(&row.ring) {
                    written += insert
                        .execute([format!("{:016x}", row.group_key()), geometry.to_string()])
                        .map_err(|error| error.to_string())?;
                }
            }
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    database.close().map_err(|(_, error)| error.to_string())?;
    std::fs::rename(&partial, out).map_err(|error| error.to_string())?;
    eprintln!("source outlines: {read} input rows, {written} distinct rings");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev4::degrees_to_z30;

    #[test]
    fn mapped_rings_close_without_spanning_the_world_or_inventing_an_area() {
        let ring: Vec<_> = [(10.0, 179.99), (10.01, -179.99), (10.0, -179.99)]
            .into_iter()
            .map(|(lat, lon)| degrees_to_z30(lat, lon))
            .collect();
        let polygon = geometry(&ring).unwrap();
        let points = polygon["coordinates"][0].as_array().unwrap();
        assert_eq!(points.len(), 4);
        assert_eq!(points.first(), points.last());
        for point in points {
            assert!((point[0].as_f64().unwrap() - 180.0).abs() < 0.02);
            assert!((point[1].as_f64().unwrap() - 10.0).abs() < 0.02);
        }
        assert!(geometry(&[]).is_none());
        assert!(geometry(&ring[..2]).is_none());
        assert!(geometry(&[ring[0]; 4]).is_none());
    }
}
