//! Stable per-cell stage evidence emitted by every warm CPU/GPU tile engine.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

pub const ENGINE_SPANS_PREFIX: &str = "engine-spans-v1 ";
pub const ENGINE_SPANS_SCHEMA: u8 = 1;
// The Hub envelope adds attempt identity, producer clocks and JSON framing. Keep the bare engine
// payload below 192 KiB so the final archived event always fits its 256 KiB line contract.
pub const ENGINE_SPANS_LINE_LIMIT_BYTES: usize = 192 * 1024;

/// The shared stage vocabulary. Every serialized report contains every key; an absent
/// measurement is JSON `null`, never a fabricated zero. Composite stages name an existing
/// boundary that cannot yet be split without changing the hot path.
pub const ENGINE_STAGE_NAMES: [&str; 16] = [
    "source_load",
    "raster",
    "cpu_prepare",
    "candidate_prepare",
    "pack",
    "queue",
    "vram_gate",
    "accumulator_init",
    "h2d",
    "gpu_kernel",
    "d2h",
    "gpu_pipeline_composite",
    "cpu_scatter",
    "encode",
    "write",
    "encode_write_composite",
];

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

fn offset_ns(cell_started: Instant, instant: Instant) -> u64 {
    duration_ns(
        instant
            .checked_duration_since(cell_started)
            .unwrap_or(Duration::ZERO),
    )
}

/// One measured interval. Host spans carry process-monotonic offsets. Aggregate counters whose
/// original code retained only a duration deliberately leave both offsets null.
#[derive(Clone, Debug)]
pub struct EngineStageSpan {
    start_offset_ns: Option<u64>,
    end_offset_ns: Option<u64>,
    active_ns: Option<u64>,
    calls: Option<u64>,
    bytes: Option<u64>,
    component: Option<String>,
    clock: &'static str,
}

impl EngineStageSpan {
    pub fn host(
        cell_started: Instant,
        started: Instant,
        ended: Instant,
        calls: Option<u64>,
        bytes: Option<u64>,
        component: Option<&str>,
    ) -> Self {
        Self {
            start_offset_ns: Some(offset_ns(cell_started, started)),
            end_offset_ns: Some(offset_ns(cell_started, ended)),
            active_ns: Some(duration_ns(
                ended
                    .checked_duration_since(started)
                    .unwrap_or(Duration::ZERO),
            )),
            calls,
            bytes,
            component: component.map(str::to_string),
            clock: "host-monotonic",
        }
    }

    /// A duration already accumulated by existing production code. Its placement on the cell
    /// timeline is unknown, so only `active_ns` is populated.
    pub fn aggregate(
        active: Duration,
        calls: Option<u64>,
        bytes: Option<u64>,
        component: Option<&str>,
    ) -> Self {
        Self {
            start_offset_ns: None,
            end_offset_ns: None,
            active_ns: Some(duration_ns(active)),
            calls,
            bytes,
            component: component.map(str::to_string),
            clock: "host-aggregate",
        }
    }

    /// Isolated device-active duration obtained from CUDA events. CUDA and host monotonic clocks
    /// have no calibrated common epoch, so offsets remain null instead of inventing alignment.
    pub fn cuda_active(active: Duration, calls: Option<u64>, component: Option<&str>) -> Self {
        Self {
            start_offset_ns: None,
            end_offset_ns: None,
            active_ns: Some(duration_ns(active)),
            calls,
            bytes: None,
            component: component.map(str::to_string),
            clock: "cuda-event",
        }
    }

    fn json(&self) -> Value {
        json!({
            "start_offset_ns": self.start_offset_ns,
            "end_offset_ns": self.end_offset_ns,
            "active_ns": self.active_ns,
            "calls": self.calls,
            "bytes": self.bytes,
            "component": self.component,
            "clock": self.clock,
        })
    }
}

/// One cell's engine-local evidence. The box agent adds the authoritative group/lease/epoch
/// envelope when it archives this line; engines intentionally do not invent another attempt ID.
pub struct EngineCellSpans {
    cell: String,
    engine: &'static str,
    slot: usize,
    cell_started: Instant,
    wall_ns: Option<u64>,
    terminal_status: &'static str,
    terminal_error: Option<String>,
    tiles_written: Option<u64>,
    tiles_skipped: Option<u64>,
    output_bytes: Option<u64>,
    stages: BTreeMap<&'static str, Vec<EngineStageSpan>>,
    metrics: Map<String, Value>,
}

impl EngineCellSpans {
    pub fn new(r4: u64, engine: &'static str, slot: usize, cell_started: Instant) -> Self {
        Self {
            cell: format!("{r4:x}"),
            engine,
            slot,
            cell_started,
            wall_ns: None,
            terminal_status: "running",
            terminal_error: None,
            tiles_written: None,
            tiles_skipped: None,
            output_bytes: None,
            stages: BTreeMap::new(),
            metrics: Map::new(),
        }
    }

    pub fn cell_started(&self) -> Instant {
        self.cell_started
    }

    pub fn push_host_span(
        &mut self,
        stage: &'static str,
        started: Instant,
        ended: Instant,
        calls: Option<u64>,
        bytes: Option<u64>,
        component: Option<&str>,
    ) {
        self.push_span(
            stage,
            EngineStageSpan::host(self.cell_started, started, ended, calls, bytes, component),
        );
    }

    pub fn push_aggregate_span(
        &mut self,
        stage: &'static str,
        active: Duration,
        calls: Option<u64>,
        bytes: Option<u64>,
        component: Option<&str>,
    ) {
        self.push_span(
            stage,
            EngineStageSpan::aggregate(active, calls, bytes, component),
        );
    }

    pub fn push_cuda_span(
        &mut self,
        stage: &'static str,
        active: Duration,
        calls: Option<u64>,
        component: Option<&str>,
    ) {
        self.push_span(
            stage,
            EngineStageSpan::cuda_active(active, calls, component),
        );
    }

    fn push_span(&mut self, stage: &'static str, span: EngineStageSpan) {
        debug_assert!(
            ENGINE_STAGE_NAMES.contains(&stage),
            "unknown engine stage {stage}"
        );
        self.stages.entry(stage).or_default().push(span);
    }

    pub fn metric(&mut self, name: impl Into<String>, value: Value) {
        self.metrics.insert(name.into(), value);
    }

    pub fn metric_u64(&mut self, name: impl Into<String>, value: u64) {
        self.metric(name, json!(value));
    }

    pub fn metric_bool(&mut self, name: impl Into<String>, value: bool) {
        self.metric(name, json!(value));
    }

    pub fn metric_str(&mut self, name: impl Into<String>, value: &str) {
        self.metric(name, json!(value));
    }

    pub fn finish_done(
        &mut self,
        wall: Duration,
        tiles_written: usize,
        tiles_skipped: usize,
        output_bytes: Option<usize>,
    ) {
        self.wall_ns = Some(duration_ns(wall));
        self.terminal_status = "done";
        self.tiles_written = Some(tiles_written as u64);
        self.tiles_skipped = Some(tiles_skipped as u64);
        self.output_bytes = output_bytes.map(|n| n as u64);
    }

    pub fn finish_failed(&mut self, wall: Duration, error: &str) {
        self.wall_ns = Some(duration_ns(wall));
        self.terminal_status = "fail";
        // A pathological anyhow chain must never breach the archive's bounded-line contract.
        self.terminal_error = Some(error.chars().take(2048).collect());
    }

    fn value(&self, oversized_original_bytes: Option<usize>) -> Value {
        let mut stages = Map::new();
        for name in ENGINE_STAGE_NAMES {
            stages.insert(
                name.to_string(),
                self.stages.get(name).map_or(Value::Null, |spans| {
                    Value::Array(spans.iter().map(EngineStageSpan::json).collect())
                }),
            );
        }
        json!({
            "schema": ENGINE_SPANS_SCHEMA,
            "cell": self.cell,
            "engine": self.engine,
            "slot": self.slot,
            "wall_ns": self.wall_ns,
            "terminal": {
                "status": self.terminal_status,
                "error": self.terminal_error,
            },
            "output": {
                "tiles_written": self.tiles_written,
                "tiles_skipped": self.tiles_skipped,
                "bytes": self.output_bytes,
            },
            "stages": stages,
            "metrics": self.metrics,
            "oversized_original_bytes": oversized_original_bytes,
        })
    }

    /// One newline-free protocol line. The normal fixed-stage report is only a few KiB. A future
    /// caller that accidentally adds an unbounded metric gets a small explicit fallback instead
    /// of wedging the engine stdout reader or consuming the Hub envelope's 256 KiB line budget.
    pub fn line(&self) -> String {
        let full = format!("{ENGINE_SPANS_PREFIX}{}", self.value(None));
        if full.len() < ENGINE_SPANS_LINE_LIMIT_BYTES {
            return full;
        }
        let fallback = json!({
            "schema": ENGINE_SPANS_SCHEMA,
            "cell": self.cell,
            "engine": self.engine,
            "slot": self.slot,
            "wall_ns": self.wall_ns,
            "terminal": {
                "status": self.terminal_status,
                "error": "engine span report exceeded bounded line size",
            },
            "output": {
                "tiles_written": self.tiles_written,
                "tiles_skipped": self.tiles_skipped,
                "bytes": self.output_bytes,
            },
            "stages": Value::Null,
            "metrics": Value::Null,
            "oversized_original_bytes": full.len(),
        });
        format!("{ENGINE_SPANS_PREFIX}{fallback}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Value {
        assert!(line.starts_with(ENGINE_SPANS_PREFIX));
        serde_json::from_str(&line[ENGINE_SPANS_PREFIX.len()..]).unwrap()
    }

    #[test]
    fn unmeasured_stages_are_explicit_nulls() {
        let started = Instant::now();
        let mut report = EngineCellSpans::new(0x841e309ffffffff, "cpu-surface", 3, started);
        report.push_aggregate_span(
            "source_load",
            Duration::from_millis(12),
            Some(7),
            Some(1234),
            None,
        );
        report.metric_bool("cuda_event_timing_enabled", false);
        report.finish_done(Duration::from_millis(20), 4, 2, Some(456));
        let value = parse(&report.line());
        assert_eq!(value["schema"], 1);
        assert_eq!(value["cell"], "841e309ffffffff");
        assert_eq!(value["output"]["bytes"], 456);
        assert_eq!(value["stages"]["gpu_kernel"], Value::Null);
        assert_eq!(value["stages"]["source_load"][0]["active_ns"], 12_000_000);
        assert_eq!(value["metrics"]["cuda_event_timing_enabled"], false);
    }

    #[test]
    fn host_and_cuda_clocks_are_not_falsely_aligned() {
        let cell_started = Instant::now();
        let stage_started = cell_started + Duration::from_millis(2);
        let stage_ended = stage_started + Duration::from_millis(3);
        let mut report = EngineCellSpans::new(
            0x841e309ffffffff,
            "relevant-source-surface",
            0,
            cell_started,
        );
        report.push_host_span("raster", stage_started, stage_ended, Some(1), None, None);
        report.push_cuda_span("gpu_kernel", Duration::from_micros(750), Some(2), None);
        let value = parse(&report.line());
        assert_eq!(value["stages"]["raster"][0]["start_offset_ns"], 2_000_000);
        assert_eq!(
            value["stages"]["gpu_kernel"][0]["start_offset_ns"],
            Value::Null
        );
        assert_eq!(value["stages"]["gpu_kernel"][0]["clock"], "cuda-event");
    }

    #[test]
    fn worst_case_supported_line_stays_below_archive_limit_and_parses() {
        let started = Instant::now();
        let mut report =
            EngineCellSpans::new(0x841e309ffffffff, "relevant-source-surface", 1, started);
        // More intervals than today's engines emit, including the two-layer source path.
        for stage in ENGINE_STAGE_NAMES {
            for i in 0..64 {
                let a = started + Duration::from_micros(i * 20);
                report.push_host_span(
                    stage,
                    a,
                    a + Duration::from_micros(10),
                    Some(1),
                    Some(1 << 20),
                    Some("road-and-rail"),
                );
            }
        }
        report.finish_done(Duration::from_secs(1), 10_000, 2_000, Some(123_456_789));
        let line = report.line();
        assert!(line.len() < ENGINE_SPANS_LINE_LIMIT_BYTES, "{}", line.len());
        let value = parse(&line);
        assert_eq!(value["oversized_original_bytes"], Value::Null);
        assert_eq!(value["stages"]["gpu_kernel"].as_array().unwrap().len(), 64);
    }

    #[test]
    fn unbounded_future_metric_fails_small_and_loud() {
        let started = Instant::now();
        let mut report = EngineCellSpans::new(0x841e309ffffffff, "cpu-aircraft", 0, started);
        report.metric(
            "bad_future_metric",
            json!("x".repeat(ENGINE_SPANS_LINE_LIMIT_BYTES)),
        );
        let line = report.line();
        assert!(line.len() < ENGINE_SPANS_LINE_LIMIT_BYTES);
        let value = parse(&line);
        assert!(value["oversized_original_bytes"].as_u64().unwrap() > 0);
        assert_eq!(value["stages"], Value::Null);
    }
}
