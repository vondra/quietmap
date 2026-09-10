//! One prepared z9 square's source files, opened lazily via mmap.
//!
//! The `qm_blocks` batch envelopes prune bodies outside the click's reach.
//! Only absent optional files are empty; opening or decoding an existing file
//! fails the query on error. Source contracts reject stale coordinate and
//! layer semantics.
//!
//! Batches decode through `FileDecoder` over a `Buffer` that owns the mapping,
//! so every decoded array is a slice of the file-backed pages the kernel can
//! drop under pressure — not anonymous heap the cached batch holds for the
//! process's life. `FileDecoder` keeps arrow's default `require_alignment =
//! false`: a buffer the mapping happens to misalign is copied and realigned
//! rather than dropped, because a dropped batch would silently hide sources
//! while a copied one only costs memory.

use arrow::buffer::Buffer;
use arrow::datatypes::DataType;
use arrow::error::ArrowError;
use arrow::ipc::convert::fb_to_schema;
use arrow::ipc::reader::{read_footer_length, FileDecoder, FileReader};
use arrow::ipc::{root_as_footer, Block};
use arrow::record_batch::RecordBatch;
use grid::Square;
use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};

/// One arrow file, opened (footer + schema only) but not decoded.
pub struct LazyArrow {
    buffer: Option<Buffer>,
    decoder: Option<FileDecoder>,
    schema: Option<arrow::datatypes::SchemaRef>,
    blocks: Option<Vec<arrow_batching::Block>>,
    batches: Vec<Block>,
    body_end: usize,
    slots: Vec<OnceLock<Result<RecordBatch, String>>>,
    path: PathBuf,
}

fn invalid_ipc(message: impl Into<String>) -> ArrowError {
    ArrowError::ParseError(message.into())
}

const ARROW_MAGIC: &[u8; 6] = b"ARROW1";
const FILE_HEADER_LEN: usize = 8;
const CONTINUATION_MARKER: [u8; 4] = [0xff; 4];

fn block_buffer(buffer: &Buffer, block: &Block, body_end: usize) -> Result<Buffer, ArrowError> {
    let offset = usize::try_from(block.offset())
        .map_err(|_| invalid_ipc(format!("negative IPC block offset {}", block.offset())))?;
    let metadata_len = usize::try_from(block.metaDataLength()).map_err(|_| {
        invalid_ipc(format!(
            "negative IPC metadata length {}",
            block.metaDataLength()
        ))
    })?;
    let body_len = usize::try_from(block.bodyLength())
        .map_err(|_| invalid_ipc(format!("negative IPC body length {}", block.bodyLength())))?;
    let block_len = metadata_len
        .checked_add(body_len)
        .ok_or_else(|| invalid_ipc("IPC block length overflow"))?;
    let end = offset
        .checked_add(block_len)
        .ok_or_else(|| invalid_ipc("IPC block end overflow"))?;
    if offset < FILE_HEADER_LEN || end > body_end {
        return Err(invalid_ipc(format!(
            "IPC block {offset}..{end} is outside file body {FILE_HEADER_LEN}..{body_end}"
        )));
    }
    if metadata_len < 4 {
        return Err(invalid_ipc(format!(
            "IPC block metadata length {metadata_len} is shorter than its prefix"
        )));
    }
    let metadata = &buffer[offset..offset + metadata_len];
    let (prefix_len, declared_len_bytes): (usize, &[u8]) = if metadata[..4] == CONTINUATION_MARKER {
        if metadata_len < 8 {
            return Err(invalid_ipc(
                "IPC continuation marker has no 4-byte message length",
            ));
        }
        (8, &metadata[4..8])
    } else {
        (4, &metadata[..4])
    };
    let declared_len = i32::from_le_bytes(declared_len_bytes.try_into().unwrap());
    let declared_len = usize::try_from(declared_len)
        .map_err(|_| invalid_ipc(format!("negative IPC message length {declared_len}")))?;
    if declared_len == 0 {
        return Err(invalid_ipc(
            "IPC file block contains an end-of-stream marker",
        ));
    }
    let declared_end = prefix_len
        .checked_add(declared_len)
        .ok_or_else(|| invalid_ipc("IPC message length overflow"))?;
    if declared_end > metadata_len {
        return Err(invalid_ipc(format!(
            "IPC message prefix declares {declared_len} bytes inside {metadata_len}-byte metadata"
        )));
    }
    Ok(buffer.slice_with_length(offset, block_len))
}

fn decode_file(
    buffer: &Buffer,
) -> Result<(arrow::datatypes::SchemaRef, FileDecoder, Vec<Block>, usize), ArrowError> {
    let trailer_start = buffer
        .len()
        .checked_sub(10)
        .ok_or_else(|| invalid_ipc("Arrow IPC file is shorter than its 10-byte trailer"))?;
    if buffer.len() < FILE_HEADER_LEN || &buffer[..ARROW_MAGIC.len()] != ARROW_MAGIC {
        return Err(invalid_ipc("Arrow IPC file has no leading ARROW1 magic"));
    }
    let trailer: [u8; 10] = buffer[trailer_start..]
        .try_into()
        .map_err(|_| invalid_ipc("invalid Arrow IPC trailer"))?;
    let footer_len = read_footer_length(trailer)?;
    let footer_start = trailer_start.checked_sub(footer_len).ok_or_else(|| {
        invalid_ipc(format!(
            "Arrow IPC footer length {footer_len} exceeds file length {}",
            buffer.len()
        ))
    })?;
    if footer_start < FILE_HEADER_LEN {
        return Err(invalid_ipc("Arrow IPC footer overlaps the file header"));
    }
    let footer = root_as_footer(&buffer[footer_start..trailer_start])
        .map_err(|error| invalid_ipc(format!("invalid Arrow IPC footer: {error}")))?;
    let ipc_schema = footer
        .schema()
        .ok_or_else(|| invalid_ipc("Arrow IPC footer has no schema"))?;
    if !ipc_schema.endianness().equals_to_target_endianness() {
        return Err(ArrowError::IpcError(
            "Arrow IPC source endianness does not match this system".into(),
        ));
    }
    let schema = Arc::new(fb_to_schema(ipc_schema));
    let mut decoder = FileDecoder::new(Arc::clone(&schema), footer.version());
    for block in footer.dictionaries().iter().flatten() {
        let data = block_buffer(buffer, block, footer_start)?;
        decoder.read_dictionary(block, &data)?;
    }
    let batches: Vec<Block> = footer
        .recordBatches()
        .ok_or_else(|| invalid_ipc("Arrow IPC footer has no record-batch vector"))?
        .iter()
        .copied()
        .collect();
    for block in &batches {
        block_buffer(buffer, block, footer_start)?;
    }
    Ok((schema, decoder, batches, footer_start))
}

impl LazyArrow {
    pub fn empty() -> Self {
        LazyArrow {
            buffer: None,
            decoder: None,
            schema: None,
            blocks: None,
            batches: Vec::new(),
            body_end: 0,
            slots: Vec::new(),
            path: PathBuf::new(),
        }
    }

    /// Open `path`: mmap + IPC footer + schema. No record-batch bodies decode here.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && path
                        .symlink_metadata()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(Self::empty())
            }
            Err(error) => return Err(format!("failed to open {}: {error}", path.display())),
        };
        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|error| format!("failed to mmap {}: {error}", path.display()))?;
        let pointer = NonNull::new(mmap.as_ptr() as *mut u8)
            .ok_or_else(|| format!("failed to mmap {}: null base pointer", path.display()))?;
        // The Buffer owner keeps the mmap alive for every sliced Arrow array,
        // even after this LazyArrow and its cache entry are dropped.
        let buffer = unsafe { Buffer::from_custom_allocation(pointer, mmap.len(), Arc::new(mmap)) };
        let (schema, decoder, batches, body_end) = decode_file(&buffer)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let num_batches = batches.len();
        let blocks = schema
            .metadata()
            .get(arrow_batching::QM_BLOCKS_KEY)
            .and_then(|v| arrow_batching::parse_blocks(v))
            .filter(|b| b.len() == num_batches);
        Ok(LazyArrow {
            buffer: Some(buffer),
            decoder: Some(decoder),
            schema: Some(schema),
            blocks,
            batches,
            body_end,
            slots: (0..num_batches).map(|_| OnceLock::new()).collect(),
            path: path.to_path_buf(),
        })
    }

    /// File-level schema (None only for an absent optional file).
    pub fn schema(&self) -> Option<&arrow::datatypes::SchemaRef> {
        self.schema.as_ref()
    }

    fn batch(&self, i: usize) -> Result<&RecordBatch, String> {
        self.slots[i]
            .get_or_init(|| {
                let decode = || -> Result<RecordBatch, ArrowError> {
                    let buffer = self
                        .buffer
                        .as_ref()
                        .ok_or_else(|| invalid_ipc("missing mmap buffer for batch"))?;
                    let decoder = self
                        .decoder
                        .as_ref()
                        .ok_or_else(|| invalid_ipc("missing IPC decoder for batch"))?;
                    let block = self
                        .batches
                        .get(i)
                        .ok_or_else(|| invalid_ipc("missing declared record batch"))?;
                    let data = block_buffer(buffer, block, self.body_end)?;
                    decoder
                        .read_record_batch(block, &data)?
                        .ok_or_else(|| invalid_ipc("declared IPC block is not a record batch"))
                };
                decode().map_err(|error| {
                    format!("failed to read {} batch {i}: {error}", self.path.display())
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Every batch of the file; a bad batch never becomes a partial result.
    pub fn batches_all(&self) -> Result<Vec<RecordBatch>, String> {
        self.batches_where(|_| true)
    }

    /// Batches whose block envelope passes `keep`. Files without valid
    /// `qm_blocks` metadata return everything. The predicate MUST be a superset
    /// of the row-level accept, or pruning drops audible sources.
    pub fn batches_where(
        &self,
        keep: impl Fn(&arrow_batching::RowBbox) -> bool,
    ) -> Result<Vec<RecordBatch>, String> {
        (0..self.slots.len())
            .filter(|&i| {
                self.blocks
                    .as_ref()
                    .is_none_or(|blocks| keep(&blocks[i].bbox))
            })
            .map(|i| self.batch(i).cloned())
            .collect()
    }

    /// Circular gate: great-circle distance from the click to the batch
    /// envelope ≤ radius. The 2% slack covers the haversine-vs-flat-earth
    /// mismatch of the row filters plus f32 bbox rounding (proven constant —
    /// over-admitting a borderline batch costs one decode).
    pub fn batches_within(
        &self,
        lat: f64,
        lon: f64,
        radius_m: f64,
    ) -> Result<Vec<RecordBatch>, String> {
        const GATE_RADIUS_SLACK: f64 = 1.02;
        self.batches_where(|bb| {
            arrow_batching::point_to_bbox_distance_m(lat, lon, bb) <= radius_m * GATE_RADIUS_SLACK
        })
    }
}

/// All source data for one z9 square — lazily-decoded Arrow IPC files.
pub struct SquareData {
    pub roads: LazyArrow,
    pub railways: LazyArrow,
    /// The merged per-square structure table (`structures.arrow`).
    pub structures: LazyArrow,
    pub industrial: LazyArrow,
    /// Leisure AREA sources (`leisure.arrow`).
    pub leisure: LazyArrow,
    pub aircraft_airborne: LazyArrow,
    pub aircraft_cruise: LazyArrow,
    pub aircraft_airport_traffic: LazyArrow,
    /// OSM aeroway microsegments (`airport_lines.arrow`).
    pub airport_lines: LazyArrow,
}

/// Load all source data from a square directory `…/z9/<x>/<y>`. Only footers +
/// schemas read here; batch bodies decode lazily at query time.
pub fn load_square(dir: &Path) -> Result<SquareData, String> {
    let structures = LazyArrow::open(&dir.join("structures.arrow"))?;
    let leisure = LazyArrow::open(&dir.join("leisure.arrow"))?;
    if let Some(schema) = structures.schema() {
        crate::structure_contract::validate_schema(schema)?;
    }
    check_contract(
        &leisure,
        "leisure_contract",
        LEISURE_CONTRACT_V2,
        "leisure.arrow",
        "re-extract the source store",
    )?;
    // Every extract-written file pins its coordinate grid; readers that do
    // not know integer grids must refuse the file, never misread it.
    for (arrow, label) in [
        (&structures, "structures.arrow"),
        (&leisure, "leisure.arrow"),
    ] {
        check_contract(
            arrow,
            "grid",
            GRID_CONTRACT_Z30,
            label,
            "re-extract the source store",
        )?;
    }
    // The nested one-row-per-flight layout decodes as valid Arrow; only the
    // stamp tells it from the flattened owner rows this reader computes on.
    let aircraft_airborne = LazyArrow::open(&dir.join("airborne.arrow"))?;
    check_contract(
        &aircraft_airborne,
        "airborne_contract",
        crate::aircraft_contract::AIRBORNE_CONTRACT,
        "airborne.arrow",
        "re-extract aircraft Stage 2A (shuffle + airborne flatten)",
    )?;

    let railways = LazyArrow::open(&dir.join("railways.arrow"))?;
    check_column_type(&railways, "maxspeed", DataType::UInt16, "railways.arrow")?;
    let roads = LazyArrow::open(&dir.join("roads.arrow"))?;
    check_column_type(&roads, "start_gx", DataType::Int32, "roads.arrow")?;

    Ok(SquareData {
        roads,
        railways,
        structures,
        industrial: LazyArrow::open(&dir.join("industrial.arrow"))?,
        leisure,
        aircraft_airborne,
        aircraft_cruise: LazyArrow::open(&dir.join("cruise.arrow"))?,
        aircraft_airport_traffic: LazyArrow::open(&dir.join("airport_traffic.arrow"))?,
        airport_lines: LazyArrow::open(&dir.join("airport_lines.arrow"))?,
    })
}

/// `structures.arrow` row routing (source of truth: `KIND_*` in
/// `scripts/structures/build-structures.py`).
pub const STRUCTURE_KIND_BUILDING: u8 = 0;
pub const STRUCTURE_KIND_BARRIER: u8 = 1;

/// Per-file contract stamps (sources of truth: `osm-extract::finalize`,
/// `scripts/structures/build-structures.py`). Mirrored here so the popup
/// rejects a stale file whose semantics predate the current schema.
pub const LEISURE_CONTRACT_V2: &str = "leisure_v2";
pub const GRID_CONTRACT_Z30: &str = "z30";

/// Verify a source arrow's schema carries the expected stamp. Missing file
/// passes. Fails loud on mismatch, naming the build step that rewrites it.
fn check_contract(
    arrow: &LazyArrow,
    key: &str,
    expected: &str,
    label: &str,
    recovery: &str,
) -> Result<(), String> {
    let Some(schema) = arrow.schema() else {
        return Ok(());
    };
    let c = schema.metadata().get(key).map(String::as_str);
    if c != Some(expected) {
        return Err(format!(
            "{label} {key} mismatch (expected {expected}, got {c:?}) — {recovery}"
        ));
    }
    Ok(())
}

fn check_column_type(
    arrow: &LazyArrow,
    column: &str,
    expected: DataType,
    label: &str,
) -> Result<(), String> {
    let Some(schema) = arrow.schema() else {
        return Ok(());
    };
    let actual = schema
        .field_with_name(column)
        .map_err(|_| format!("{label} is missing required {column} column"))?
        .data_type();
    if actual != &expected {
        return Err(format!(
            "{label} {column} must be {expected:?}, got {actual:?} — re-extract OSM"
        ));
    }
    Ok(())
}

/// Square directory for a prepared root + square, or `None` when the NAME is
/// not a square (stale caller guard — and the path is then built from parsed
/// integers only, so no traversal can escape the prepared root).
pub fn square_dir(prepared_root: &Path, name: &str) -> Option<PathBuf> {
    let square: Square = grid::parse_square_name(name)?;
    Some(
        prepared_root
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string()),
    )
}

/// Strictly read the committed readiness square's roads file without any
/// cache. Fails closed on corrupt/empty/wrong-schema files; fails open is
/// never an option for the readiness gate.
pub fn validate_reference_square(prepared_root: &Path, name: &str) -> Result<usize, String> {
    let Some(dir) = square_dir(prepared_root, name) else {
        return Err(format!("invalid reference square: {name:?}"));
    };

    let path = dir.join("roads.arrow");
    let file =
        File::open(&path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let reader = FileReader::try_new(file, None)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let schema = reader.schema();
    for (col, expected) in [
        ("osm_id", DataType::Int64),
        ("start_gx", DataType::Int32),
        ("start_gy", DataType::Int32),
        ("end_gx", DataType::Int32),
        ("end_gy", DataType::Int32),
    ] {
        let field = schema.field_with_name(col).map_err(|_| {
            format!(
                "{} roads schema is missing required column {col}",
                path.display()
            )
        })?;
        if field.data_type() != &expected {
            return Err(format!(
                "{} roads schema column {col} must have type {expected:?}, got {:?}",
                path.display(),
                field.data_type()
            ));
        }
    }

    let mut rows = 0usize;
    for (batch_index, batch) in reader.enumerate() {
        let batch = batch.map_err(|error| {
            format!(
                "failed to read {} batch {batch_index}: {error}",
                path.display()
            )
        })?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or_else(|| format!("row count overflow in {}", path.display()))?;
    }
    if rows == 0 {
        return Err(format!("{} contains no road rows", path.display()));
    }
    Ok(rows)
}

#[cfg(test)]
mod lazy_arrow_tests {
    use super::*;
    use arrow::array::{
        ArrayRef, DictionaryArray, Int32Array, StringArray, StructArray, UInt8Array,
    };
    use arrow::datatypes::{Field, Int32Type};
    use arrow::ipc::writer::FileWriter;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "qm-square-store-{label}-{}-{}.arrow",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_dictionary_file(path: &Path) {
        // `flights` mirrors airborne.arrow's identity column: one struct
        // dictionary shared by every row of the file.
        let flights = StructArray::from(vec![
            (
                Arc::new(Field::new("callsign", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["CSA1", "TVS100P"])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("profile_idx", DataType::UInt8, false)),
                Arc::new(UInt8Array::from(vec![3u8, 7])) as ArrayRef,
            ),
        ]);
        let batch = RecordBatch::try_from_iter([
            (
                "numbers",
                Arc::new(Int32Array::from(vec![11, 22, 33])) as ArrayRef,
            ),
            (
                "labels",
                Arc::new(DictionaryArray::<Int32Type>::from_iter([
                    Some("alpha"),
                    None,
                    Some("beta"),
                ])) as ArrayRef,
            ),
            (
                "flights",
                Arc::new(
                    DictionaryArray::<Int32Type>::try_new(
                        Int32Array::from(vec![1, 0, 1]),
                        Arc::new(flights),
                    )
                    .unwrap(),
                ) as ArrayRef,
            ),
        ])
        .unwrap();
        let mut file = File::create(path).unwrap();
        let mut writer = FileWriter::try_new(&mut file, batch.schema().as_ref()).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn decoded_buffers_including_struct_dictionaries_are_mmap_backed_and_survive_lazy_arrow_drop() {
        let path = test_path("zero-copy");
        write_dictionary_file(&path);
        let lazy = LazyArrow::open(&path).unwrap();
        let mapped = lazy.buffer.as_ref().unwrap();
        let mapped_start = mapped.as_ptr() as usize;
        let mapped_end = mapped_start + mapped.len();
        let batch = lazy.batches_all().unwrap().pop().unwrap();

        let numbers = batch
            .column_by_name("numbers")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        let numbers_start = numbers.values().as_ptr() as usize;
        assert!(numbers_start >= mapped_start);
        assert!(numbers_start + numbers.values().len() * size_of::<i32>() <= mapped_end);

        let labels = batch
            .column_by_name("labels")
            .unwrap()
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .unwrap();
        let label_values = labels
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let labels_start = label_values.value_data().as_ptr() as usize;
        assert!(labels_start >= mapped_start);
        assert!(labels_start + label_values.value_data().len() <= mapped_end);

        let flights = batch
            .column_by_name("flights")
            .unwrap()
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .unwrap();
        let identity = flights
            .values()
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        let callsigns = identity
            .column_by_name("callsign")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let profiles = identity
            .column_by_name("profile_idx")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        for (start, len) in [
            (
                callsigns.value_data().as_ptr() as usize,
                callsigns.value_data().len(),
            ),
            (
                flights.keys().values().as_ptr() as usize,
                3 * size_of::<i32>(),
            ),
            (profiles.values().as_ptr() as usize, 2),
        ] {
            assert!(start >= mapped_start && start + len <= mapped_end);
        }

        drop(lazy);
        assert_eq!(numbers.values(), &[11, 22, 33]);
        assert_eq!(label_values.value(0), "alpha");
        assert_eq!(label_values.value(1), "beta");
        assert_eq!(flights.keys().values(), &[1, 0, 1]);
        assert_eq!(callsigns.value(1), "TVS100P");
        assert_eq!(profiles.values(), &[3, 7]);
        std::fs::remove_file(path).unwrap();
    }

    /// A block table that outlives the body it points at fails the open as a
    /// whole: the layer reads as an error, never as the batches that still fit.
    #[test]
    fn a_block_table_pointing_past_the_body_fails_the_open() {
        let path = test_path("short-body");
        write_dictionary_file(&path);
        let bytes = std::fs::read(&path).unwrap();
        let trailer_start = bytes.len() - 10;
        let footer_len =
            i32::from_le_bytes(bytes[trailer_start..trailer_start + 4].try_into().unwrap())
                as usize;
        let footer_start = trailer_start - footer_len;
        // Cut 16 bytes off the end of the body and keep footer and trailer
        // intact: every block keeps its offset, so the last one now declares
        // bytes the file no longer holds.
        let mut short = bytes[..footer_start - 16].to_vec();
        short.extend_from_slice(&bytes[footer_start..]);
        std::fs::write(&path, short).unwrap();
        let error = LazyArrow::open(&path).err().unwrap();
        assert!(error.contains("outside file body"), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_lengths_prefixes_and_blocks_fail_closed() {
        let path = test_path("corrupt");
        std::fs::write(&path, b"too short").unwrap();
        assert!(LazyArrow::open(&path)
            .err()
            .unwrap()
            .contains("shorter than its 10-byte trailer"));

        write_dictionary_file(&path);
        let mut bytes = std::fs::read(&path).unwrap();
        let trailer = bytes.len() - 10;
        bytes[trailer..trailer + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(LazyArrow::open(&path)
            .err()
            .unwrap()
            .contains("footer length"));

        let buffer = Buffer::from_vec(vec![0_u8; 32]);
        assert!(block_buffer(&buffer, &Block::new(-1, 4, 4), 32).is_err());
        assert!(block_buffer(&buffer, &Block::new(24, 8, 8), 32).is_err());
        assert!(block_buffer(&buffer, &Block::new(8, 0, 8), 32).is_err());
        let mut truncated_prefix = vec![0_u8; 32];
        truncated_prefix[8..12].copy_from_slice(&CONTINUATION_MARKER);
        let truncated_prefix = Buffer::from_vec(truncated_prefix);
        assert!(block_buffer(&truncated_prefix, &Block::new(8, 4, 8), 32).is_err());
        let mut oversized_message = vec![0_u8; 32];
        oversized_message[8..12].copy_from_slice(&20_i32.to_le_bytes());
        let oversized_message = Buffer::from_vec(oversized_message);
        assert!(block_buffer(&oversized_message, &Block::new(8, 8, 8), 32).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
