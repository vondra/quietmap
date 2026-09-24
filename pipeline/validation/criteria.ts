/**
 * W1 acceptance criteria as data: cohort membership frozen at the baseline run and each
 * station's uncertainty budget (u_i, its cohort-band part and the one-sided allowance).
 */
import { readFileSync } from 'node:fs'
import type { IndicatorComparison } from './comparison.ts'
import type { StationRow } from './report.ts'

type Valued<T> = { value: T; provenance: string }
export type CriteriaCohort = {
  id: string
  role: 'gated' | 'reported' | 'diagnostic' | 'named_gap'
  compare: string
  membership: Record<string, unknown>
  metrics_gated?: string[]
  metrics_reported?: string[]
}
export type Criteria = {
  criteria_version: string
  constants: {
    n_min: Valued<number>; tau_db: Valued<number>; move_explain_db: Valued<number>; tail_thresholds_db: Valued<[number, number]>
    objective_tail6_max_share: Valued<number>; objective_tail10_max_share: Valued<number>; u_method_db: Valued<number>
    layer_relevance_db: Valued<number>; upper_bound_factor: Valued<number>
  }
  bootstrap: { draws: number; level: number; seed: number }
  membership: { order: string[] }
  metrics: { periods_gated: string[]; periods_reported: string[] }
  uncertainty_model: { components: {
    instrument: { class2_or_unknown_db: number }
    window: { months_to_db: Record<string, number> }
    year_gap: { k_definition: string; station_db: Record<string, number>; network_mean_db: Record<string, number>; diagnostic_years: number[] }
    period: Record<string, unknown>
    receiver_convention: { free_field_db: number; facade_0_5_to_2m_correction_db: number; facade_flush_correction_db: number; u_when_corrected_db: number; unknown: string }
    height: { unknown: string }
  } }
  cohorts: CriteriaCohort[]
}

export function loadCriteria(path: string): Criteria {
  const criteria = JSON.parse(readFileSync(path, 'utf8')) as Criteria
  for (const id of criteria.membership.order) if (!criteria.cohorts.some(cohort => cohort.id === id)) throw new Error(`criteria: no cohort ${id}`)
  return criteria
}

/** A number the criteria state inside a sentence; a missing one fails loudly rather than defaulting. */
function stated(text: string, pattern: RegExp, what: string): number {
  const match = pattern.exec(text)
  if (!match) throw new Error(`criteria: cannot read ${what} from ${JSON.stringify(text)}`)
  return Number(match[1])
}

/** Criteria metric names → the runner's comparison periods. */
export const METRIC_PERIODS: Record<string, string> = { lden: 'lden', ln: 'night', ld: 'day', le: 'evening' }

/**
 * The catalogue states each publisher's source class (`expected_source`); the criteria's site
 * classes add whether an aircraft value is event-classified (a guard) or the total near an airport.
 */
export function siteClass(row: StationRow): string | null {
  const byExpectedSource: Record<string, string> = {
    road: 'traffic', nightlife: 'nightlife', pedestrian_zone: 'pedestrian', other: 'activity', park: 'park_quiet',
    rail: 'rail_trackside', industry: 'industry',
  }
  if (row.expected_source === 'aircraft') return row.guard ? 'aircraft_nmt' : 'airport_area'
  return byExpectedSource[row.expected_source] ?? null
}

function matches(row: StationRow, condition: string, value: unknown): boolean {
  const road = row.model?.dominant_road ?? null
  const list = value as unknown[]
  switch (condition) {
    case 'site_class': return list.includes(siteClass(row))
    case 'network': return list.some(prefix => row.set.startsWith(String(prefix)))
    case 'dominance': return value === 'event_classified' && row.guard != null
    case 'dominant_road.max_distance_m': return road != null && road.distance_m <= Number(value)
    case 'dominant_road.min_distance_m_exclusive': return road == null || road.distance_m > Number(value)
    case 'dominant_road.traffic_provenance': return road != null && list.includes(road.traffic_provenance)
    case 'dominant_road.class': return road != null && list.includes(road.road_class)
    default: throw new Error(`criteria: unsupported membership condition ${condition}`)
  }
}

/** Criteria §3: first matching cohort in the declared order; stations outside every cohort say why. */
export function membership(row: StationRow, criteria: Criteria): { cohort: string | null; reason: string } {
  if (row.error) return { cohort: null, reason: `popup failed: ${row.error}` }
  if (row.truth_kind === 'official_map_modelled') return { cohort: null, reason: 'cross_check (modelled official map)' }
  if (row.measurand === 'traffic_count') return { cohort: null, reason: 'input_truth (traffic count)' }
  if (row.scoring === 'diagnostic_only') return { cohort: null, reason: `diagnostic: ${row.scoring_reason ?? ''}` }
  for (const id of criteria.membership.order) {
    const cohort = criteria.cohorts.find(entry => entry.id === id)!
    if (Object.entries(cohort.membership).every(([condition, value]) => matches(row, condition, value))) return { cohort: id, reason: 'matched' }
  }
  return { cohort: null, reason: `no cohort for site class ${siteClass(row) ?? `unknown (${row.expected_source})`}` }
}

/** Linear interpolation in a {k: dB} table, capped at its largest k. */
function interpolate(table: Record<string, number>, k: number): number {
  const points = Object.entries(table).map(([key, value]) => [Number(key), value] as const).sort((a, b) => a[0] - b[0])
  const capped = Math.min(k, points.at(-1)![0])
  const upper = points.findIndex(([key]) => key >= capped)
  if (points[upper][0] === capped || upper === 0) return points[upper][1]
  const [k0, v0] = points[upper - 1]
  const [k1, v1] = points[upper]
  return v0 + (v1 - v0) * (capped - k0) / (k1 - k0)
}

export type Uncertainty = {
  eligible: boolean
  flags: string[]
  correction_db: number
  one_sided_allowance_db: number
  components: { instrument: number; window: number; year_gap: number; period: number; receiver_convention: number; height: number; position: number | null }
  /** u_drift² + u_period² + u_conv² + u_height² of the cohort band U_c. */
  band_square: number
  u_i: number
}

/** Standard deviation of the model Lden over the nominal point and the outdoor offset points. */
export function positionSpread(row: StationRow): number | null {
  const levels = [row.model?.total.lden ?? null, ...(row.position_samples ?? []).filter(sample => !sample.inside_footprint).map(sample => sample.lden)]
    .filter((level): level is number => level != null)
  if (!row.position_samples?.length || levels.length < 2) return null
  const average = levels.reduce((sum, level) => sum + level, 0) / levels.length
  return Math.sqrt(levels.reduce((sum, level) => sum + (level - average) ** 2, 0) / (levels.length - 1))
}

/** Criteria §4 budget of one station and metric, computed once from the baseline run. */
export function uncertainty(row: StationRow, comparison: IndicatorComparison, criteria: Criteria): Uncertainty {
  const parts = criteria.uncertainty_model.components
  const flags: string[] = []
  let eligible = row.months_covered == null || row.months_covered >= 9
  const windowTerm = parts.window.months_to_db[String(Math.min(row.months_covered ?? 12, 12))] ?? 0
  if (row.year != null && parts.year_gap.diagnostic_years.includes(row.year)) eligible = false
  const defaultInputYear = stated(parts.year_gap.k_definition, /else \|measurement year - (\d{4})\|/, 'the default input year')
  const inputYear = row.model?.dominant_layer === 'road' ? row.model.dominant_road?.dataset_year ?? defaultInputYear : defaultInputYear
  const k = Math.abs((row.year ?? defaultInputYear) - inputYear)

  const shiftedKey = Object.keys(parts.period).find(key => key.startsWith('shifted_end_periods_'))
  const shifted = shiftedKey ? /(\d{2})_(\d{2})_(\d{2})$/.exec(shiftedKey) : null
  let periodTerm = 0
  if (comparison.period_mapping === 'piecewise_constant') {
    const [day, evening, night] = shifted ? shifted.slice(1) : []
    const terms = parts.period[shiftedKey ?? ''] as { lden_db: number; ln_db: number } | undefined
    if (terms && comparison.windows === `${day}-${evening}/${evening}-${night}/${night}-${day}`) periodTerm = terms.lden_db
    else if (terms && comparison.windows === `${night}-${day}`) periodTerm = terms.ln_db
    else flags.push(`no period term for windows ${comparison.windows}`)
  }

  const convention = parts.receiver_convention
  let correction = 0
  let conventionTerm = 0
  let allowance = 0
  const documentedFacade = row.mount === 'facade' && row.facade_distance_m != null && row.publisher_facade_correction_db == null
  if (documentedFacade && row.facade_distance_m! >= 0.5 && row.facade_distance_m! <= 2) {
    correction = convention.facade_0_5_to_2m_correction_db
    conventionTerm = convention.u_when_corrected_db
  } else if (documentedFacade && row.facade_distance_m! < 0.5) {
    correction = convention.facade_flush_correction_db
    conventionTerm = convention.u_when_corrected_db
  } else if (row.mount === 'unknown' || row.mount === 'facade') {
    // A facade microphone whose publisher may already have corrected counts as an unknown convention.
    allowance = stated(convention.unknown, /a_i = ([\d.]+) dB/, 'the one-sided allowance')
    if (row.mount === 'facade') flags.push('facade correction ambiguous (publisher states one)')
  }

  const unknownHeight = stated(parts.height.unknown, /assume ([\d.]+) m/, 'the assumed height')
  const againstHeight = stated(parts.height.unknown, /term against ([\d.]+) m/, 'the comparison height')
  const layer = comparison.layer === null || comparison.layer === 'non_aircraft' ? null : comparison.layer
  const source = layer ? row.model?.loudest_by_layer[layer] : row.model?.contributors[0]
  const distance = source?.distance_m ?? 0
  const divergence = (a: number, b: number) => distance > 0 ? Math.abs(10 * Math.log10(Math.hypot(distance, a) / Math.hypot(distance, b))) : 0
  const used = row.receiver_height_used_m ?? unknownHeight
  const heightTerm = row.mic_height_m == null ? divergence(againstHeight, unknownHeight) : divergence(row.mic_height_m, used)
  if (row.mic_height_m != null && Math.abs(row.mic_height_m - used) > 1e-9) flags.push('height_pending')

  const position = positionSpread(row)
  if (position == null) flags.push(row.position_uncertainty_m == null ? 'position uncertainty undocumented' : 'position spread not sampled')
  const components = {
    instrument: parts.instrument.class2_or_unknown_db, window: windowTerm, year_gap: interpolate(parts.year_gap.station_db, k),
    period: periodTerm, receiver_convention: conventionTerm, height: heightTerm, position,
  }
  const u = Math.sqrt(Object.values(components).reduce<number>((sum, value) => sum + (value ?? 0) ** 2, 0))
  const drift = interpolate(parts.year_gap.network_mean_db, k)
  return {
    eligible, flags, correction_db: correction, one_sided_allowance_db: allowance, components,
    band_square: drift ** 2 + periodTerm ** 2 + conventionTerm ** 2 + heightTerm ** 2, u_i: u,
  }
}
