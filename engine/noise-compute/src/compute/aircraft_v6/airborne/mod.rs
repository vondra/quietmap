//! Direct scatter over flattened airborne popup batches. Each
//! `AirborneSegmentBatch` carries sub-segment rows and its file's flight
//! table; this kernel iterates the rows (`row`: gates + Doc 29 SEL chain +
//! Lmax LUT), updates per-real-flight `FlightAccum`s, joining a flight's
//! identity only when its first row contributes, and folds split chords
//! back into one event each (`chords`). No `AircraftSegment` `Vec` is
//! allocated — segments are built on the stack per iteration.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};

use rayon::prelude::*;

use crate::compute::aircraft_v6::state::{BandStats, FlightAccum, TopFlightCandidate};
use crate::compute::aircraft_v6::views::AirborneSegmentBatch;
use crate::emission::aircraft;
use crate::types::{
    AircraftAirborneDetail, AircraftEventBandStats, AircraftTopFlight, ImpactDeltas, NoisePeriods,
    Receiver, SegmentTrace, TraceCollector,
};

mod chords;
mod row;

use chords::{ChordCandidate, PieceEval};
use row::{build_row_trace, evaluate_row, flight_accumulator, ScatterContext};

/// Maximum number of `top_flights` rows the popup returns. Frontend
/// renders a sortable table; 20 is the empirical break-even between
/// "user can see all the loud flights" and "table fits without paging".
const TOP_FLIGHTS_N: usize = 20;

/// Lmax (dB) below which sub-segment traces are not built. Saves
/// ~95 % of `SegmentTrace` allocations at LKPR.
///
/// Safety margin (worst case, generous): a single transit yields
/// SEL ≈ Lmax + 15 dB for the loudest profile classes; over a 24 h
/// daily window the contribution is SEL − 10·log10(86400) + 10 dB
/// night penalty = (Lmax+15) − 49.4 + 10 ≈ Lmax − 24.4 dB. With
/// Lmax = 25 dB that puts a single-event Lden contribution near
/// 0.6 dB — already well below the popup segment tab's empirical
/// rank floor of ~40 dB Lden (LKPR top-150). Below 25 dB Lmax a
/// sub-segment cannot survive the user-visible cap regardless of
/// the SEL/Lmax delta assumed. Energy accumulation (period_energy,
/// peak_lmax updates) runs regardless — only the SegmentTrace
/// allocation + push are gated, so Lden parity is held.
const AIRBORNE_TRACE_CUTOFF_DB: f64 = 25.0;

/// Monotone-with-`received_lden.full` rank key used by the bounded
/// top-K heap. Avoids per-sub-seg `compute_lden` calls (3× exp + 1× log10
/// ≈ 120 ns) — `received_lden.full` is
///   `10·log10(W[period] · energy / (n_days · 86400)) + … silent-period floors`
/// and `10·log10(·)` plus the common `/(n_days · 86400)` are monotone.
/// So we rank by `energy * AIRBORNE_RANK_W[period]` (3 ops: index + mul).
/// `W[period]` is the standard Doc 9613 Lden time-weight per period.
const AIRBORNE_RANK_W: [f64; 3] = [
    // day:     12 h × 1.0     ⇒ 12 / 43200 s
    12.0 / 43200.0,
    // evening: 4 h × 10^0.5   ⇒ 4·√10 / 14400 s
    4.0 * 3.162277660168379_f64 / 14400.0,
    // night:   8 h × 10^1.0   ⇒ 80 / 28800 s
    80.0 / 28800.0,
];

/// One entry of the bounded top-K airborne trace heap. Ranks by
/// `rank_key` (linear, monotone with `received_lden.full`) using
/// `f64::total_cmp` so heap pop/peek give a total order even with
/// NaN edge cases.
struct ScoredTrace {
    rank_key: f64,
    /// Input row position: the earlier candidate outranks an equal
    /// `rank_key`, so the kept set is a total order over the input and
    /// identical for the serial walk and any chunking.
    order: u64,
    trace: SegmentTrace,
}

impl ScoredTrace {
    fn outranks(rank_key: f64, order: u64, weakest: &ScoredTrace) -> bool {
        match rank_key.total_cmp(&weakest.rank_key) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => order < weakest.order,
        }
    }
}

impl PartialEq for ScoredTrace {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for ScoredTrace {}
impl PartialOrd for ScoredTrace {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScoredTrace {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank_key
            .total_cmp(&other.rank_key)
            .then_with(|| other.order.cmp(&self.order))
    }
}

/// Scatter the airborne batches onto a per-real-flight accumulator
/// table. Caller passes the resulting flights map to `build_detail` for
/// Doc 29 normalization (`n_days × period_seconds`).
///
/// The receiver terrain horizon is mandatory. The optional building horizon
/// represents an empty local roof skyline, not an alternate propagation path.
///
/// Per-sub-segment `SegmentTrace`s land in `traces.segments` so the
/// Noise Segments popup tab can render one row per Doc 29 SEL call
/// instead of one row per flight (the popup's actual compute unit); a split
/// chord takes one slot and lands as one polyline of pieces (`chords`).
///
/// Rows are independent — a sub-segment only ever touches its own flight's
/// accumulator, whichever square stored it — so the batch list is cut into
/// runs of consecutive batches, one rayon task each, and the private tables
/// are merged in CHUNK ORDER. That order is fixed by the input, so one click
/// gives the same bytes every run; f64 addition is not associative, so the
/// chunked sums differ from a single-threaded walk in the last bits. A popup
/// with fewer than `SCATTER_CHUNK_ROWS` rows stays one chunk and keeps the
/// serial order exactly.
pub fn scatter(
    receiver: &Receiver,
    batches: &[AirborneSegmentBatch<'_>],
    n_days_f: f64,
    // GA hybrid per-class weight LUT.
    // Each row's energy AND its count (`flight_weight`) are multiplied by
    // `class_weights.get(class)` so a GA one-off divides by `ga_n_days`, not
    // `n_days`. Uniform (all-1.0) for non-hybrid extracts.
    class_weights: &aircraft::ClassWeights,
    horizon: &aircraft::ReceiverHorizon,
    buildings: Option<&aircraft::BuildingHorizon>,
    trace_cap: usize,
    traces: Option<&mut TraceCollector>,
) -> HashMap<u64, FlightAccum> {
    let want_traces = traces.is_some();
    let ctx = ScatterContext::new(receiver, n_days_f, class_weights, horizon, buildings);
    let chunks: Vec<ChunkScatter> = chunk_batches(batches)
        .into_par_iter()
        .map(|(range, first_row)| {
            scatter_chunk(
                &ctx,
                &batches[range.clone()],
                range.start,
                first_row,
                trace_cap,
                want_traces,
            )
        })
        .collect();
    merge_chunks(&ctx, batches, chunks, trace_cap, traces)
}

/// Rows a rayon task gathers before the next one starts. A constant, not a
/// thread-count division, so chunk boundaries and therefore every f64
/// summation order are a pure function of the input: the same click gives
/// the same bytes on any machine or pool. Below a few thousand rows the
/// per-chunk `HashMap` + heap allocation and the merge cost more than the
/// split saves, so sparse z14 batches are grouped up to this size.
const SCATTER_CHUNK_ROWS: usize = 4_096;

/// Consecutive batch runs holding at least [`SCATTER_CHUNK_ROWS`] rows (the
/// last run may be shorter), each with the global index of its first row.
fn chunk_batches(batches: &[AirborneSegmentBatch<'_>]) -> Vec<(std::ops::Range<usize>, usize)> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut first_row = 0;
    let mut rows_before = 0;
    for (index, batch) in batches.iter().enumerate() {
        rows_before += batch.len();
        if rows_before - first_row >= SCATTER_CHUNK_ROWS {
            chunks.push((start..index + 1, first_row));
            start = index + 1;
            first_row = rows_before;
        }
    }
    if start < batches.len() {
        chunks.push((start..batches.len(), first_row));
    }
    chunks
}

/// One chunk's private accumulators. Every field recombines associatively
/// (sum / max / min / count), which is what makes the split legal.
struct ChunkScatter {
    flights: HashMap<u64, FlightAccum>,
    /// Bounded top-K heap over this chunk's unsplit rows only. Ranks form a
    /// total order (rank, then input position), so a trace in the global
    /// top-K is inside its own chunk's top-K and merging the heaps drops
    /// nothing.
    heap: BinaryHeap<Reverse<ScoredTrace>>,
    above_cutoff: u32,
    /// Split pieces in row order; their chords are only known after the merge.
    pieces: Vec<PieceEval>,
}

/// Merge the chunk tables in chunk order, fold the split chords, and, when
/// traces were requested, reduce the per-chunk heaps and the chord
/// candidates to one global top-`trace_cap` set.
fn merge_chunks(
    ctx: &ScatterContext<'_>,
    batches: &[AirborneSegmentBatch<'_>],
    chunks: Vec<ChunkScatter>,
    trace_cap: usize,
    traces: Option<&mut TraceCollector>,
) -> HashMap<u64, FlightAccum> {
    use std::collections::hash_map::Entry;

    let mut chunks = chunks.into_iter();
    let Some(first) = chunks.next() else {
        return HashMap::new();
    };
    let mut flights = first.flights;
    let mut above_cutoff = first.above_cutoff;
    let mut pieces = first.pieces;
    // `into_vec` is the heap's backing array: arbitrary order, but a pure
    // function of this chunk's insertion sequence, hence run-to-run stable.
    let mut scored: Vec<ScoredTrace> = first.heap.into_vec().into_iter().map(|r| r.0).collect();
    for chunk in chunks {
        // The per-key f64 order is decided by chunk order, not by this
        // walk: each chunk holds at most one accumulator per flight id.
        for (flight_id, acc) in chunk.flights {
            match flights.entry(flight_id) {
                Entry::Occupied(mut e) => e.get_mut().merge_chunk(acc),
                Entry::Vacant(e) => {
                    e.insert(acc);
                }
            }
        }
        above_cutoff = above_cutoff.saturating_add(chunk.above_cutoff);
        scored.extend(chunk.heap.into_vec().into_iter().map(|r| r.0));
        pieces.extend(chunk.pieces);
    }
    let want_traces = traces.is_some() && trace_cap > 0;
    let (chords, chord_above_cutoff) =
        chords::fold_chords(&pieces, batches, &mut flights, want_traces);
    above_cutoff = above_cutoff.saturating_add(chord_above_cutoff);

    if let Some(t) = traces {
        // One total order (rank, then input position) over unsplit rows and
        // whole chords: the exact global top-K the serial heap keeps.
        // `apply_segment_top_k_with_cap` (source-reader) re-sorts by
        // `received_lden.full` afterwards because road / rail / cruise
        // traces are mixed in.
        enum Candidate {
            Row(Box<SegmentTrace>),
            Chord(ChordCandidate),
        }
        let mut candidates: Vec<(f64, u64, Candidate)> = scored
            .into_iter()
            .map(|s| (s.rank_key, s.order, Candidate::Row(Box::new(s.trace))))
            .chain(
                chords
                    .into_iter()
                    .map(|c| (c.rank_key, c.order, Candidate::Chord(c))),
            )
            .collect();
        if candidates.len() > trace_cap {
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            candidates.truncate(trace_cap);
        }
        t.airborne_above_cutoff = t.airborne_above_cutoff.saturating_add(above_cutoff);
        for (_, _, candidate) in candidates {
            match candidate {
                Candidate::Row(trace) => t.segments.push(*trace),
                Candidate::Chord(chord) => t
                    .segments
                    .extend(chords::chord_traces(ctx, batches, &pieces, &chord)),
            }
        }
    }
    flights
}

/// The Doc 29 kernel loop over one contiguous batch run starting at global
/// batch index `first_batch` and row index `first_row`. Writes only
/// chunk-private state; running it over the whole list is the serial
/// reference implementation the parity test compares against.
fn scatter_chunk(
    ctx: &ScatterContext<'_>,
    batches: &[AirborneSegmentBatch<'_>],
    first_batch: usize,
    first_row: usize,
    trace_cap: usize,
    want_traces: bool,
) -> ChunkScatter {
    let mut flights: HashMap<u64, FlightAccum> = HashMap::new();
    let mut above_cutoff: u32 = 0;
    let mut pieces = Vec::new();
    // Bounded top-K min-heap (size `trace_cap`). We rank by `rank_key`
    // (monotone with received_lden.full) and use `Reverse` so the heap
    // root is the *weakest* kept trace — pop+push replaces it when a
    // stronger candidate arrives. Avoids ~4.4 M `SegmentTrace`
    // allocations at LKPR (only ~150 + a few hundred replacements
    // actually allocate; the rest skip the trace builder entirely).
    let mut heap: BinaryHeap<Reverse<ScoredTrace>> = if want_traces && trace_cap > 0 {
        BinaryHeap::with_capacity(trace_cap)
    } else {
        BinaryHeap::new()
    };

    let mut row_order = first_row as u64;
    for (batch_offset, batch) in batches.iter().enumerate() {
        for i in 0..batch.len() {
            let order = row_order;
            row_order += 1;
            if batch.flags[i] & chords::SPLIT_PIECE != 0 {
                if let Some(row) = evaluate_row::<false>(ctx, batch, i) {
                    pieces.push(PieceEval::new(
                        first_batch + batch_offset,
                        i,
                        order,
                        batch,
                        row,
                    ));
                }
                continue;
            }
            let Some(row) = evaluate_row::<true>(ctx, batch, i) else {
                continue;
            };
            let acc = flight_accumulator(&mut flights, batch, i, row.class_weight);
            // The retained variants deliberately run before the received
            // floor so a strong aircraft hidden by a terrain/building edge
            // still contributes to `periods_free` and its effect deltas.
            row.apply_variants(acc);
            if row.below_event_floor() {
                continue;
            }
            row.apply_received(acc);

            if want_traces && row.lmax >= AIRBORNE_TRACE_CUTOFF_DB {
                // Maintain the "N visible" denominator regardless of
                // whether this sub-seg's trace survives the heap.
                above_cutoff = above_cutoff.saturating_add(1);

                if trace_cap > 0 {
                    let rank_key = row.energy * AIRBORNE_RANK_W[row.period];
                    // Skip the trace builder unless this sub-seg can
                    // displace the weakest kept trace.
                    let should_build = heap.len() < trace_cap
                        || heap
                            .peek()
                            .map(|w| ScoredTrace::outranks(rank_key, order, &w.0))
                            .unwrap_or(true);
                    if should_build {
                        let scored = ScoredTrace {
                            rank_key,
                            order,
                            trace: build_row_trace(ctx, batch, i, &row),
                        };
                        if heap.len() < trace_cap {
                            heap.push(Reverse(scored));
                        } else {
                            heap.pop();
                            heap.push(Reverse(scored));
                        }
                    }
                }
            }
        }
    }
    ChunkScatter {
        flights,
        heap,
        above_cutoff,
        pieces,
    }
}

/// Build airborne-side `AircraftAirborneDetail` and the airborne-only
/// periods (Doc 29 normalized). Walks airborne flights for per-band
/// stats; folds cruise period_energy into the periods total via the
/// separate `cruise_flights` table, whose synth-fid namespace is disjoint.
#[allow(clippy::too_many_arguments)]
pub fn build_detail(
    flights: &HashMap<u64, FlightAccum>,
    cruise_flights: &HashMap<u64, FlightAccum>,
    cruise_transit_count: usize,
    top_flight_candidates: &HashMap<u64, TopFlightCandidate>,
    cruise_band_stats: &[BandStats; 3],
    n_days_f: f64,
    // GA-class window for the popup's per-class "Data" row;
    // equals `n_days_f` for non-hybrid extracts.
    ga_n_days_f: f64,
) -> (
    NoisePeriods,
    NoisePeriods,
    ImpactDeltas,
    AircraftAirborneDetail,
) {
    use crate::periods;

    let mut airborne_energy = [0.0f64; 3];
    let mut free_airborne_energy = [0.0f64; 3];
    let mut no_terrain_airborne_energy = [0.0f64; 3];
    let mut no_screening_airborne_energy = [0.0f64; 3];
    let mut band_faint = BandStats::new();
    let mut band_audible = BandStats::new();
    let mut band_disruptive = BandStats::new();
    let mut helicopter_count = 0.0f64;
    let mut global_peak_lmax = f64::NEG_INFINITY;

    // Cruise period_energy folds into the airborne total — the popup
    // exposes a single Aircraft Lden, not separate cruise / airborne
    // numbers. Cruise band counters come via `cruise_band_stats`
    // (real-fid dedup) so we don't iterate cruise here for those.
    // Ascending flight_id, not HashMap order: f64 addition is not
    // associative, so the iteration order is part of the number. See
    // `crate::compute::key_sorted` for why sorting beats a fixed hasher here.
    for (_, acc) in crate::compute::key_sorted(cruise_flights) {
        // Period accumulation (day/evening/night) — `p` indexes both sides; the
        // f64 sum order across flights is part of popup byte parity.
        #[allow(clippy::needless_range_loop)]
        for p in 0..3 {
            airborne_energy[p] += acc.period_energy[p];
            free_airborne_energy[p] += acc.free_period_energy[p];
            no_terrain_airborne_energy[p] += acc.no_terrain_period_energy[p];
            no_screening_airborne_energy[p] += acc.no_screening_period_energy[p];
        }
        if acc.peak_lmax > global_peak_lmax {
            global_peak_lmax = acc.peak_lmax;
        }
    }

    // Sampling-fragility accumulators (see AircraftAirborneDetail docs):
    // real airborne flights only — cruise buckets are aggregate-stable by
    // construction and synthetic fids carry no date.
    //
    // BTreeMap, not HashMap: the `max_by` below picks the loudest day and
    // `max_by` returns the LAST maximum, so a HashMap's iteration order
    // would decide ties. At most one entry per sampled day (≤ 365).
    let mut energy_by_day: std::collections::BTreeMap<u32, f64> = std::collections::BTreeMap::new();
    let mut max_flight_energy = 0.0f64;

    // Sorted once and reused by `build_top_flights` below — the top-20
    // selection needs the same deterministic order and would otherwise
    // sort the same map a second time.
    let flights_by_id = crate::compute::key_sorted(flights);

    for &(&flight_id, acc) in flights_by_id.iter() {
        // Period accumulation — same byte-parity sum order as the cruise loop above.
        #[allow(clippy::needless_range_loop)]
        for p in 0..3 {
            airborne_energy[p] += acc.period_energy[p];
            free_airborne_energy[p] += acc.free_period_energy[p];
            no_terrain_airborne_energy[p] += acc.no_terrain_period_energy[p];
            no_screening_airborne_energy[p] += acc.no_screening_period_energy[p];
        }
        let flight_energy: f64 = acc.period_energy.iter().sum();
        if flight_energy <= 0.0 {
            continue;
        }
        if acc.peak_lmax > global_peak_lmax {
            global_peak_lmax = acc.peak_lmax;
        }
        if acc.is_cruise {
            continue;
        }
        if let crate::flight_id::FlightIdKind::Real { start_unix, .. } =
            crate::flight_id::unpack(flight_id)
        {
            *energy_by_day.entry(start_unix / 86_400).or_default() += flight_energy;
            max_flight_energy = max_flight_energy.max(flight_energy);
        }
        if aircraft::is_helicopter_profile(acc.profile_idx) {
            helicopter_count += acc.flight_weight / n_days_f;
        }
        let cls = aircraft::noise_class_of(acc.profile_idx) as usize;
        let weight = acc.flight_weight.round().max(1.0) as u32;
        // Band stats want average altitude per event, not CPA distance.
        // Feeding `min_dist_m` into `alt_sum` would report CPA values
        // labelled as altitude. Use the peak-encounter altitude
        // weighted by flight_weight to match cruise.rs band stats.
        let alt_w_sum = acc.peak_altitude_m * acc.flight_weight;
        if acc.peak_lmax > 30.0 {
            band_faint.add_event(acc.flight_weight, alt_w_sum, cls, weight);
            if acc.peak_lmax > 45.0 {
                band_audible.add_event(acc.flight_weight, alt_w_sum, cls, weight);
                if acc.peak_lmax > 60.0 {
                    band_disruptive.add_event(acc.flight_weight, alt_w_sum, cls, weight);
                }
            }
        }
    }

    // Cruise band counters routed via the dedicated cruise dedup table —
    // `cruise.rs::scatter` populated `cruise_band_stats` per band so a
    // single transit crossing many grid cells counts once per band.
    for (band, cruise) in [&mut band_faint, &mut band_audible, &mut band_disruptive]
        .into_iter()
        .zip(cruise_band_stats.iter())
    {
        band.count += cruise.count;
        band.alt_sum += cruise.alt_sum;
        for k in 0..aircraft::NUM_CLASSES {
            band.class_counts[k] += cruise.class_counts[k];
        }
    }

    let periods_from_energy = |energy: [f64; 3]| {
        if energy.iter().sum::<f64>() > 0.0 {
            let ld = aircraft::period_leq(energy[0], n_days_f, aircraft::PERIOD_SECONDS[0]);
            let le = aircraft::period_leq(energy[1], n_days_f, aircraft::PERIOD_SECONDS[1]);
            let ln = aircraft::period_leq(energy[2], n_days_f, aircraft::PERIOD_SECONDS[2]);
            periods::periods(ld, le, ln)
        } else {
            NoisePeriods::silence()
        }
    };
    let airborne_periods = periods_from_energy(airborne_energy);
    let airborne_periods_free = periods_from_energy(free_airborne_energy);
    let periods_no_terrain = periods_from_energy(no_terrain_airborne_energy);
    let periods_no_screening = periods_from_energy(no_screening_airborne_energy);
    let impact = |variant: &NoisePeriods| {
        if airborne_periods.lden_db.is_finite() && variant.lden_db.is_finite() {
            (airborne_periods.lden_db - variant.lden_db).min(0.0)
        } else {
            0.0
        }
    };
    let impacts = ImpactDeltas {
        terrain: impact(&periods_no_terrain),
        screening: impact(&periods_no_screening),
        ..Default::default()
    };

    let observed_flights_per_day: f64 = flights_by_id
        .iter()
        .map(|&(_, f)| f)
        .filter(|f| !f.is_cruise && f.period_energy.iter().sum::<f64>() > 0.0)
        .map(|f| f.flight_weight / n_days_f)
        .sum();
    // Cruise transits seen at this receiver, real-fid deduped (count
    // passed in by `compute_aircraft_v6` from `cruise_flight_stats`).
    // Distinct from `observed_flights_per_day` (airborne only). Acts as
    // a context counter for the Lmax band rows: cruise transits whose
    // `peak_lmax` crosses 30/45/60 dB inflate band counts above the
    // airborne flight count, and naming the cruise total separately
    // makes that delta legible. (Below-threshold cruise transits are
    // included here but don't enter the bands — the row is therefore
    // an upper bound on the cruise contribution, not an exact remainder.)
    let cruise_transits_per_day = cruise_transit_count as f64 / n_days_f;

    let total_airborne_energy: f64 = airborne_energy.iter().sum();
    let top_flights =
        build_top_flights(&flights_by_id, top_flight_candidates, total_airborne_energy);

    let (top_day_energy_share, top_day_date) = energy_by_day
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .filter(|_| total_airborne_energy > 0.0)
        .map(|(&day, &e)| (e / total_airborne_energy, date_from_unix(day * 86_400)))
        .unwrap_or((0.0, String::new()));
    let top_flight_energy_share = if total_airborne_energy > 0.0 {
        max_flight_energy / total_airborne_energy
    } else {
        0.0
    };

    let to_band = |band: &BandStats| AircraftEventBandStats {
        observed_events_per_day: band.count / n_days_f,
        avg_altitude_m: if band.count > 0.0 {
            band.alt_sum / band.count
        } else {
            0.0
        },
        top_aircraft: band.top_type().to_string(),
    };
    let detail = AircraftAirborneDetail {
        periods: airborne_periods.clone(),
        observed_flights_per_day,
        helicopter_flights_per_day: helicopter_count,
        cruise_transits_per_day,
        lmax_peak: if global_peak_lmax > -900.0 {
            Some(global_peak_lmax)
        } else {
            None
        },
        faint: to_band(&band_faint),
        audible: to_band(&band_audible),
        disruptive: to_band(&band_disruptive),
        top_day_energy_share: round3(top_day_energy_share),
        top_day_date,
        top_flight_energy_share: round3(top_flight_energy_share),
        sample_days: n_days_f as u32,
        ga_sample_days: ga_n_days_f as u32,
        top_flights,
    };

    (airborne_periods, airborne_periods_free, impacts, detail)
}

/// Top-N flights by `peak_lmax` interleaving airborne (`flights`,
/// real fid) and cruise (`cruise_candidates`, real fid). Bounded
/// insertion sort so a busy airport doesn't pay the full `O(n log n)`.
/// Cruise rows show `energy_pct = 0` because the bucket aggregates many
/// real fids and per-fid energy split would be artificial.
fn build_top_flights(
    flights_by_id: &[(&u64, &FlightAccum)],
    cruise_candidates: &HashMap<u64, TopFlightCandidate>,
    total_airborne_energy: f64,
) -> Vec<AircraftTopFlight> {
    use std::cmp::Ordering;

    // Rank on plain scalars first, build the rows afterwards. The previous
    // bounded insertion sort built a full `AircraftTopFlight` — five String
    // allocations (callsign, typecode, profile name, date, ICAO hex) — for
    // EVERY flight and then dropped all but 20. São Paulo carries ~95 k
    // airborne flights, so ~475 k allocations were made to be discarded.
    //
    // `is_cruise` (0 = airborne, 1 = cruise) is a SELECTION tiebreak, not a
    // display field: the insertion sort fed airborne candidates first and
    // kept the first arrival at an equal `peak_lmax`, so airborne won a tie
    // for the last slot. (lmax desc, is_cruise asc, fid asc) reproduces
    // that, and since a fid appears in at most one of the two groups it is
    // a TOTAL order — which is also why the cruise map below can be walked
    // in hash order without costing reproducibility.
    let mut cands: Vec<(f64, u8, u64)> =
        Vec::with_capacity(flights_by_id.len() + cruise_candidates.len());
    for &(&fid, acc) in flights_by_id.iter() {
        if acc.is_cruise {
            continue;
        }
        let flight_energy: f64 = acc.period_energy.iter().sum();
        if flight_energy <= 0.0 || acc.peak_lmax <= -900.0 {
            continue;
        }
        cands.push((acc.peak_lmax, 0, fid));
    }
    for (&fid, cand) in cruise_candidates.iter() {
        if cand.peak_lmax <= -900.0 {
            continue;
        }
        // Same real fid in both maps means the flight had both an
        // airborne sub-segment encounter and a cruise bucket encounter
        // in receiver radius — keep the airborne entry (sub-segment-level
        // granularity, real `energy_pct`) and skip the cruise dup.
        if flights_by_id
            .binary_search_by_key(&&fid, |&(k, _)| k)
            .is_ok()
        {
            continue;
        }
        cands.push((cand.peak_lmax, 1, fid));
    }

    if cands.len() > TOP_FLIGHTS_N {
        cands.select_nth_unstable_by(TOP_FLIGHTS_N, |a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        cands.truncate(TOP_FLIGHTS_N);
    }
    // Stable total order for DISPLAY: descending peak_lmax, ascending fid
    // as final tiebreak so equal-Lmax + equal-callsign rows don't fall back
    // to map iteration order. Provenance deliberately does not enter here —
    // it only decides which rows survive the cut above.
    cands.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });

    cands
        .into_iter()
        .filter_map(|(_, is_cruise, fid)| {
            if is_cruise == 1 {
                return cruise_candidates
                    .get(&fid)
                    .map(|cand| cruise_top_flight_entry(fid, cand));
            }
            let idx = flights_by_id
                .binary_search_by_key(&&fid, |&(k, _)| k)
                .ok()?;
            let acc = flights_by_id[idx].1;
            let flight_energy: f64 = acc.period_energy.iter().sum();
            let energy_pct = if total_airborne_energy > 0.0 {
                flight_energy / total_airborne_energy * 100.0
            } else {
                0.0
            };
            Some(airborne_top_flight_entry(fid, acc, energy_pct))
        })
        .collect()
}

fn airborne_top_flight_entry(fid: u64, acc: &FlightAccum, energy_pct: f64) -> AircraftTopFlight {
    let (icao_hex, start_unix) = crate::flight_id::icao_hex_and_start_unix(fid);
    let synthetic = start_unix.is_none();
    AircraftTopFlight {
        lmax_db: round1(acc.peak_lmax),
        cpa_distance_m: round1(acc.min_dist_m),
        altitude_m: round1(acc.peak_altitude_m),
        period: acc.peak_period,
        date: date_from_id(acc.peak_date_id),
        profile: aircraft::PROFILES[aircraft::clamp_profile_idx(acc.profile_idx)]
            .name
            .to_string(),
        aircraft_type: aircraft::typecode_to_string(&acc.aircraft_type),
        callsign: acc.callsign.clone(),
        energy_pct: round1(energy_pct),
        geometry: [
            [acc.peak_seg_start[0], acc.peak_seg_start[1]],
            [acc.peak_seg_end[0], acc.peak_seg_end[1]],
        ],
        icao_hex,
        start_unix,
        synthetic,
    }
}

fn cruise_top_flight_entry(fid: u64, cand: &TopFlightCandidate) -> AircraftTopFlight {
    let (icao_hex, start_unix) = crate::flight_id::icao_hex_and_start_unix(fid);
    // `date` is the flight's start_unix-derived date (when ADS-B first
    // saw the flight, ~= takeoff); not the overflight encounter time.
    // Cruise scatter has no per-encounter timestamp — Stage 2B
    // aggregates by grid cell, dropping individual sample timing.
    let date = start_unix.map(date_from_unix).unwrap_or_default();
    AircraftTopFlight {
        lmax_db: round1(cand.peak_lmax),
        cpa_distance_m: round1(cand.min_dist_m),
        altitude_m: round1(cand.peak_altitude_m),
        period: cand.peak_period,
        date,
        profile: aircraft::PROFILES[aircraft::clamp_profile_idx(cand.profile_idx)]
            .name
            .to_string(),
        aircraft_type: aircraft::typecode_to_string(&cand.aircraft_type),
        callsign: cand.callsign.clone(),
        // Cruise bucket aggregates many real fids; per-fid energy share
        // would be artificial, so we report 0 instead of fabricating one.
        energy_pct: 0.0,
        geometry: [
            [cand.peak_seg_start[0], cand.peak_seg_start[1]],
            [cand.peak_seg_end[0], cand.peak_seg_end[1]],
        ],
        icao_hex,
        start_unix,
        synthetic: start_unix.is_none(),
    }
}

#[inline]
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[inline]
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

use super::dates::{date_from_id, date_from_unix};

#[cfg(test)]
mod tests;
