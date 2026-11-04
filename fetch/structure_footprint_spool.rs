//! Bound footprint conversion memory by spilling the existing observation format, then sorting one square.
use crate::structures::observations::{self, Observation};
use anyhow::Result;
use std::path::Path;

pub struct Footprints {
    directory: tempfile::TempDir,
    year: String,
    rows: Vec<Vec<Observation>>,
    parts: Vec<usize>,
    buffered_bytes: usize,
    buffer_limit: usize,
}

impl Footprints {
    pub fn new(squares: usize, out: &Path, year: &str) -> Result<Self> {
        std::fs::create_dir_all(out)?;
        Ok(Self {
            directory: tempfile::Builder::new()
                .prefix(".footprints-")
                .tempdir_in(out)?,
            year: year.into(),
            rows: vec![Vec::new(); squares],
            parts: vec![0; squares],
            buffered_bytes: 0,
            // This is an I/O buffer budget, independent of world size and model values.
            buffer_limit: 64 * 1024 * 1024,
        })
    }

    pub fn push(&mut self, square: usize, row: Observation) -> Result<()> {
        self.buffered_bytes +=
            std::mem::size_of::<Observation>() + row.geometry.as_ref().map_or(0, Vec::len);
        self.rows[square].push(row);
        if self.buffered_bytes >= self.buffer_limit {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        for (square, rows) in self.rows.iter_mut().enumerate() {
            if rows.is_empty() {
                continue;
            }
            let path = self
                .directory
                .path()
                .join(format!("{square}/{}.arrow", self.parts[square]));
            observations::write(&path, rows, &self.year)?;
            self.parts[square] += 1;
            // Release capacities too: sparse squares must not retain the world's peak buffers.
            *rows = Vec::new();
        }
        self.buffered_bytes = 0;
        Ok(())
    }

    pub fn take(&mut self, square: usize) -> Result<Vec<Observation>> {
        let mut rows = Vec::new();
        for part in 0..self.parts[square] {
            let path = self.directory.path().join(format!("{square}/{part}.arrow"));
            rows.extend(observations::read(&path, &self.year)?);
            std::fs::remove_file(path)?;
        }
        self.parts[square] = 0;
        // Stable degree ordering retains publisher order within a degree across spill boundaries.
        rows.sort_by_key(|row| [row.latitude.floor() as i16, row.longitude.floor() as i16]);
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spills_preserve_degree_and_publisher_order_and_exact_observation_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let row = |latitude, height| Observation {
            kind: observations::FOOTPRINT,
            geometry: Some(vec![1, 2, 3]),
            longitude: 14.0,
            latitude,
            height_m: Some(height),
            floors: 0,
            building_type: None,
            envelope: 1,
            open_roof: false,
            owned: true,
        };
        let mut expected = vec![row(50.2, 3.0), row(49.8, 1.0), row(50.1, 2.0)];
        let mut spool = Footprints::new(3, temporary.path(), "2024").unwrap();
        spool.buffer_limit = 1;
        for value in &expected {
            spool.push(1, value.clone()).unwrap();
            assert_eq!(spool.buffered_bytes, 0);
        }
        spool.flush().unwrap();
        assert!(spool.take(0).unwrap().is_empty());
        expected.sort_by_key(|row| [row.latitude.floor() as i16, row.longitude.floor() as i16]);
        let actual = spool.take(1).unwrap();
        let before = temporary.path().join("before.arrow");
        let after = temporary.path().join("after.arrow");
        observations::write(&before, &expected, "2024").unwrap();
        observations::write(&after, &actual, "2024").unwrap();
        assert_eq!(
            std::fs::read(before).unwrap(),
            std::fs::read(after).unwrap()
        );
        let scratch = spool.directory.path().to_owned();
        drop(spool);
        assert!(!scratch.exists());
    }
}
