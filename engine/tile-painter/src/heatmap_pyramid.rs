//! SQLite lower zooms keep the canonical z13 bytes in owner artifacts and average energy only.
use crate::hm3::{decode_cells, encode_cells, Hm3Layer, COMPUTED_SILENCE, NOT_ASSESSED};
use anyhow::Result;
use grid::surface_corner::TILE_PIXEL_SIDE;
use rusqlite::{params, Connection};

pub(crate) fn tile_id(z: u8, x: u32, y: u32) -> Result<u64> {
    Ok(pmtiles::TileId::from(pmtiles::TileCoord::new(z, x, y)?).value())
}

pub(crate) fn create(connection: &Connection) -> Result<()> {
    connection.execute_batch("CREATE TABLE tiles(z INTEGER NOT NULL,x INTEGER NOT NULL,y INTEGER NOT NULL,id INTEGER NOT NULL UNIQUE,hm3 BLOB NOT NULL,PRIMARY KEY(z,x,y)) WITHOUT ROWID;")?;
    Ok(())
}

pub(crate) fn insert(connection: &Connection, z: u8, x: u32, y: u32, bytes: &[u8]) -> Result<()> {
    connection.execute(
        "INSERT INTO tiles VALUES(?1,?2,?3,?4,?5)",
        params![z, x, y, tile_id(z, x, y)?, bytes],
    )?;
    Ok(())
}

/// Fold one z(n+1) child tile into its quadrant of the z(n) parent canvas:
/// the energy mean of the assessed source pixels among the four, silence (254)
/// counting as zero energy and not-assessed (255) left out; a parent is
/// not assessed only when all four children are. Building pixels enter with
/// their façade exposure like any other level. Cells are 0.5 dB quanta
/// (byte = 2·dB), so the mean's byte is 20·log10(mean energy).
pub(crate) fn downsample_child(
    bytes: &[u8],
    layer: Hm3Layer,
    destination: &mut [u8],
    quadrant_x: usize,
    quadrant_y: usize,
) -> Result<()> {
    let source = decode_cells(bytes, layer)?;
    let side = TILE_PIXEL_SIDE;
    let half = side / 2;
    let cell_energy: [f64; 256] = std::array::from_fn(|quantum| match quantum as u8 {
        COMPUTED_SILENCE | NOT_ASSESSED => 0.0,
        level => (f64::from(level) * 0.05 * std::f64::consts::LN_10).exp(),
    });
    for y in 0..half {
        for x in 0..half {
            let start = y * 2 * side + x * 2;
            let children = [start, start + 1, start + side, start + side + 1].map(|i| source[i]);
            destination[(quadrant_y * half + y) * side + quadrant_x * half + x] =
                parent_cell(children, &cell_energy);
        }
    }
    Ok(())
}

fn parent_cell(children: [u8; 4], cell_energy: &[f64; 256]) -> u8 {
    let assessed: Vec<u8> = children
        .into_iter()
        .filter(|cell| *cell != NOT_ASSESSED)
        .collect();
    if assessed.is_empty() {
        return NOT_ASSESSED;
    }
    let mean = assessed
        .iter()
        .map(|cell| cell_energy[usize::from(*cell)])
        .sum::<f64>()
        / assessed.len() as f64;
    let byte = (20.0 * mean.log10()).round();
    if byte >= 0.0 {
        byte.min(253.0) as u8
    } else {
        COMPUTED_SILENCE
    }
}

/// Build z11 down to z2 from the z12 parents the owners seeded; a covered
/// owner always has all four children of every parent, so an absent quadrant
/// is outside painted coverage and stays not assessed. One transaction per zoom bounds writer memory.
pub(crate) fn build(connection: &mut Connection, layer: Hm3Layer) -> Result<()> {
    for zoom in (2_u8..12).rev() {
        let transaction = connection.transaction()?;
        let mut parents = transaction
            .prepare("SELECT DISTINCT x/2,y/2 FROM tiles WHERE z=?1 ORDER BY x/2,y/2")?;
        let mut rows = parents.query([zoom + 1])?;
        let mut children = transaction
            .prepare("SELECT x,y,hm3 FROM tiles WHERE z=?1 AND x IN (?2,?3) AND y IN (?4,?5)")?;
        while let Some(row) = rows.next()? {
            let (x, y): (u32, u32) = (row.get(0)?, row.get(1)?);
            let mut pixels = vec![NOT_ASSESSED; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE];
            let mut child_rows =
                children.query(params![zoom + 1, x * 2, x * 2 + 1, y * 2, y * 2 + 1])?;
            while let Some(child) = child_rows.next()? {
                let cx: u32 = child.get(0)?;
                let cy: u32 = child.get(1)?;
                downsample_child(
                    &child.get::<_, Vec<u8>>(2)?,
                    layer,
                    &mut pixels,
                    (cx % 2) as usize,
                    (cy % 2) as usize,
                )?;
            }
            insert(
                &transaction,
                zoom,
                x,
                y,
                encode_cells(&pixels, layer)?.as_bytes(),
            )?;
        }
        drop(rows);
        drop(parents);
        drop(children);
        transaction.commit()?;
        eprintln!("{} pyramid z{zoom} complete", layer.name());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_energy() -> [f64; 256] {
        std::array::from_fn(|quantum| match quantum as u8 {
            COMPUTED_SILENCE | NOT_ASSESSED => 0.0,
            level => (f64::from(level) * 0.05 * std::f64::consts::LN_10).exp(),
        })
    }

    /// Silence is zero energy, not-assessed is no sample (#38, #2).
    #[test]
    fn a_parent_averages_assessed_children_and_is_unassessed_only_when_all_are() {
        let energy = cell_energy();
        let (s, n) = (COMPUTED_SILENCE, NOT_ASSESSED);
        assert_eq!(parent_cell([s, n, n, n], &energy), s);
        assert_eq!(parent_cell([n; 4], &energy), n);
        assert_eq!(parent_cell([120, s, s, s], &energy), 108, "60 dB among quiet: 60 − 6.02");
        assert_eq!(parent_cell([120, n, n, n], &energy), 120, "unassessed children are left out");
        assert_eq!(parent_cell([80, n, s, n], &energy), 74, "40 dB beside silence: 40 − 3.01");
        assert_eq!(parent_cell([0, s, s, s], &energy), s, "a mean below 0 dB is quiet");
        assert_eq!(parent_cell([253; 4], &energy), 253);
    }
}
