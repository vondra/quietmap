//! Inspect uncompressed IPC allocation facts without loading record-batch bodies.

use anyhow::{ensure, Context, Result};
use arrow::ipc::{reader::read_footer_length, root_as_footer, root_as_message};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct IpcAllocation {
    pub rows: usize,
    pub file_bytes: u64,
    pub largest_batch_bytes: u64,
    pub batches: usize,
}

pub(crate) fn inspect_ipc_allocation(path: &Path) -> Result<IpcAllocation> {
    let mut file = File::open(path)?;
    let file_bytes = file.metadata()?.len();
    ensure!(file_bytes >= 10, "short IPC file");
    file.seek(SeekFrom::End(-10))?;
    let mut trailer = [0; 10];
    file.read_exact(&mut trailer)?;
    let footer_length = read_footer_length(trailer)?;
    ensure!(
        footer_length as u64 <= file_bytes - 10,
        "invalid IPC footer length"
    );
    let footer_start = file_bytes - 10 - footer_length as u64;
    file.seek(SeekFrom::Start(footer_start))?;
    let mut bytes = vec![0; footer_length];
    file.read_exact(&mut bytes)?;
    let footer = root_as_footer(&bytes).map_err(|error| anyhow::anyhow!("IPC footer: {error}"))?;
    ensure!(
        footer.dictionaries().is_none_or(|blocks| blocks.is_empty()),
        "aircraft IPC dictionaries are not supported by allocation accounting"
    );
    let mut result = IpcAllocation {
        file_bytes,
        ..Default::default()
    };
    let mut previous_end = 8;
    for block in footer.recordBatches().into_iter().flatten() {
        let offset = u64::try_from(block.offset())?;
        let metadata_bytes = usize::try_from(block.metaDataLength())?;
        let body_bytes = u64::try_from(block.bodyLength())?;
        ensure!(
            metadata_bytes >= 4
                && offset >= previous_end
                && offset
                    .checked_add(metadata_bytes as u64)
                    .and_then(|value| value.checked_add(body_bytes))
                    .is_some_and(|end| end <= footer_start),
            "invalid IPC block bounds"
        );
        previous_end = offset + metadata_bytes as u64 + body_bytes;
        file.seek(SeekFrom::Start(offset))?;
        let mut metadata = vec![0; metadata_bytes];
        file.read_exact(&mut metadata)?;
        let prefix = if metadata[..4] == [255; 4] { 8 } else { 4 };
        let message = root_as_message(metadata.get(prefix..).context("short IPC message")?)
            .map_err(|error| anyhow::anyhow!("IPC message: {error}"))?;
        ensure!(
            u64::try_from(message.bodyLength())? == body_bytes,
            "IPC message and footer body lengths differ"
        );
        let batch = message
            .header_as_record_batch()
            .context("expected IPC record batch")?;
        ensure!(
            batch.compression().is_none(),
            "compressed aircraft IPC requires decoded-size accounting"
        );
        result.rows = result
            .rows
            .checked_add(usize::try_from(batch.length())?)
            .context("IPC row count overflow")?;
        result.batches += 1;
        result.largest_batch_bytes = result
            .largest_batch_bytes
            .max(body_bytes + metadata_bytes as u64);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::{read_record_batches, write_segments};

    #[test]
    fn metadata_counts_match_actual_batches_and_reject_truncated_bodies() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("empty.arrow");
        write_segments(&path, &[]).unwrap();
        let facts = inspect_ipc_allocation(&path).unwrap();
        assert_eq!(
            facts.rows,
            read_record_batches(&path)
                .unwrap()
                .1
                .iter()
                .map(|batch| batch.num_rows())
                .sum::<usize>()
        );
        assert_eq!(facts.file_bytes, path.metadata().unwrap().len());
        std::fs::write(&path, b"ARROW1").unwrap();
        assert!(inspect_ipc_allocation(&path).is_err());
    }

    #[test]
    fn intact_footer_cannot_hide_a_truncated_record_body() {
        use arrow::{
            array::{ArrayRef, UInt64Array},
            ipc::writer::FileWriter,
            record_batch::RecordBatch,
        };
        use std::sync::Arc;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("batch.arrow");
        let batch = RecordBatch::try_from_iter([(
            "value",
            Arc::new(UInt64Array::from(vec![42; 128])) as ArrayRef,
        )])
        .unwrap();
        let mut writer =
            FileWriter::try_new(File::create(&path).unwrap(), &batch.schema()).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        drop(writer);
        assert_eq!(inspect_ipc_allocation(&path).unwrap().rows, 128);
        let mut bytes = std::fs::read(&path).unwrap();
        let footer = bytes.len()
            - 10
            - read_footer_length(bytes[bytes.len() - 10..].try_into().unwrap()).unwrap();
        bytes.drain(footer - 64..footer);
        std::fs::write(&path, bytes).unwrap();
        assert!(inspect_ipc_allocation(&path)
            .unwrap_err()
            .to_string()
            .contains("block bounds"));
    }
}
