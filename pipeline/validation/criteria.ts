/**
 * W1 criteria v2 as data: the JSON's constants, membership order, provenance map, convention
 * variants and uncertainty tables, read once with loud failures for anything unreadable.
 */
import { readFileSync } from 'node:fs'
import type { StationRow } from './report.ts'

type Valued = { value: number }
type ByMetric = { lden: Record<string, number>; ln: Record<string, number> }
export type CriteriaCohort = { id: string; role: string; metrics_gated?: string[] | null; metrics_reported?: string[] | null }
export type CriteriaGuard = {
  id: string; lat?: number; lng?: number; points?: Array<[number, number, string]>; layer: string; members?: string[]
  subchecks: Array<{ id: string; predicate: string; quantity_db?: string }>
}
export type Criteria = {
  criteria_version: string
  constants: Record<'tau_db' | 'n_min' | 'move_explain_db' | 'severe_db' | 'large_error_db' | 'large_error_net_share' | 'upper_bound_k'
    | 'u_method_db' | 'layer_relevance_db' | 'default_position_radius_m' | 'default_facade_offset_m' | 'convention_alternative_db', Valued>
    & { bootstrap: { draws: number; seed: number } }
  panel: {
    membership: { order: string[]; provenance_class: Record<'measured' | 'class_default' | 'service_tree' | 'transferred', string>; major_classes: string[]; dominant_road: string }
  }
  uncertainty: {
    instrument: { class1: number; class2_or_unknown: number }
    window_months_to_db: ByMetric
    year_gap: { k: string; interpolate: string; station_rms_db: ByMetric; network_mean_rms_db: ByMetric }
    period: { end_variant_06_18_22: { correction_to_end_equivalent_db: { lden: number; ln: number }; u_db: { lden: number; ln: number } } }
    convention: { u_when_minus3_applied_db: number }
    height: { pending_term: string }
  }
  cohorts: CriteriaCohort[]
  guards: CriteriaGuard[]
}

export function loadCriteria(path: string): Criteria {
  return JSON.parse(readFileSync(path, 'utf8')) as Criteria
}

/** A number the criteria state inside a sentence; a missing one fails loudly rather than defaulting. */
export function stated(text: string, pattern: RegExp, what: string): string {
  const match = pattern.exec(text)
  if (!match) throw new Error(`criteria: cannot read ${what} from ${JSON.stringify(text)}`)
  return match[1]
}

/** The two metrics the tables of criteria v2 carry; Ld and Le use the Lden tables. */
export type TableMetric = 'lden' | 'ln'
export const tableMetric = (metric: string): TableMetric => metric === 'ln' ? 'ln' : 'lden'
export const METRIC_PERIODS: Record<string, string> = { lden: 'lden', ln: 'night', ld: 'day', le: 'evening' }

/** `panel.membership.order`: site class → cohort, first match wins; `road -> road cohorts` defers to the road rules. */
export function siteClassCohorts(criteria: Criteria): Array<{ classes: string[]; cohort: string }> {
  return criteria.panel.membership.order.map(entry => {
    const [classes, cohort] = entry.split('->').map(part => part.trim())
    if (!classes || !cohort) throw new Error(`criteria: unreadable membership entry ${JSON.stringify(entry)}`)
    return { classes: classes.split(',').map(value => value.trim()), cohort }
  })
}

/**
 * The catalogue's `site_class` when present; until w1-catalogue adds it everywhere, the
 * publisher-evidenced `expected_source` in the criteria vocabulary (never a model attribute).
 */
export function siteClass(row: StationRow): { site_class: string; basis: string } {
  if (row.site_class) return { site_class: row.site_class, basis: 'catalogue site_class' }
  const byExpectedSource: Record<string, string> = {
    road: 'road', nightlife: 'nightlife', pedestrian_zone: 'pedestrian', other: 'activity', park: 'park_quiet',
    rail: 'rail_trackside', industry: 'industry', unknown: 'unknown',
  }
  const derived = row.expected_source === 'aircraft' ? row.guard ? 'aircraft_nmt' : 'airport_area' : byExpectedSource[row.expected_source] ?? 'unknown'
  return { site_class: derived, basis: `catalogue expected_source ${row.expected_source}` }
}

export type ProvenanceClass = 'measured' | 'class_default' | 'service_tree' | 'transferred'

/** `panel.membership.provenance_class` applied to the dominant road's popup fields. */
export function provenanceClass(criteria: Criteria, road: NonNullable<StationRow['model']>['dominant_road']): ProvenanceClass | null {
  if (!road) return null
  const rules = criteria.panel.membership.provenance_class
  const measuredTiers = stated(rules.measured, /\{([^}]*)\}/, 'the measured tiers').split(',').map(value => value.trim())
  const lightBit = 1 << (Number(stated(rules.measured, /bit (\d+)/, 'the light-traffic bit')) - 1)
  const defaultIds = [...rules.class_default.matchAll(/dominant_source_id == (\d+)/g)].map(match => Number(match[1]))
  const defaultTiers = [...rules.class_default.matchAll(/tier == ([\w-]+)/g)].map(match => match[1])
  const serviceTreeId = Number(stated(rules.service_tree, /dominant_source_id == (\d+)/, 'the service-tree id'))
  const lightEstimated = road.traffic_estimated == null || (road.traffic_estimated & lightBit) !== 0
  if (road.provenance_tier && measuredTiers.includes(road.provenance_tier) && !lightEstimated) return 'measured'
  if ((road.dominant_source_id != null && defaultIds.includes(road.dominant_source_id) && road.provenance_tier == null)
    || (road.provenance_tier != null && defaultTiers.includes(road.provenance_tier))) return 'class_default'
  if (road.dominant_source_id === serviceTreeId) return 'service_tree'
  return 'transferred'
}

/** Criteria v2 road cohorts from the dominant road's distance, provenance class and road class. */
export function roadCohort(criteria: Criteria, road: NonNullable<StationRow['model']>['dominant_road']): string {
  const reach = Number(stated(criteria.panel.membership.dominant_road, /> (\d+) m/, 'the dominant-road reach'))
  if (!road || road.distance_m > reach) return 'R-far'
  const provenance = provenanceClass(criteria, road)
  if (provenance === 'measured') return 'R-counted'
  if (provenance === 'service_tree') return 'R-local'
  if (provenance === 'class_default') return road.road_class && criteria.panel.membership.major_classes.includes(road.road_class) ? 'R-default-major' : 'R-default-minor'
  return 'R-transferred'
}

export type Variant = 'as_published' | 'minus3'

/** Criteria v2 convention variants: a facade or unknown mount is scored as published and −3 dB unless documented. */
export function conventionVariants(row: StationRow): Variant[] {
  if (row.mount === 'free_field' || row.mount === 'pole' || row.mount === 'roof') return ['as_published']
  if (row.mount === 'facade' && row.publisher_facade_correction_applied === true) return ['as_published']
  if (row.mount === 'facade' && row.publisher_facade_correction_applied === false) return ['minus3']
  return ['as_published', 'minus3']
}

/** Linear interpolation in a {k: dB} table (k = 0 gives 0 dB unless the table says otherwise). */
export function interpolate(table: Record<string, number>, k: number): number {
  const points = [[0, table['0'] ?? 0] as const, ...Object.entries(table).map(([key, value]) => [Number(key), value] as const)]
    .sort((a, b) => a[0] - b[0])
  if (k >= points.at(-1)![0]) return points.at(-1)![1]
  const upper = points.findIndex(([key]) => key >= k)
  if (points[upper][0] === k) return points[upper][1]
  const [k0, v0] = points[upper - 1]
  const [k1, v1] = points[upper]
  return v0 + (v1 - v0) * (k - k0) / (k1 - k0)
}

/** Web Mercator tile of a point (criteria site clusters use z15, holdout squares z9). */
export function tile(lat: number, lng: number, zoom: number): { x: number; y: number } {
  const n = 2 ** zoom
  const x = Math.floor((lng + 180) / 360 * n)
  const y = Math.floor((1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * n)
  return { x: Math.min(Math.max(x, 0), n - 1), y: Math.min(Math.max(y, 0), n - 1) }
}
