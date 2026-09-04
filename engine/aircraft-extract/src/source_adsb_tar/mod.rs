//! `AdsbTarSource` — read flights from a local adsb.lol TAR cache.
//!
//! Layout (matches `data/source/flights-cache/{global|radius/<region>}/<year>/<day>/`):
//!
//! ```text
//! <root>/<year>/<day>/subset.tar          # one file
//! <root>/<year>/<day>/subset.tar.aa       # split, recovered via ConcatReader
//! <root>/<year>/<day>/subset.tar.ab
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::filters;
use crate::flight::{origin, source_id, Flight};
use crate::profile;
use crate::segment::split_flights;
use crate::source::FlightSource;
use crate::trace::{read_day_traces, read_day_traces_filtered, AircraftTrace, TracePoint};

/// Stage-0 class-window routing for the hybrid GA/airline sampling. The GA
/// pass observes only full-year-sampled classes (PROP_C172 + HELICOPTER);
/// the airline pass
/// keeps the complement — including GSE: ground vehicles belong to the
/// 12-day airline window. `All` is the single-window default,
/// byte-identical to the pre-hybrid pipeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClassWindowFilter {
    #[default]
    All,
    GaOnly,
    NonGa,
}

impl ClassWindowFilter {
    /// Whether a trace with this raw `"t"` typecode survives this pass.
    ///
    /// Single decision point shared by [`trace_to_flight`] (the
    /// authoritative drop) and the gzip prefix probe in `read_day`
    /// (early skip of the full inflate+parse) — sharing it makes
    /// GaOnly/NonGa complementarity and probe == full-parse consistency
    /// hold by construction. TWR + glider traces are dropped in EVERY
    /// pass (`trace_to_flight` drops them before the window check;
    /// returning false here merely saves their parse).
    pub fn keeps_typecode(self, raw_typecode: &str) -> bool {
        if matches!(self, ClassWindowFilter::All) {
            return true;
        }
        let trimmed = raw_typecode.trim();
        if trimmed.eq_ignore_ascii_case("TWR") || profile::is_negligible_noise_typecode(trimmed) {
            return false;
        }
        // GSE (GND) routes by vehicle kind, not by what `profile_idx`
        // makes of the "GND" string — it belongs to the airline pass.
        let is_gse = trimmed.eq_ignore_ascii_case("GND");
        let ga_sampled =
            !is_gse && profile::is_ga_sampled_profile(profile::profile_idx(raw_typecode));
        match self {
            ClassWindowFilter::All => true,
            ClassWindowFilter::GaOnly => ga_sampled,
            ClassWindowFilter::NonGa => !ga_sampled,
        }
    }
}

pub struct AdsbTarSource {
    root: PathBuf,
    source_id: u8,
    class_filter: ClassWindowFilter,
}

impl AdsbTarSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            source_id: source_id::ADSB_LOL_TAR,
            class_filter: ClassWindowFilter::All,
        }
    }

    /// Tag the provenance `source_id` (default [`source_id::ADSB_LOL_TAR`]).
    /// The adsbexchange feed passes [`source_id::ADSB_EXCHANGE`] — same TAR
    /// format, so only the stamped provenance (and dedup identity) differ.
    pub fn with_source_id(mut self, source_id: u8) -> Self {
        self.source_id = source_id;
        self
    }

    /// Restrict ingest to one hybrid sampling pass (default
    /// [`ClassWindowFilter::All`] = single-window extract).
    pub fn with_class_filter(mut self, class_filter: ClassWindowFilter) -> Self {
        self.class_filter = class_filter;
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
}

impl FlightSource for AdsbTarSource {
    fn source_id(&self) -> u8 {
        self.source_id
    }

    fn read_day(&self, day_str: &str) -> Result<Vec<Flight>> {
        crate::period::parse_date_id(day_str)?;
        let dir = self.day_dir(day_str);
        anyhow::ensure!(
            dir.is_dir(),
            "missing ADS-B day {day_str}: {}",
            dir.display()
        );
        // Window-filtered passes drive the gzip typecode prefix probe:
        // traces the window would drop skip the full inflate+parse
        // (the GA pass's cost lever — airliner traces are the longest
        // files). Probe misses full-parse and re-filter, so
        // `trace_to_flight` below stays the single authority.
        let filter = self.class_filter;
        let traces = match filter {
            ClassWindowFilter::All => read_day_traces(&dir)
                .with_context(|| format!("read ADS-B day {day_str} from {}", dir.display()))?,
            _ => {
                let keep = |typecode: &str| filter.keeps_typecode(typecode);
                let (traces, probe) = read_day_traces_filtered(&dir, Some(&keep))?;
                eprintln!(
                    "{} [stage0] {day_str} typecode-probe ({filter:?}): {} hits \
                     ({} skipped pre-parse), {} misses → full parse",
                    crate::progress::ts(),
                    probe.probe_hits,
                    probe.skipped_pre_parse,
                    probe.probe_misses,
                );
                traces
            }
        };
        // One trace_full_<icao>.json typically covers multiple flights
        // per day (a 737 doing 4 rotations); `trace_to_flight` splits
        // the day on long telemetry gaps and emits one `Flight` per
        // movement so downstream `flight_id` is per-rotation, not
        // per-icao24-day.
        let mut out = Vec::with_capacity(traces.len());
        for tr in traces {
            out.extend(trace_to_flight(tr, self.source_id, filter));
        }
        Ok(out)
    }
}

/// Convert a parsed adsb.lol trace into one [`Flight`] per rotation.
/// Filters structurally-bad points, splits at sustained on-ground rests
/// (≥ `MIN_TURNAROUND_S`) via [`split_flights`], packs a per-rotation
/// `flight_id` from `(icao24, rotation_start_ts)`, and picks the
/// callsign active at each rotation's start from the trace's
/// pre-rebased transition list. `window` drops the traces outside this
/// hybrid sampling pass ([`ClassWindowFilter::All`] keeps everything).
pub fn trace_to_flight(tr: AircraftTrace, source: u8, window: ClassWindowFilter) -> Vec<Flight> {
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
    // Hybrid class-window routing: the GA pass keeps only GA-sampled
    // classes (GSE → airline pass), while the
    // airline pass drops them. Runs before the per-point work so a
    // probe-missed trace costs no more than its parse.
    if !window.keeps_typecode(&tr.aircraft_type) {
        return Vec::new();
    }
    let mut surviving: Vec<u32> = Vec::with_capacity(tr.points.len());
    let mut points: Vec<TracePoint> = Vec::with_capacity(tr.points.len());
    for (old_idx, p) in tr.points.into_iter().enumerate() {
        if filters::point_is_sane(&p) {
            surviving.push(old_idx as u32);
            points.push(p);
        }
    }
    if points.len() < 2 {
        return Vec::new();
    }

    // Several raw transitions inside a dropped span collapse onto the
    // next surviving point — the LAST one wins (active value when
    // telemetry resumed); subsequent `dedup_by` collapses runs that
    // ended up identical.
    let mut callsigns: Vec<crate::trace::CallsignChange> = Vec::with_capacity(tr.callsigns.len());
    for ch in tr.callsigns {
        let new_idx = surviving.partition_point(|&i| (i as usize) < ch.point_idx);
        if new_idx >= surviving.len() {
            continue;
        }
        match callsigns.last_mut().filter(|c| c.point_idx == new_idx) {
            Some(last) => last.value = ch.value,
            None => callsigns.push(crate::trace::CallsignChange {
                point_idx: new_idx,
                value: ch.value,
            }),
        }
    }
    callsigns.dedup_by(|a, b| a.value == b.value);

    let icao24 = profile::parse_icao24_hex(&tr.icao24).unwrap_or(0);
    let icao24_real = icao24 != 0 && icao24 != 0xFF_FFFF;
    let prof = profile::profile_idx(&tr.aircraft_type);
    let ranges = split_flights(&points);

    let mut flights = Vec::with_capacity(ranges.len());
    for rot in ranges {
        let rot_pts: Vec<TracePoint> = points[rot.clone()].to_vec();
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
