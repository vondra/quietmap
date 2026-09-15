//! SQLite lower zooms keep the canonical z13 bytes in owner artifacts and average energy only.
use crate::hm3::{decode_cells, encode_cells, Hm3Layer};
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
/// mean energy of the four source pixels, silent (255) counting as zero and
/// the division by four always applied. Cells are 0.5 dB quanta (byte = 2·dB),
/// so the mean's byte is 20·log10(energy/4) — no dB detour needed.
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
    let cell_energy: [f64; 256] = std::array::from_fn(|quantum| {
        if quantum == 255 {
            0.0
        } else {
            (quantum as f64 * 0.05 * std::f64::consts::LN_10).exp()
        }
    });
    for y in 0..half {
        for x in 0..half {
            let start = y * 2 * side + x * 2;
            let energy = [start, start + 1, start + side, start + side + 1]
                .into_iter()
                .map(|index| cell_energy[source[index] as usize])
                .sum::<f64>();
            if energy > 0.0 {
                destination[(quadrant_y * half + y) * side + quadrant_x * half + x] =
                    (20.0 * (energy / 4.0).log10()).round().clamp(0.0, 254.0) as u8;
            }
        }
    }
    Ok(())
}

/// Build z11 down to z2 from the z12 parents the owners seeded; a covered
/// owner always has all four children of every parent, so an absent quadrant
/// can only stay silent. One transaction per zoom bounds writer memory.
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
            let mut pixels = vec![255; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE];
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
