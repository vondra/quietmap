//! Published source/code pins let a popup verify a different producer executable.
use crate::corner_store::CornerGeneration;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};
include!(concat!(env!("OUT_DIR"), "/surface_code.rs"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationReceipt {
    pub sources: [u8; 32],
    pub code: [u8; 32],
    pub producer: [u8; 32],
}
impl GenerationReceipt {
    pub fn generation(self) -> CornerGeneration {
        CornerGeneration::from_manifests(self.sources, self.code, self.producer)
    }
    pub fn publish(self, root: &Path) -> Result<()> {
        ensure!(
            self.code == SURFACE_CODE_DIGEST,
            "producer code receipt mismatch"
        );
        crate::durable_directory::create_dir_all(root)?;
        let mut connection = Connection::open(root.join("generation.sqlite"))?;
        connection.busy_timeout(std::time::Duration::from_secs(60))?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS surface_generation(id INTEGER PRIMARY KEY CHECK(id=1),
            sources BLOB NOT NULL,code BLOB NOT NULL,producer BLOB NOT NULL);")?;
        tx.execute(
            "INSERT OR IGNORE INTO surface_generation VALUES(1,?1,?2,?3)",
            params![
                self.sources.as_slice(),
                self.code.as_slice(),
                self.producer.as_slice()
            ],
        )?;
        ensure!(
            read_receipt(&tx)? == self,
            "corner publication mixes generations"
        );
        tx.commit()?;
        crate::durable_directory::sync_directory(root)?;
        Ok(())
    }
    pub fn read_compatible(root: &Path, sources: [u8; 32]) -> Result<Option<Self>> {
        let path = root.join("generation.sqlite");
        if !path.try_exists()? {
            return Ok(None);
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let receipt = read_receipt(&connection)?;
        ensure!(
            receipt.sources == sources && receipt.code == SURFACE_CODE_DIGEST,
            "stored corners do not match current sources and shared physics"
        );
        Ok(Some(receipt))
    }
}
fn read_receipt(connection: &Connection) -> Result<GenerationReceipt> {
    let values: [Vec<u8>; 3] = connection.query_row(
        "SELECT sources,code,producer FROM surface_generation WHERE id=1",
        [],
        |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?]),
    )?;
    let mut digests = [[0; 32]; 3];
    for (digest, value) in digests.iter_mut().zip(values) {
        *digest = value
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid generation receipt digest"))?;
    }
    Ok(GenerationReceipt {
        sources: digests[0],
        code: digests[1],
        producer: digests[2],
    })
}

pub fn file_digest(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reader_accepts_producer_receipt_only_with_current_code_and_external_pins() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("corners");
        assert!(GenerationReceipt::read_compatible(&root, [1; 32])
            .unwrap()
            .is_none());
        assert!(!root.exists());
        let receipt = GenerationReceipt {
            sources: [1; 32],
            code: SURFACE_CODE_DIGEST,
            producer: [4; 32],
        };
        receipt.publish(&root).unwrap();
        assert_eq!(
            GenerationReceipt::read_compatible(&root, [1; 32]).unwrap(),
            Some(receipt)
        );
        assert!(GenerationReceipt::read_compatible(&root, [5; 32]).is_err());
        assert!(GenerationReceipt {
            producer: [5; 32],
            ..receipt
        }
        .publish(&root)
        .is_err());
        let connection = Connection::open(root.join("generation.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE surface_generation SET code=?1",
                [[0u8; 32].as_slice()],
            )
            .unwrap();
        assert!(GenerationReceipt::read_compatible(&root, [1; 32]).is_err());
    }
}
