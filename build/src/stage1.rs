//! Stage 1's day files (`segments/<day>.arrow`, compressed batches of a world day's 86 M
//! segments) read by batches at once: each batch is decoded by a reader of its own, with only the
//! columns its reader needs.

use arrow_array::RecordBatch;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

type Reader = arrow_ipc::reader::FileReader<BufReader<File>>;

/// One day file and the columns read from it.
pub struct DayFile {
    path: PathBuf,
    projection: Vec<usize>,
    batches: usize,
}

impl DayFile {
    /// The day file at `path` with `columns`; `None` when it is absent.
    pub fn open(path: &Path, columns: &[&str]) -> Result<Option<Self>, String> {
        let reader = match File::open(path) {
            Ok(file) => {
                Reader::try_new(BufReader::new(file), None).map_err(|e| failed(path, &e))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failed(path, &error)),
        };
        let schema = reader.schema();
        let projection = columns
            .iter()
            .map(|name| schema.index_of(name).map_err(|e| failed(path, &e)))
            .collect::<Result<_, _>>()?;
        Ok(Some(DayFile {
            path: path.to_path_buf(),
            projection,
            batches: reader.num_batches(),
        }))
    }

    pub fn batches(&self) -> usize {
        self.batches
    }

    /// Batch `index`, decoded by a reader of its own (batches of one file decode at once).
    pub fn batch(&self, index: usize) -> Result<RecordBatch, String> {
        let file = File::open(&self.path).map_err(|e| failed(&self.path, &e))?;
        let mut reader = Reader::try_new(BufReader::new(file), Some(self.projection.clone()))
            .map_err(|e| failed(&self.path, &e))?;
        reader
            .set_index(index)
            .map_err(|e| failed(&self.path, &e))?;
        reader
            .next()
            .ok_or_else(|| failed(&self.path, &"a batch missing"))?
            .map_err(|e| failed(&self.path, &e))
    }
}

fn failed(path: &Path, error: &dyn std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}
