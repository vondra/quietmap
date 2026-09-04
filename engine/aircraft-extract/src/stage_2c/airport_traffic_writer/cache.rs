//! Validated real and synthetic airport lines with explicit prepared-square ownership.
use super::*;
use crate::airport_io::{nearest_aerodrome_within, read_airport_lines};
use crate::geo::midpoint;
use crate::synth_airport_io::{is_synthetic_osm_id, read_synth_airport_lines, SYNTH_LINES_FILE};

#[derive(Default)]
pub(super) struct SquareCache {
    pub lines: Vec<AirportLineSegment>,
    pub line_index: HashMap<(u64, u16), usize>,
    pub airport_keys: Vec<String>,
    pub owners: Vec<u64>,
}

impl SquareCache {
    pub(super) fn load(root: &Path, owner: u64, areas: &[AirportArea]) -> Result<Self> {
        Self::load_many(root, &[owner], areas)
    }

    pub(super) fn load_many(root: &Path, owners: &[u64], areas: &[AirportArea]) -> Result<Self> {
        let mut cache = Self::default();
        for &owner in owners {
            let dir = root.join(square_path(owner));
            let real_path = dir.join("airport_lines.arrow");
            for row in read_airport_lines(&real_path)
                .with_context(|| format!("read {}", real_path.display()))?
            {
                anyhow::ensure!(
                    !is_synthetic_osm_id(row.osm_id),
                    "real airport line has synthetic id in {}",
                    real_path.display()
                );
                let line = AirportLineSegment {
                    osm_id: row.osm_id,
                    segment_idx: row.segment_idx,
                    start_lat: row.start_lat,
                    start_lon: row.start_lon,
                    end_lat: row.end_lat,
                    end_lon: row.end_lon,
                    grid: row.grid,
                    length_m: row.length_m,
                    aeroway_type: row.aeroway_type,
                };
                let (lat, lon) =
                    midpoint(line.start_lat, line.start_lon, line.end_lat, line.end_lon);
                let key = nearest_aerodrome_within(lat as f64, lon as f64, areas)
                    .filter(|area| !area.airport_key.is_empty())
                    .map(|area| area.airport_key.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "strip:z15:{}",
                            crate::spatial::cruise_cell_id(lat as f64, lon as f64)
                        )
                    });
                cache.insert(line, key, owner)?;
            }
            let synth_path = dir.join(SYNTH_LINES_FILE);
            for row in read_synth_airport_lines(&synth_path)
                .with_context(|| format!("read {}", synth_path.display()))?
            {
                anyhow::ensure!(
                    is_synthetic_osm_id(row.osm_id),
                    "synthetic airport line has real id in {}",
                    synth_path.display()
                );
                let (start_lon, start_lat) =
                    square_store::grid_cols::grid_cell_lonlat(row.start_gx, row.start_gy);
                let (end_lon, end_lat) =
                    square_store::grid_cols::grid_cell_lonlat(row.end_gx, row.end_gy);
                let line = AirportLineSegment {
                    osm_id: row.osm_id,
                    segment_idx: row.segment_idx,
                    start_lat: start_lat as f32,
                    start_lon: start_lon as f32,
                    end_lat: end_lat as f32,
                    end_lon: end_lon as f32,
                    grid: ((row.start_gx, row.start_gy), (row.end_gx, row.end_gy)),
                    length_m: row.length_m,
                    aeroway_type: row.aeroway_type,
                };
                cache.insert(line, row.airport_key, owner)?;
            }
        }
        Ok(cache)
    }

    fn insert(&mut self, line: AirportLineSegment, key: String, owner: u64) -> Result<()> {
        anyhow::ensure!(
            line.length_m.is_finite() && line.length_m > 0.0 && !key.is_empty(),
            "invalid airport line length or identity in {}",
            square_path(owner)
        );
        anyhow::ensure!(
            ops_kind_from_aeroway(line.aeroway_type).is_some(),
            "invalid airport line aeroway type {}",
            line.aeroway_type
        );
        for (lat, lon) in [
            (line.start_lat, line.start_lon),
            (line.end_lat, line.end_lon),
        ] {
            anyhow::ensure!(
                lat.is_finite()
                    && lon.is_finite()
                    && (-90.0..=90.0).contains(&lat)
                    && (-180.0..=180.0).contains(&lon),
                "invalid airport line coordinates in {}",
                square_path(owner)
            );
        }
        let identity = (line.osm_id, line.segment_idx);
        anyhow::ensure!(
            !self.line_index.contains_key(&identity),
            "duplicate airport microsegment {identity:?} in {}",
            square_path(owner)
        );
        self.line_index.insert(identity, self.lines.len());
        self.lines.push(line);
        self.airport_keys.push(key);
        self.owners.push(owner);
        Ok(())
    }
}
