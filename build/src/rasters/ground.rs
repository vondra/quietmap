//! Prepare missing flight-path heights from the same raw raster model, without building unrelated bands.
use super::{config::Band, countries::Countries, source::Source};
use crate::{dev4::Square, year::YearInputs};
use std::path::{Path, PathBuf};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct GroundHeights {
    sources: Vec<Mutex<Vec<Source>>>,
    countries: Countries,
    coverage: Vec<f64>,
    root: PathBuf,
    squares: Mutex<HashMap<Square, Arc<Mutex<()>>>>,
}

impl GroundHeights {
    pub fn open(inputs: &YearInputs, root: &Path) -> Result<Self, String> {
        let countries = super::read_countries(inputs)?;
        let sources = super::open_sources(inputs, &countries, |band| {
            matches!(band, Band::GroundHeight | Band::Coverage)
        })?;
        super::cache::prepare(inputs, &sources, root)?;
        let coverage = super::land_coverage(&sources)?;
        Self::from_sources(
            sources,
            countries,
            coverage,
            root.to_owned(),
            rayon::current_num_threads(),
        )
    }

    pub(super) fn from_sources(
        sources: Vec<Source>,
        countries: Countries,
        coverage: Vec<f64>,
        root: PathBuf,
        workers: usize,
    ) -> Result<Self, String> {
        let mut copies = Vec::new();
        for _ in 1..workers {
            copies.push(Mutex::new(
                sources
                    .iter()
                    .map(Source::reopen)
                    .collect::<Result<Vec<_>, _>>()?,
            ));
        }
        copies.push(Mutex::new(sources));
        Ok(Self {
            sources: copies,
            countries,
            coverage,
            root,
            squares: Mutex::new(HashMap::new()),
        })
    }

    pub fn square(&self, square: Square) -> Result<PathBuf, String> {
        let lock = self
            .squares
            .lock()
            .map_err(|_| "raw-height square locks poisoned")?
            .entry(square)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _square = lock.lock().map_err(|_| "raw-height square lock poisoned")?;
        let path = self
            .root
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string())
            .join("dem.u16le");
        if !path.exists() {
            let started = std::time::Instant::now();
            eprintln!("flight terrain: preparing {}:{}", square.x, square.y);
            let window = crate::dev4::z9_raster_window(square);
            let bytes = if super::touches_land(&self.coverage, window) {
                let index = rayon::current_thread_index().unwrap_or(0) % self.sources.len();
                let sources = self.sources[index]
                    .lock()
                    .map_err(|_| "raw-height worker lock poisoned")?;
                super::dem::build(&sources, &self.countries, window)?
            } else {
                Vec::new()
            };
            super::write(&path, &bytes)?;
            eprintln!(
                "flight terrain: {}:{}: {} bytes in {:.3} seconds",
                square.x,
                square.y,
                bytes.len(),
                started.elapsed().as_secs_f64()
            );
        }
        Ok(path)
    }
}
