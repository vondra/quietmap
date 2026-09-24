/**
 * Summaries and the Markdown report of one validation run: error statistics by truth kind ×
 * category × road cohort or traffic provenance × period, guards, traffic, diagnostics, latency.
 */
import type { GuardResult, IndicatorComparison } from './comparison.ts'
import type { StationModel } from './popup.ts'
import { quantile, summarizeGroups, type ErrorSummary } from './statistics.ts'

export type StationRow = {
  key: string
  set: string
  station_id: string
  name: string
  lat: number
  lng: number
  expected_source: string
  truth_kind: string
  measurand: string
  holdout: boolean
  /** The station lies in a holdout z9 square (rule v1); acoustic data never fit anything anyway. */
  holdout_square: boolean
  /** 'accuracy' rows enter the statistics; diagnostic and trend-only rows are listed apart. */
  scoring: 'accuracy' | 'diagnostic_only' | 'trend_only'
  scoring_reason: string | null
  mount: string | null
  publisher_facade_correction_db: number | null
  position_uncertainty_m: number | null
  mic_height_m: number | null
  requested_receiver_height_m: number | null
  receiver_height_used_m: number | null
  height_matches_microphone: boolean
  request_ms: number
  model: StationModel | null
  comparisons: IndicatorComparison[]
  guard: GuardResult | null
  unsupported_indicators: string[]
  error: string | null
}

type Comparison = { row: StationRow; comparison: IndicatorComparison }
export type SummaryTable = { dimensions: string[]; groups: Array<{ group: string; summary: ErrorSummary }> }

/** Through-traffic classes of criteria v1 §3 (R-default-major) and the minor classes (R-default-minor). */
const MAJOR_ROAD_CLASSES = new Set(['motorway', 'trunk', 'primary', 'motorway_link', 'trunk_link', 'primary_link'])
const MINOR_ROAD_CLASSES = new Set(['secondary', 'tertiary', 'unclassified', 'secondary_link', 'tertiary_link'])
/** Criteria v1 §3: the dominant road decides a traffic station's cohort only within 50 m. */
const DOMINANT_ROAD_REACH_M = 50
const SITE_COHORTS: Record<string, string> = {
  park: 'Q-quiet', nightlife: 'N-nightlife', pedestrian_zone: 'N-pedestrian', other: 'N-activity', unknown: 'U-unclassified',
  rail: 'T-rail', industry: 'I-industry',
}

/** Criteria v1 §3 membership: site class first, then the dominant road for traffic stations. */
export function cohort(row: StationRow): string {
  if (row.truth_kind === 'official_map_modelled') return `X-map-${row.expected_source}`
  if (row.measurand === 'traffic_count') return 'input-road-count'
  if (row.expected_source === 'aircraft') return row.comparisons.some(comparison => comparison.layer === 'aircraft') ? 'A-nmt' : 'A-area'
  if (row.expected_source !== 'road') return SITE_COHORTS[row.expected_source] ?? `other-${row.expected_source}`
  const road = row.model?.dominant_road
  if (!road || road.distance_m > DOMINANT_ROAD_REACH_M) return 'R-far'
  if (road.traffic_provenance === 'counted') return 'R-counted'
  if (road.traffic_provenance === 'service_tree') return 'R-local'
  if (road.traffic_provenance !== 'class_default') return 'R-transferred'
  if (road.road_class && MAJOR_ROAD_CLASSES.has(road.road_class)) return 'R-default-major'
  if (road.road_class && MINOR_ROAD_CLASSES.has(road.road_class)) return 'R-default-minor'
  return 'R-default-local-class'
}

function comparisons(rows: StationRow[]): Comparison[] {
  return rows.flatMap(row => row.comparisons.map(comparison => ({ row, comparison })))
}

/** Acoustic accuracy comparisons: scored rows, dB deltas (bands included), no traffic or diagnostics. */
function accuracyComparisons(rows: StationRow[]): Comparison[] {
  return comparisons(rows.filter(row => row.scoring === 'accuracy')).filter(({ comparison }) => comparison.unit === 'dB'
    && comparison.delta_db != null && (comparison.period_mapping === 'exact' || comparison.period_mapping === 'piecewise_constant'))
}

const period = (comparison: IndicatorComparison) => `${comparison.period}${comparison.layer ? ` (${comparison.layer})` : ''}`

export function summarize(rows: StationRow[]): Record<string, SummaryTable> {
  const scored = accuracyComparisons(rows)
  const sample = ({ row, comparison }: Comparison) => ({ delta: comparison.delta_db!, network: row.set })
  const table = (dimensions: string[], key: (entry: Comparison) => string | null): SummaryTable =>
    ({ dimensions, groups: summarizeGroups(scored, key, sample) })
  return {
    by_cohort: table(['cohort', 'period (layer)'], ({ row, comparison }) => `${cohort(row)} | ${period(comparison)}`),
    by_cohort_holdout: table(['cohort', 'z9 holdout square', 'period (layer)'],
      ({ row, comparison }) => `${cohort(row)} | ${row.holdout_square ? 'holdout' : 'training'} | ${period(comparison)}`),
    by_cohort_network: table(['cohort', 'network', 'period (layer)'], ({ row, comparison }) => `${cohort(row)} | ${row.set} | ${period(comparison)}`),
    by_road_provenance: table(['model dominant road provenance', 'road class', 'period (layer)'],
      ({ row, comparison }) => row.model?.dominant_layer === 'road' && row.model.dominant_road
        ? `${row.model.dominant_road.traffic_provenance} | ${row.model.dominant_road.road_class ?? '-'} | ${period(comparison)}` : null),
    by_model_dominant_layer: table(['model dominant layer', 'period (layer)'],
      ({ row, comparison }) => `${row.model?.dominant_layer ?? 'none'} | ${period(comparison)}`),
    by_set: table(['network', 'period (layer)'], ({ row, comparison }) => `${row.set} | ${period(comparison)}`),
  }
}

export function latency(rows: StationRow[]): { n: number; p50_ms: number; p95_ms: number; max_ms: number } | null {
  const sorted = rows.filter(row => !row.error).map(row => row.request_ms).sort((a, b) => a - b)
  if (sorted.length === 0) return null
  return { n: sorted.length, p50_ms: Math.round(quantile(sorted, 0.5)), p95_ms: Math.round(quantile(sorted, 0.95)), max_ms: sorted.at(-1)! }
}

const estimate = (value: { value: number; ci95: [number, number] | null }, percent = false): string => {
  const scale = percent ? 100 : 1
  const digits = percent ? 0 : 1
  const text = (number: number) => (number * scale).toFixed(digits)
  return value.ci95 ? `${text(value.value)} [${text(value.ci95[0])}, ${text(value.ci95[1])}]` : text(value.value)
}

function statisticsTable(title: string, table: SummaryTable): string[] {
  return [
    `### ${title}`, '',
    `| ${table.dimensions.join(' | ')} | n | bias dB | MAE dB | >3 dB % | >6 dB % | >10 dB % | P10 dB | P90 dB |`,
    `|${' --- |'.repeat(table.dimensions.length + 8)}`,
    ...table.groups.map(({ group, summary }) => `| ${group} | ${summary.n} | ${estimate(summary.bias)} | ${estimate(summary.mae)} | `
      + `${estimate(summary.share_beyond_3db, true)} | ${estimate(summary.share_beyond_6db, true)} | ${estimate(summary.share_beyond_10db, true)} | `
      + `${estimate(summary.p10)} | ${estimate(summary.p90)} |`),
    '',
  ]
}

const shown = (value: number | null) => value == null ? '-' : String(value)
const measuredText = (comparison: IndicatorComparison) => comparison.band ? `[${shown(comparison.band[0])}, ${shown(comparison.band[1])}]` : shown(comparison.measured)

function comparisonList(title: string, entries: Comparison[]): string[] {
  return [
    `### ${title}`, '',
    '| station | indicator | layer | measured | model | Δ | unit | mapping | model dominant layer |', '| --- | --- | --- | --- | --- | --- | --- | --- | --- |',
    ...entries.map(({ row, comparison }) => `| ${row.key} | ${comparison.indicator} | ${comparison.layer ?? 'all'} | ${measuredText(comparison)} | `
      + `${shown(comparison.model)} | ${shown(comparison.delta_db)} | ${comparison.unit} | ${comparison.period_mapping} | ${row.model?.dominant_layer ?? '-'} |`),
    '',
  ]
}

export function renderReport(rows: StationRow[], identity: Record<string, unknown>, runSeconds: number): string {
  const tables = summarize(rows)
  const timing = latency(rows)
  const failed = rows.filter(row => row.error)
  const all = comparisons(rows)
  const inside = rows.filter(row => row.model?.inside_footprint)
  const heightMismatch = rows.filter(row => !row.error && !row.height_matches_microphone)
  const guardRows = rows.filter(row => row.guard)
  const guardCell = (guard: GuardResult) => guard.loudest_in_expected_layer
    ? `${guard.loudest_in_expected_layer.osm_id ?? ''} ${guard.loudest_in_expected_layer.name} (${guard.loudest_in_expected_layer.distance_m} m, ${guard.loudest_in_expected_layer.received_lden} dB)` : '-'
  return [
    `# Validation run ${String(identity.label ?? '')}`, '',
    `Stations ${rows.length} (failed ${failed.length}; accuracy ${rows.filter(row => row.scoring === 'accuracy').length}, `
      + `diagnostic only ${rows.filter(row => row.scoring === 'diagnostic_only').length}, trend only ${rows.filter(row => row.scoring === 'trend_only').length}); `
      + `comparisons ${all.length}; run ${runSeconds.toFixed(0)} s; `
      + (timing ? `popup latency p50 ${timing.p50_ms} ms, p95 ${timing.p95_ms} ms, max ${timing.max_ms} ms (${String(identity.concurrency ?? '')} concurrent, cold cache).` : 'no popups.'),
    'Δ = model − measurement at the station (a class band: distance outside it). Statistics are provisional until '
      + 'criteria.json is evaluated; intervals are percentile bootstrap 95 % (10,000 draws resampled within each network, '
      + 'seed 20260924, ≥ 3 samples). Accuracy tables hold accuracy-scored stations, dB deltas and windows the model can evaluate.', '',
    ...statisticsTable('By cohort (criteria v1 §3 membership) and period', tables.by_cohort),
    ...statisticsTable('By cohort, holdout square and period', tables.by_cohort_holdout),
    ...statisticsTable('By cohort, network and period (network strata)', tables.by_cohort_network),
    ...statisticsTable('Road-dominated stations by the dominant road\'s traffic provenance and class', tables.by_road_provenance),
    ...statisticsTable('By the model\'s dominant layer', tables.by_model_dominant_layer),
    ...statisticsTable('By network', tables.by_set),
    '### Guards (source identity and shares)', '',
    '| station | expected layer | expected source | passed | expected layer share | model dominant layer | loudest contributor of the layer | reason |',
    '| --- | --- | --- | --- | --- | --- | --- | --- |',
    ...guardRows.map(row => `| ${row.key} | ${row.guard!.expected.layer ?? '-'} | ${row.guard!.expected.name ?? '-'}${row.guard!.expected.identity_verified_by_catalogue ? '' : ' (unverified)'} | `
      + `${row.guard!.passed ? 'yes' : 'NO'} | ${row.guard!.expected_layer_share} | ${row.guard!.dominant_layer ?? '-'} | ${guardCell(row.guard!)} | ${row.guard!.reason} |`),
    '',
    ...comparisonList('Traffic counts against the dominant road (flow ratio in dB, heavy share in points)',
      all.filter(({ comparison }) => comparison.kind === 'traffic')),
    ...comparisonList('Trend only (listed, never pooled)', all.filter(({ row }) => row.scoring === 'trend_only')),
    ...comparisonList('Diagnostic-only stations', all.filter(({ row }) => row.scoring === 'diagnostic_only')),
    ...comparisonList('Background percentiles (diagnostic; never a lower bound on modelled levels)',
      all.filter(({ comparison }) => comparison.period_mapping === 'diagnostic')),
    `Inside a footprint (facade level restored; receiver at the nearest facade exit, finding #2): ${inside.length} — `
      + inside.map(row => `${row.key} ${row.model!.receiver.click_to_receiver_m} m`).join(', '), '',
    `Publisher facade correction stated but not known to be applied: ${rows.filter(row => row.publisher_facade_correction_db != null).length} stations (scored as published).`,
    `Receiver height differs from the microphone or the microphone height is unknown: ${heightMismatch.length} of ${rows.length - failed.length}.`,
    `Windows splitting an END period: ${all.filter(({ comparison }) => comparison.period_mapping === 'piecewise_constant').length} exact on `
      + `period-constant layers, ${all.filter(({ comparison }) => comparison.period_mapping === 'aircraft_split').length} cutting aircraft energy (diagnostic); `
      + `END hours assumed where the catalogue names none: ${all.filter(({ comparison }) => comparison.periods_assumed_end).length}.`,
    `Unsupported indicators: ${rows.flatMap(row => row.unsupported_indicators.map(entry => `${row.key} ${entry}`)).join('; ') || 'none'}.`,
    `Failed stations: ${failed.map(row => `${row.key} (${row.error})`).join(', ') || 'none'}.`, '',
  ].join('\n')
}
