//! `AdsbTarSource` — read flights from a local adsb.lol TAR cache.
//!
//! Layout (matches `data/source/flights-cache/{global|radius/<region>}/<year>/<day>/`):
//!
//! ```text
//! <root>/<year>/<day>/subset.tar          # one file
//! <root>/<year>/<day>/subset.tar.aa       # split, recovered via ConcatReader
//! <root>/<year>/<day>/subset.tar.ab
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::filters;
use crate::flight::{origin, source_id, Flight};
use crate::profile;
use crate::provider_receipt::ProviderDayReceipt;
use crate::segment::split_flights;
use crate::source::{FlightSource, ProviderDay};
use crate::trace::{read_day_archive, AircraftTrace, TracePoint};

pub struct AdsbTarSource {
    root: PathBuf,
    selected_archives: Option<BTreeMap<String, Vec<PathBuf>>>,
    source_id: u8,
}

impl AdsbTarSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            selected_archives: None,
            source_id: source_id::ADSB_LOL_TAR,
        }
    }

    /// Use the publisher-validated paths while retaining the native archive selector.
    pub fn with_selected_archives(mut self, paths: BTreeMap<String, Vec<PathBuf>>) -> Self {
        self.selected_archives = Some(paths);
        self
    }

    fn selected_day_dir(&self, day: &str) -> Result<PathBuf> {
        let Some(selected) = &self.selected_archives else {
            return Ok(self.day_dir(day));
        };
        let paths = selected
            .get(day)
            .context("day absent from selected source inputs")?;
        let dir = paths
            .first()
            .and_then(|p| p.parent())
            .context("empty selected source day")?;
        let expected: BTreeSet<_> = paths.iter().cloned().collect();
        let actual: BTreeSet<_> = crate::trace::archive_parts(dir)?.into_iter().collect();
        anyhow::ensure!(
            expected.len() == paths.len() && expected == actual,
            "{day}: native archive set differs from publisher-selected inputs"
        );
        Ok(dir.to_path_buf())
    }

    /// Tag the provenance `source_id` (default [`source_id::ADSB_LOL_TAR`]).
    /// The adsbexchange feed passes [`source_id::ADSB_EXCHANGE`] — same TAR
    /// format, so only the stamped provenance differs.
    pub fn with_source_id(mut self, source_id: u8) -> Self {
        self.source_id = source_id;
        self
    }

    /// Preferred layout: `<root>/<year>/<day>/` (the full ADS-B
    /// archive). Second candidate: the raw adsb.lol release naming
    /// `<root>/<year>/v{YYYY.MM.DD}-planes-readsb-prod-0/` as
    /// downloaded in the release tree — reading it in place
    /// keeps the 1.1 TB archive pristine with no symlink farm. Per-day `.ok`
    /// markers are ignored by the tar-extension filter. The `…prod-0tmp` suffix
    /// is upstream's release-tag naming for 15 days of 2025-05/06 —
    /// complete downloads, verified on the real archive; without it
    /// those days would silently resolve to zero flights. Falls back
    /// to `<root>/<day>/` — bbox / radius subsets produced by
    /// `adsb-subset-cache` typically place day dirs directly under
    /// the cache root, no year layer.
    fn day_dir(&self, day_str: &str) -> PathBuf {
        let year = day_str.split('-').next().unwrap_or("");
        let with_year = self.root.join(year).join(day_str);
        if with_year.exists() {
            return with_year;
        }
        let dotted = day_str.replace('-', ".");
        for suffix in ["", "tmp"] {
            let release = self
                .root
                .join(year)
                .join(format!("v{dotted}-planes-readsb-prod-0{suffix}"));
            if release.exists() {
                return release;
            }
        }
        self.root.join(day_str)
    }

    /// Read-only world-build preflight admission: the day resolves to its
    /// archive directory whose TAR parts pass the same structural admission
    /// (`archive_parts`, end-marker tails) Stage 0 will read. Only the final
    /// kilobyte of each archive is touched, never a whole multi-terabyte day.
    pub fn require_archive_day(&self, day_str: &str) -> Result<()> {
        crate::period::parse_date_id(day_str)?;
        let dir = self.day_dir(day_str);
        anyhow::ensure!(
            dir.is_dir(),
            "missing ADS-B day {day_str}: {}",
            dir.display()
        );
        crate::trace::archive_parts(&dir)?;
        Ok(())
    }
}

impl FlightSource for AdsbTarSource {
    fn source_id(&self) -> u8 {
        self.source_id
    }

    fn has_day(&self, day_str: &str) -> bool {
        match &self.selected_archives {
            Some(selected) => selected.contains_key(day_str),
            None => self.day_dir(day_str).is_dir(),
        }
    }

    fn read_provider_day(&self, day_str: &str) -> Result<ProviderDay> {
        crate::period::parse_date_id(day_str)?;
        let dir = self.selected_day_dir(day_str)?;
        anyhow::ensure!(
            dir.is_dir(),
            "missing ADS-B day {day_str}: {}",
            dir.display()
        );
        let read = read_day_archive(&dir)
            .with_context(|| format!("read ADS-B day {day_str} from {}", dir.display()))?;
        let receipt =
            ProviderDayReceipt::from_traces(self.source_id, day_str, &read.traces, read.corrupt_members)?;
        Ok(ProviderDay {
            source_id: self.source_id,
            traces: read.traces,
            receipt,
        })
    }
}

/// Convert a merged provider-day trace into one [`Flight`] per rotation.
/// Filters structurally-bad points, splits at sustained on-ground rests
/// (≥ `MIN_TURNAROUND_S`) via [`split_flights`], packs a per-rotation
/// `flight_id` from `(icao24, rotation_start_ts)`, and picks the
/// callsign active at each rotation's start. A rotation made only of
/// secondary-provider samples carries `secondary_source`, every other one
/// `primary_source`; the per-sample provenance itself travels in the point flags.
pub fn trace_to_flight(mut tr: AircraftTrace, primary_source: u8, secondary_source: u8) -> Vec<Flight> {
    // Fixed towers are silent; GND traces use vehicle emission rather than aircraft NPD.
    let typecode_trim = tr.aircraft_type.trim();
    if typecode_trim.eq_ignore_ascii_case("TWR") {
        return Vec::new();
    }
    // Gliders are normally unpowered; classification codes live with the NPD generator.
    if profile::is_negligible_noise_typecode(typecode_trim) {
        return Vec::new();
    }
    let is_gse = typecode_trim.eq_ignore_ascii_case("GND");
    tr.retain_points(|_, point| filters::point_is_sane(point));
    if tr.points.len() < 2 {
        return Vec::new();
    }
    let points = tr.points;
    let callsigns = tr.callsigns;

    let icao24 = profile::parse_icao24_hex(&tr.icao24).unwrap_or(0);
    let icao24_real = icao24 != 0 && icao24 != 0xFF_FFFF;
    let prof = profile::profile_idx(&tr.aircraft_type);
    let ranges = split_flights(&points);

    let mut flights = Vec::with_capacity(ranges.len());
    for rot in ranges {
        let rot_pts: Vec<TracePoint> = points[rot.clone()].to_vec();
        let source = if rot_pts.iter().all(TracePoint::is_secondary_provider) {
            secondary_source
        } else {
            primary_source
        };
        let first_ts = rot_pts[0].timestamp as u32;
        let flight_id = if icao24_real {
            profile::pack_real(icao24, first_ts)
                .unwrap_or_else(|| synth_id_for(&tr.aircraft_type, first_ts, &rot_pts))
        } else {
            synth_id_for(&tr.aircraft_type, first_ts, &rot_pts)
        };
        // Best movement label: the first callsign announced inside
        // this rotation. ADS-B identification frames re-fire every few
        // seconds, so any rotation actively broadcasting will hit.
        // Sparse / silent rotations leave the scalar empty rather than
        // inheriting a stale callsign from the previous movement
        // (which would silently mis-attribute).
        let callsign = callsigns
            .iter()
            .find(|c| rot.contains(&c.point_idx))
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let (veh_kind, gse_class, profile_idx_field) = if is_gse {
            // GSE emission uses gse_class, never an aircraft profile. The sentinel
            // marks that absence; consumers must branch on veh_kind (NPD lookups clamp).
            (
                1u8,
                noise_compute::emission::gse::classify_gse_callsign(&callsign),
                u8::MAX,
            )
        } else {
            (0u8, 0u8, prof)
        };
        flights.push(Flight {
            flight_id,
            callsign,
            aircraft_type: tr.aircraft_type.clone(),
            profile_idx: profile_idx_field,
            source_id: source,
            origin: origin::OBSERVED,
            veh_kind,
            gse_class,
            points: rot_pts,
        });
    }
    flights
}

fn synth_id_for(typecode: &str, first_ts: u32, pts: &[TracePoint]) -> u64 {
    // Anonymous traffic gets a deterministic synthetic id keyed on
    // (typecode, first-point coordinate, first timestamp). Two reads
    // of the same anonymous flight produce the same id; collisions
    // across distinct anonymous flights are rounding noise relative
    // to real-flight headcount.
    let p = &pts[0];
    let mut seed: u64 = first_ts as u64;
    for b in typecode.as_bytes() {
        seed = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(*b as u64);
    }
    seed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(p.lat.to_bits() as u64);
    seed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(p.lon.to_bits() as u64);
    profile::pack_synth(seed)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod rotation_tests;
#[cfg(test)]
mod vehicle_tests;
