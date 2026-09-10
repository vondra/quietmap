//! Lossless source dictionary stores stable identities once per owner and exact f32 bytes per vertex.
use crate::corner_store::{CornerEnergy, SourceEnergy, SourceIdentity};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;

pub(crate) struct SourceDictionary {
    entries: Vec<(u8, SourceIdentity)>,
    indices: HashMap<(u8, SourceIdentity), u32>,
}

impl SourceDictionary {
    pub fn load(connection: &Connection) -> Result<Self> {
        let mut entries = Vec::new();
        let mut indices = HashMap::new();
        let mut statement =
            connection.prepare("SELECT id,layer,identity FROM sources ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, layer, identity) = row?;
            ensure!(
                id as usize == entries.len() && layer < 5,
                "invalid source dictionary order or layer"
            );
            let identity = SourceIdentity(
                identity
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid source identity"))?,
            );
            ensure!(
                indices.insert((layer, identity), id).is_none(),
                "duplicate source identity"
            );
            entries.push((layer, identity));
        }
        Ok(Self { entries, indices })
    }

    pub fn encode(&mut self, connection: &Connection, energy: &CornerEnergy) -> Result<Vec<u8>> {
        validate(energy)?;
        let mut bytes = Vec::with_capacity(energy.0.len() * 16);
        for source in &energy.0 {
            let key = (source.layer, source.source);
            let id = if let Some(&id) = self.indices.get(&key) {
                id
            } else {
                let id: u32 = self.entries.len().try_into()?;
                connection.execute(
                    "INSERT INTO sources VALUES(?1,?2,?3)",
                    params![id, source.layer, source.source.0.as_slice()],
                )?;
                self.indices.insert(key, id);
                self.entries.push(key);
                id
            };
            bytes.extend_from_slice(&id.to_le_bytes());
            for value in source.periods {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        Ok(bytes)
    }

    pub fn decode(&self, bytes: &[u8]) -> Result<CornerEnergy> {
        ensure!(bytes.len().is_multiple_of(16), "truncated corner payload");
        let mut energy = Vec::with_capacity(bytes.len() / 16);
        for record in bytes.chunks_exact(16) {
            let id = u32::from_le_bytes(record[..4].try_into().unwrap());
            let &(layer, source) = self
                .entries
                .get(id as usize)
                .context("missing durable source identity")?;
            let periods = std::array::from_fn(|period| {
                f32::from_le_bytes(record[4 + period * 4..8 + period * 4].try_into().unwrap())
            });
            energy.push(SourceEnergy {
                layer,
                source,
                periods,
            });
        }
        let energy = CornerEnergy(energy);
        validate(&energy)?;
        Ok(energy)
    }
}

fn validate(energy: &CornerEnergy) -> Result<()> {
    ensure!(
        energy
            .0
            .windows(2)
            .all(|pair| (pair[0].layer, pair[0].source) < (pair[1].layer, pair[1].source)),
        "corner source identities must be unique and sorted"
    );
    ensure!(
        energy.0.iter().all(|source| source.layer < 5
            && source
                .periods
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)),
        "invalid layer or period energy"
    );
    Ok(())
}
