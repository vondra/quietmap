//! Stage eight PMTiles archives from acknowledged owner outputs; publication remains the caller's job.
use crate::{
    generation_receipt::{file_digest, hex_digest},
    heatmap_authority::{completed_owners, OwnerArtifact},
    heatmap_pyramid as pyramid,
    hm3::{encode_cells, Hm3Layer, ALL_LAYERS},
};
use anyhow::{ensure, Result};
use grid::surface_corner::TILE_PIXEL_SIDE;
use grid::Z13_PER_Z9_SIDE;
use pmtiles::{Compression, Compressor, PmTilesWriter, TileCoord, TileType};
use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};

/// z12 is the first averaged level: half the z13 resolution per owner side.
const Z12_PER_Z9_SIDE: u32 = Z13_PER_Z9_SIDE / 2;

struct BrotliPassthrough;
impl Compressor for BrotliPassthrough {
    fn compression(&self) -> Compression {
        Compression::Brotli
    }
    fn compress(
        &self,
        write: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
        output: &mut dyn Write,
    ) -> pmtiles::PmtResult<()> {
        write(output)?;
        Ok(())
    }
}

fn read_tile(connection: &Connection, layer: usize, x: u32, y: u32) -> Result<Vec<u8>> {
    let (bytes, expected): (Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT hm3,sha256 FROM surface_tiles WHERE x=?1 AND y=?2 AND layer=?3",
        params![x, y, layer],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        Sha256::digest(&bytes).as_slice() == expected,
        "owner HM3 checksum changed"
    );
    Ok(bytes)
}

fn build_owner_parents(
    pyramid: &mut Connection,
    artifact: &OwnerArtifact,
    layer_index: usize,
    layer: Hm3Layer,
) -> Result<()> {
    let owner = Connection::open_with_flags(&artifact.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let transaction = pyramid.transaction()?;
    for y in u32::from(artifact.owner.y) * Z12_PER_Z9_SIDE
        ..u32::from(artifact.owner.y) * Z12_PER_Z9_SIDE + Z12_PER_Z9_SIDE
    {
        for x in u32::from(artifact.owner.x) * Z12_PER_Z9_SIDE
            ..u32::from(artifact.owner.x) * Z12_PER_Z9_SIDE + Z12_PER_Z9_SIDE
        {
            let mut cells = vec![255; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE];
            for qy in 0..2 {
                for qx in 0..2 {
                    let bytes = read_tile(&owner, layer_index, x * 2 + qx, y * 2 + qy)?;
                    pyramid::downsample_child(&bytes, layer, &mut cells, qx as usize, qy as usize)?;
                }
            }
            pyramid::insert(
                &transaction,
                12,
                x,
                y,
                encode_cells(&cells, layer)?.as_bytes(),
            )?;
        }
    }
    transaction.commit()?;
    Ok(())
}

fn pack_layer(
    owners: &[OwnerArtifact],
    pyramid: &Connection,
    output: &Path,
    build: &str,
    layer_index: usize,
    layer: Hm3Layer,
) -> Result<serde_json::Value> {
    let name = format!("{}.{build}.pmtiles", layer.name());
    let path = output.join(&name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut writer = PmTilesWriter::new(TileType::Unknown)
        .tile_codec(BrotliPassthrough)
        .internal_compression(Compression::Gzip)
        .min_zoom(2)
        .max_zoom(13)
        .bounds(-180.0, -85.051_13, 180.0, 85.051_13)
        .center(0.0, 0.0)
        .center_zoom(2)
        .metadata(
            &serde_json::json!({"name":layer.name(),"build":build,"source_id":layer.source_id()})
                .to_string(),
        )
        .create(file)?;
    let mut query = pyramid.prepare("SELECT z,x,y,hm3 FROM tiles ORDER BY id")?;
    let mut rows = query.query([])?;
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        writer.add_raw_tile(
            TileCoord::new(row.get(0)?, row.get(1)?, row.get(2)?)?,
            &row.get::<_, Vec<u8>>(3)?,
        )?;
        count += 1;
    }
    for artifact in owners {
        let connection =
            Connection::open_with_flags(&artifact.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut coordinates = Vec::with_capacity((Z13_PER_Z9_SIDE * Z13_PER_Z9_SIDE) as usize);
        for y in u32::from(artifact.owner.y) * Z13_PER_Z9_SIDE
            ..u32::from(artifact.owner.y) * Z13_PER_Z9_SIDE + Z13_PER_Z9_SIDE
        {
            for x in u32::from(artifact.owner.x) * Z13_PER_Z9_SIDE
                ..u32::from(artifact.owner.x) * Z13_PER_Z9_SIDE + Z13_PER_Z9_SIDE
            {
                coordinates.push((pyramid::tile_id(13, x, y)?, x, y));
            }
        }
        coordinates.sort_unstable();
        for (_id, x, y) in coordinates {
            writer.add_raw_tile(
                TileCoord::new(13, x, y)?,
                &read_tile(&connection, layer_index, x, y)?,
            )?;
            count += 1;
        }
    }
    writer.finalize()?;
    let file = File::open(&path)?;
    file.sync_all()?;
    let bytes = file.metadata()?.len();
    let sha = hex_digest(file_digest(&path)?);
    eprintln!("{}: {count} tiles, {bytes} bytes", layer.name());
    Ok(serde_json::json!({"file":name,"bytes":bytes,"sha256":sha}))
}

pub fn pack(authority: &Path, generation: &str, output: &Path, build: &str) -> Result<()> {
    ensure!(
        build.starts_with('b') && build.len() > 1 && build[1..].bytes().all(|c| c.is_ascii_digit()),
        "build must be b followed by digits"
    );
    let (digest, owners) = completed_owners(authority, generation)?;
    // Fresh staging directory: failures never replace an archive or a current manifest.
    std::fs::create_dir(output)?;
    let mut layers = serde_json::Map::new();
    for (index, layer) in ALL_LAYERS.into_iter().enumerate() {
        let scratch = output.join("pyramid.sqlite");
        let mut database = Connection::open(&scratch)?;
        pyramid::create(&database)?;
        for (i, owner) in owners.iter().enumerate() {
            build_owner_parents(&mut database, owner, index, layer)?;
            if (i + 1) % 1000 == 0 || i + 1 == owners.len() {
                eprintln!("{} z12: {}/{} owners", layer.name(), i + 1, owners.len());
            }
        }
        pyramid::build(&mut database, layer)?;
        layers.insert(
            layer.name().to_string(),
            pack_layer(&owners, &database, output, build, index, layer)?,
        );
        drop(database);
        std::fs::remove_file(scratch)?;
    }
    // Source bytes are immutable authority outputs; reject any mutation during packing.
    for owner in &owners {
        owner.verify(digest)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("manifest.json"))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({"build":build,"zoom":13,"generation":generation,"layers":layers}),
    )?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    crate::durable_directory::sync_directory(output)?;
    crate::durable_directory::sync_directory(output.parent().unwrap_or(Path::new(".")))?;
    Ok(())
}

#[cfg(test)]
#[path = "heatmap_pack_tests.rs"]
mod tests;
