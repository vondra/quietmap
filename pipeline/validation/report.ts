/**
 * One validation run's station rows and its Markdown run report: identity, latency, every station
 * comparison, guards, traffic counts, diagnostics and caveats. Criteria statistics live in evaluate.ts.
 */
import type { GuardResult, IndicatorComparison } from './comparison.ts'
import type { StationModel } from './popup.ts'
import type { PositionSample } from './receiver.ts'
import { quantile } from './statistics.ts'

export type StationRow = {
  key: string
  set: string
  station_id: string
  name: string
  lat: number
  lng: number
  expected_source: string
  /** The catalogue's OSM identity of a guard's source (`type/id`). */
  guard_osm: string | null
  site_class: string | null
  physical_station_id: string | null
  instrument_class: string | null
  truth_kind: string
  measurand: string
  year: number | null
  months_covered: number | null
  /** The station lies in a holdout z9 square (rule v1, `z9_holdout_square`). */
  holdout_square: boolean
  diagnostic_only: boolean
  diagnostic_reason: string | null
  native_periods: Record<string, unknown> | null
  mount: string | null
  facade_distance_m: number | null
  publisher_facade_correction_db: number | null
  publisher_facade_correction_applied: boolean | null
  position_uncertainty_m: number | null
  mic_height_m: number | null
  requested_receiver_height_m: number | null
  /** Where the model value was computed; an indoor popup value is never scored. */
  receiver: { lat: number; lng: number; basis: 'station point, outdoors' | 'interior rule: nearest outline moved outward'
    moved_m: number; position_radius_m: number } | null
  receiver_height_used_m: number | null
  height_matches_microphone: boolean
  request_ms: number
  model: StationModel | null
  comparisons: IndicatorComparison[]
  guard: GuardResult | null
  /** Model totals on the circle of the position radius (criteria u_position). */
  position_samples: PositionSample[] | null
  unsupported_indicators: string[]
  /** Why an answered station cannot be scored (indoor receiver, unavailable layers). */
  unscored: string | null
  error: string | null
}

/** A guard point of the criteria (not a catalogue station) and the popup there. */
export type GuardPointRow = { guard: string; label: string; lat: number; lng: number; model: StationModel | null; error: string | null }

type Comparison = { row: StationRow; comparison: IndicatorComparison }

function comparisons(rows: StationRow[]): Comparison[] {
  return rows.flatMap(row => row.comparisons.map(comparison => ({ row, comparison })))
}

export function latency(rows: StationRow[]): { n: number; p50_ms: number; p95_ms: number; max_ms: number } | null {
  const sorted = rows.filter(row => !row.error).map(row => row.request_ms).sort((a, b) => a - b)
  if (sorted.length === 0) return null
  return { n: sorted.length, p50_ms: Math.round(quantile(sorted, 0.5)), p95_ms: Math.round(quantile(sorted, 0.95)), max_ms: sorted.at(-1)! }
}

const shown = (value: number | null) => value == null ? '-' : String(value)
const measuredText = (comparison: IndicatorComparison) => comparison.band ? `[${shown(comparison.band[0])}, ${shown(comparison.band[1])}]` : shown(comparison.measured)

function comparisonList(title: string, entries: Comparison[]): string[] {
  return [
    `## ${title}`, '',
    '| station | indicator | layer | measured | model | Δ | unit | mapping | model dominant layer |', '| --- | --- | --- | --- | --- | --- | --- | --- | --- |',
    ...entries.map(({ row, comparison }) => `| ${row.key} | ${comparison.indicator} | ${comparison.layer ?? 'all'} | ${measuredText(comparison)} | `
      + `${shown(comparison.model)} | ${shown(comparison.delta_db)} | ${comparison.unit} | ${comparison.period_mapping} | ${row.model?.dominant_layer ?? '-'} |`),
    '',
  ]
}

export function renderReport(rows: StationRow[], identity: Record<string, unknown>, runSeconds: number): string {
  const timing = latency(rows)
  const failed = rows.filter(row => row.error)
  const all = comparisons(rows)
  const inside = rows.filter(row => row.receiver?.basis === 'interior rule: nearest outline moved outward')
  const heightMismatch = rows.filter(row => !row.error && !row.height_matches_microphone)
  const guardRows = rows.filter(row => row.guard)
  const guardCell = (guard: GuardResult) => guard.loudest_in_expected_layer
    ? `${guard.loudest_in_expected_layer.osm_id ?? ''} ${guard.loudest_in_expected_layer.name} (${guard.loudest_in_expected_layer.distance_m} m, ${guard.loudest_in_expected_layer.received_lden} dB)` : '-'
  return [
    `# Validation run ${String(identity.label ?? '')}`, '',
    `Stations ${rows.length} (failed ${failed.length}; unscored ${rows.filter(row => row.unscored).length}; diagnostic only ${rows.filter(row => row.diagnostic_only).length}); `
      + `comparisons ${all.length}; run ${runSeconds.toFixed(0)} s; `
      + (timing ? `popup latency p50 ${timing.p50_ms} ms, p95 ${timing.p95_ms} ms, max ${timing.max_ms} ms (${String(identity.concurrency ?? '')} concurrent, cold cache).` : 'no popups.'),
    'Δ = model − measurement as published (a class band: distance outside it); criteria corrections, cohorts and '
      + 'statistics are in the evaluation (evaluate.ts).', '',
    ...comparisonList('Every station comparison', all.filter(({ comparison }) => comparison.kind !== 'traffic' && comparison.period_mapping !== 'diagnostic')),
    '### Guards (source identity and shares)', '',
    '| station | expected layer | expected source | passed | expected layer share | named source share of layer | model dominant layer | loudest contributor of the layer | reason |',
    '| --- | --- | --- | --- | --- | --- | --- | --- | --- |',
    ...guardRows.map(row => `| ${row.key} | ${row.guard!.expected.layer ?? '-'} | ${row.guard!.expected.name ?? '-'}${row.guard!.expected.identity_verified_by_catalogue ? '' : ' (unverified)'} | `
      + `${row.guard!.passed ? 'yes' : 'NO'} | ${row.guard!.expected_layer_share} | ${row.guard!.identity_share ?? '-'} | ${row.guard!.dominant_layer ?? '-'} | ${guardCell(row.guard!)} | ${row.guard!.reason} |`),
    '',
    ...comparisonList('Traffic counts against the dominant road (flow ratio in dB, heavy share in points)',
      all.filter(({ comparison }) => comparison.kind === 'traffic')),
    ...comparisonList('Background percentiles (diagnostic; never a lower bound on modelled levels)',
      all.filter(({ comparison }) => comparison.period_mapping === 'diagnostic')),
    `Interior stations (never scored indoors; receiver on the nearest outline moved outward): ${inside.length} — `
      + inside.map(row => `${row.key} ${row.receiver!.moved_m} m`).join(', '), '',
    `Unscored: ${rows.filter(row => row.unscored).map(row => `${row.key} (${row.unscored})`).join(', ') || 'none'}.`, '',
    `Publisher facade correction stated but not known to be applied: ${rows.filter(row => row.publisher_facade_correction_db != null).length} stations.`,
    `Receiver height differs from the microphone or the microphone height is unknown: ${heightMismatch.length} of ${rows.length - failed.length}.`,
    `Windows splitting an END period: ${all.filter(({ comparison }) => comparison.period_mapping === 'piecewise_constant').length} exact on `
      + `period-constant layers, ${all.filter(({ comparison }) => comparison.period_mapping === 'aircraft_split').length} cutting aircraft energy (diagnostic); `
      + `END hours assumed where the catalogue names none: ${all.filter(({ comparison }) => comparison.periods_assumed_end).length}.`,
    `Unsupported indicators: ${rows.flatMap(row => row.unsupported_indicators.map(entry => `${row.key} ${entry}`)).join('; ') || 'none'}.`,
    `Failed stations: ${failed.map(row => `${row.key} (${row.error})`).join(', ') || 'none'}.`, '',
  ].join('\n')
}
