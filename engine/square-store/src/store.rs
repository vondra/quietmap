//! One prepared z9 square's source files, opened lazily via mmap.
//!
//! Spatial batch metadata prunes bodies outside the click's reach. Only absent
//! optional files are empty; opening or decoding an existing file fails the
//! query on error. Source contracts reject stale coordinate and layer semantics.

use arrow::datatypes::DataType;
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use grid::Square;
use memmap2::Mmap;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// One arrow file, opened (footer + schema only) but not decoded.
pub struct LazyArrow {
    mmap: Option<Arc<Mmap>>,
    schema: Option<arrow::datatypes::SchemaRef>,
    batch_bboxes: Option<Vec<arrow_batching::RowBbox>>,
    slots: Vec<OnceLock<Result<RecordBatch, String>>>,
    path: PathBuf,
}

impl LazyArrow {
    pub fn empty() -> Self {
        LazyArrow {
            mmap: None,
            schema: None,
            batch_bboxes: None,
            slots: Vec::new(),
            path: PathBuf::new(),
        }
    }

    /// Open `path`: mmap + IPC footer + schema. No batch bodies decode here.
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
        let mmap = Arc::new(mmap);
        let reader = FileReader::try_new(Cursor::new(mmap.as_ref().as_ref()), None)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let schema = reader.schema();
        let num_batches = reader.num_batches();
        let batch_bboxes = schema
            .metadata()
            .get(arrow_batching::QM_BATCH_BBOXES_KEY)
            .and_then(|v| arrow_batching::parse_batch_bboxes(v))
            .filter(|b| b.len() == num_batches);
        Ok(LazyArrow {
            mmap: Some(mmap),
            schema: Some(schema),
            batch_bboxes,
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
                let decode = || -> Result<RecordBatch, arrow::error::ArrowError> {
                    let mmap = self.mmap.as_ref().ok_or_else(|| {
                        arrow::error::ArrowError::ParseError("missing mmap for batch".into())
                    })?;
                    let mut reader =
                        FileReader::try_new(Cursor::new(mmap.as_ref().as_ref()), None)?;
                    reader.set_index(i)?;
                    reader.next().ok_or_else(|| {
                        arrow::error::ArrowError::ParseError("missing declared record batch".into())
                    })?
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

    /// Batches whose bbox passes `keep`. Files without valid bbox metadata
    /// return everything. The predicate MUST be a superset of the row-level
    /// accept, or pruning drops audible sources.
    pub fn batches_where(
        &self,
        keep: impl Fn(&arrow_batching::RowBbox) -> bool,
    ) -> Result<Vec<RecordBatch>, String> {
        (0..self.slots.len())
            .filter(|&i| {
                self.batch_bboxes
                    .as_ref()
                    .is_none_or(|bboxes| keep(&bboxes[i]))
            })
            .map(|i| self.batch(i).cloned())
            .collect()
    }

    /// Circular gate for planar distance ≤ radius. The 2% slack covers the
    /// haversine-vs-planar metric mismatch plus f32 bbox rounding (proven
    /// constant — over-admitting a borderline batch costs one decode).
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
    pub aircraft_airport_summary: LazyArrow,
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
    )?;
    // Every extract-written file pins its coordinate grid; readers that do
    // not know integer grids must refuse the file, never misread it.
    for (arrow, label) in [
        (&structures, "structures.arrow"),
        (&leisure, "leisure.arrow"),
    ] {
        check_contract(arrow, "grid", GRID_CONTRACT_Z30, label)?;
    }

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
        aircraft_airborne: LazyArrow::open(&dir.join("airborne.arrow"))?,
        aircraft_cruise: LazyArrow::open(&dir.join("cruise.arrow"))?,
        aircraft_airport_traffic: LazyArrow::open(&dir.join("airport_traffic.arrow"))?,
        aircraft_airport_summary: LazyArrow::open(&dir.join("airport_summary.arrow"))?,
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
/// passes. Fails loud on mismatch.
fn check_contract(arrow: &LazyArrow, key: &str, expected: &str, label: &str) -> Result<(), String> {
    let Some(schema) = arrow.schema() else {
        return Ok(());
    };
    let c = schema.metadata().get(key).map(String::as_str);
    if c != Some(expected) {
        return Err(format!(
            "{label} {key} mismatch (expected {expected}, got {c:?}) — \
             re-extract the source store"
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
