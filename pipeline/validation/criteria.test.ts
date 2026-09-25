/** Criteria v2 as data: provenance map, frozen panel, paired rules, guard predicates and outcomes. */
import assert from 'node:assert/strict'
import test from 'node:test'
import type { IndicatorComparison } from './comparison.ts'
import { conventionVariants, roadCohort, type Criteria } from './criteria.ts'
import { evaluateGuardSubchecks } from './guards.ts'
import { freezeManifest } from './manifest.ts'
import type { StationModel } from './popup.ts'
import type { StationRow } from './report.ts'
import { changeRules, targetMet } from './rules.ts'
import { gate } from './verdict.ts'

const valued = (value: number) => ({ value })
const criteria: Criteria = {
  criteria_version: 'fixture',
  constants: {
    tau_db: valued(0.6), n_min: valued(10), move_explain_db: valued(3), severe_db: valued(10), large_error_db: valued(6),
    large_error_net_share: valued(0.1), upper_bound_k: valued(2), u_method_db: valued(1.5), layer_relevance_db: valued(10),
    default_position_radius_m: valued(15), default_facade_offset_m: valued(2), convention_alternative_db: valued(-3),
    bootstrap: { draws: 300, seed: 20260924 },
  },
  panel: { membership: {
    order: ['nightlife -> N-nightlife', 'park_quiet, background -> Q-quiet', 'unknown -> U-unclassified', 'road -> road cohorts'],
    provenance_class: {
      measured: 'metadata.provenance.tier in {city-measured, national-measured, continental-measured, global-measured} whatever the traffic_estimated class bits',
      class_default: 'metadata.dominant_source_id == 0 (provenance null) OR tier == national-proxy',
      service_tree: 'metadata.dominant_source_id == 11 ("Service-tree residential flow heuristic")',
      transferred: 'everything else',
    },
    major_classes: ['motorway', 'trunk', 'primary'],
    dominant_road: 'distance = metadata.dominant_distance_m; none or > 50 m -> R-far',
  } },
  uncertainty: {
    instrument: { class1: 0.5, class2_or_unknown: 1.5 },
    window_months_to_db: { lden: { 12: 0, 11: 0.34, 10: 0.49, 9: 0.62 }, ln: { 12: 0, 11: 0.53, 10: 0.76, 9: 0.96 } },
    year_gap: {
      k: '|measurement year - metadata.provenance.year of the dominant input| when present, else |measurement year - 2026| with k_assumed = true',
      interpolate: 'linear in k; k > 10 -> diagnostic',
      station_rms_db: { lden: { 1: 1.02, 2: 1.23, 10: 2.23 }, ln: { 1: 1.29, 2: 1.59, 10: 2.73 } },
      network_mean_rms_db: { lden: { 1: 0.32, 2: 0.42, 10: 1.44 }, ln: { 1: 0.39, 2: 0.55, 10: 1.72 } },
    },
    period: { end_variant_06_18_22: { correction_to_end_equivalent_db: { lden: 0.03, ln: 0.36 }, u_db: { lden: 0.07, ln: 0.26 } } },
    convention: { u_when_minus3_applied_db: 0.87 },
    height: { pending_term: '|10 lg(r(h_mic)/r(4 m))|, r from the dominant line source (road 0.05 m, rail 0.5 m)' },
  },
  cohorts: [
    { id: 'R-counted', role: 'gated', metrics_gated: ['lden', 'ln'], metrics_reported: ['ld', 'le'] },
    { id: 'R-transferred', role: 'reported (gated like any cohort once n >= n_min)', metrics_gated: ['lden', 'ln'] },
    { id: 'U-unclassified', role: 'upper_bound_only', metrics_gated: ['upper_bound on lden, ln'] },
  ],
  guards: [],
}

type RoadFields = { tier?: string | null; estimated?: number; source?: number; distance?: number; road_class?: string; year?: number | null }
const road = (fields: RoadFields = {}) => ({
  osm_id: 7, name: 'street', road_class: fields.road_class ?? 'secondary', distance_m: fields.distance ?? 8, aadt: { light: 1, medium: 0, heavy: 0, moto: 0, total: 1 },
  traffic_estimated: fields.estimated ?? 8, vehicle_split_estimated: false, dominant_source_id: fields.source ?? 9003,
  provenance_tier: fields.tier === undefined ? 'city-measured' : fields.tier, dataset_name: 'counts', dataset_year: fields.year === undefined ? 2025 : fields.year,
  speed_source: 'osm_posted', traffic_provenance: 'measured_count' as const,
})
const comparison = (indicator: string, period: string, model: number, measured: number, windows: string, mapping: IndicatorComparison['period_mapping']): IndicatorComparison => ({
  indicator, period, kind: period === 'lden' ? 'weighted' : 'window', layer: null, measured, band: null, windows, model, delta_db: model - measured,
  unit: 'dB', period_mapping: mapping, periods_assumed_end: false,
})
function row(key: string, lden: number, fields: Partial<StationRow> & { road?: RoadFields; measured?: number } = {}): StationRow {
  const layer = { lden, periods: { day: lden - 2, evening: lden - 3, night: lden - 8 }, share_lden: 1 }
  const model = {
    total: { lden, periods: layer.periods }, layers: { road: layer }, dominant_layer: 'road', unavailable_layers: [],
    contributors: [{ source_type: 'road', osm_id: 7, name: 'street', subtype: 'secondary', distance_m: 8, received_lden: lden, aadt_total: 1, dataset_name: 'counts' }],
    loudest_by_layer: {}, dominant_road: road(fields.road), receiver: { lat: 0, lng: 0, height_m: 4, click_to_receiver_m: 0 },
  } as unknown as StationModel
  const measured = fields.measured ?? 62
  return {
    key, set: key.split('/')[0], station_id: key, name: key, lat: 40 + key.length * 0.01, lng: 2, expected_source: 'road', guard_osm: null, site_class: 'road',
    physical_station_id: null, instrument_class: null, truth_kind: 'measured', measurand: 'sound_level', year: 2025, months_covered: 12,
    holdout_square: false, diagnostic_only: false, diagnostic_reason: null, native_periods: null, mount: 'free_field', facade_distance_m: null,
    publisher_facade_correction_db: null, publisher_facade_correction_applied: null, position_uncertainty_m: null, mic_height_m: 4,
    requested_receiver_height_m: 4, receiver: { lat: 40, lng: 2, basis: 'station point, outdoors', moved_m: 0, position_radius_m: 15 },
    receiver_height_used_m: 4, height_matches_microphone: true, request_ms: 1, model, guard: null, position_samples: null,
    comparisons: [comparison('Lden', 'lden', lden, measured, '07-19/19-23/23-07', 'exact'), comparison('Lnight', 'night', lden - 8, measured - 8, '23-07', 'exact')],
    unsupported_indicators: [], unscored: null, error: null, ...fields,
  }
}

test('the provenance map sends counted, default, service-tree and transferred roads to their cohorts', () => {
  assert.equal(roadCohort(criteria, road()), 'R-counted')
  assert.deepEqual(conventionVariants(row('nmt/unknown', 60, { site_class: 'aircraft_nmt', mount: null })), ['as_published'])
  assert.deepEqual(conventionVariants(row('road/unknown', 60, { mount: null })), ['as_published', 'minus3'])
  assert.equal(roadCohort(criteria, road({ estimated: 15 })), 'R-counted', 'a counted total stays measured when its class split is estimated')
  assert.equal(roadCohort(criteria, road({ tier: null, source: 0, road_class: 'primary' })), 'R-default-major')
  assert.equal(roadCohort(criteria, road({ tier: null, source: 0 })), 'R-default-minor')
  assert.equal(roadCohort(criteria, road({ tier: 'heuristic', source: 11 })), 'R-local')
  assert.equal(roadCohort(criteria, road({ tier: 'heuristic', source: 12 })), 'R-transferred')
  assert.equal(roadCohort(criteria, road({ distance: 51 })), 'R-far')
})

test('the frozen panel keeps one record per physical station, both conventions for unknown mounts and French periods corrected', () => {
  const rows = [
    row('paris-ville-2024/bastille', 65, { year: 2024, mount: 'facade', publisher_facade_correction_db: -3 }),
    row('paris-ville-2025/bastille', 65, { mount: 'facade', publisher_facade_correction_db: -3,
      comparisons: [comparison('Lden', 'lden', 65.5, 70, '06-18/18-22/22-06', 'piecewise_constant')] }),
    row('madrid/RF-01', 60, { site_class: 'unknown', mic_height_m: null, road: { year: null } }),
  ]
  const manifest = freezeManifest(rows, criteria, 'baseline', null)
  assert.match(manifest.stations['paris-ville-2024/bastille'].exclusion_reason!, /another record/)
  const paris = manifest.stations['paris-ville-2025/bastille']
  assert.deepEqual(paris.variants, ['as_published', 'minus3'])
  assert.deepEqual(paris.metrics.lden.O_by_variant, { as_published: 70.03, minus3: 67.03 })
  assert.equal(paris.metrics.lden.period_basis, 'end_variant_06_18_22')
  assert.equal(paris.metrics.lden.u_components.period, 0.07)
  assert.ok(Math.abs(paris.metrics.lden.u_i_by_variant.minus3! - Math.hypot(paris.metrics.lden.u_i_by_variant.as_published!, 0.87)) < 0.002)
  const madrid = manifest.stations['madrid/RF-01']
  assert.equal(madrid.cohort, 'U-unclassified')
  assert.equal(madrid.k_assumed, true)
  // Unknown height: 4 m against 6 m to a road 8 m away (source 0.05 m): |10 lg(r(6)/r(4))| = 0.48 dB.
  assert.equal(madrid.metrics.lden.u_components.height, 0.482)
})

test('paired rules fire on a worse gated cohort and its stations, and the target needs a real MAE gain in every variant', () => {
  const baseline = Array.from({ length: 12 }, (_, index) => row(`net${index % 2}/s${index}`, 60 + (index % 4) * 0.3))
  const manifest = freezeManifest(baseline, criteria, 'baseline', null)
  const worse = baseline.map(entry => row(entry.key, entry.model!.total.lden! - 5))
  const whole = changeRules(baseline, worse, manifest, criteria).results
    .find(result => result.cohort === 'R-counted' && result.metric === 'lden' && result.variant === 'as_published' && result.stratum === 'all')!
  assert.deepEqual(whole.fired, ['mae', 'bias', 'large_error_share'])
  const better = baseline.map(entry => row(entry.key, entry.model!.total.lden! + 1.5))
  assert.equal(targetMet(baseline, better, manifest, criteria, 'R-counted', false).met, true)
  assert.equal(targetMet(baseline, baseline.map(entry => row(entry.key, entry.model!.total.lden! + 0.3)), manifest, criteria, 'R-counted', false).met, false)
  const over = baseline.map(entry => row(entry.key, entry.model!.total.lden! + 12))
  const upper = changeRules(baseline, over, manifest, criteria).results.find(result => result.cohort === 'R-counted' && result.metric === 'lden' && result.stratum === 'all')!
  assert.ok(upper.fired.includes('upper_bound') && upper.fired.includes('severe_station'))
})

test('an unscored frozen station makes the outcome incomplete, never a pass', () => {
  const identity = { server_cohort: { runtime_sha256: 'r' }, ops: { code: { native_sha256: 'n' }, prepared: { content_sha256: 'p', verified_current: true }, rasters: { station_square_rasters_sha256: 'x' }, definitions: {} } }
  const outcome = (unscored: Array<{ key: string; reason: string }>) =>
    gate({ target: { kind: 'deterministic', evidence: 'fixture' }, rules: [], moves: [], guardRegressions: [], unscored, guardErrors: [], identity }).outcome
  assert.equal(outcome([]), 'pass')
  assert.equal(outcome([{ key: 'bcn/1', reason: 'unavailable layers: aircraft' }]), 'incomplete')
  assert.equal(gate({ target: null, rules: [], moves: [], guardRegressions: [], unscored: [], guardErrors: [], identity: {} }).outcome, 'fail')
})

test('guard predicates are read from the criteria text and judged with their quantities', () => {
  const base = row('guards/velsen', 66.5).model!
  const model = { ...base, layers: { industrial: { lden: 66.5, periods: base.total.periods, share_lden: 1 } }, contributors: [
    { source_type: 'industrial', osm_id: 256624883, name: 'Vattenfall Cluster Velsen', subtype: 'industrial', distance_m: 28, received_lden: 66.4 },
    { source_type: 'industrial', osm_id: 6320127, name: 'Tata Steel', subtype: 'steel', distance_m: 423, received_lden: 41.9 },
  ] } as StationModel
  const guard = { id: 'velsen', lat: 52.474, lng: 4.633, layer: 'industrial', subchecks: [
    { id: 'top', predicate: 'top industrial contributor osm_id == 256624883 ("Vattenfall Cluster Velsen")' },
    { id: 'share', predicate: 'share of that contributor in industrial-layer energy >= 0.90', quantity_db: '10 lg(share / 0.90)' },
    { id: 'tata', predicate: 'contributor osm_id == 6320127 ("Tata Steel") >= 0.50 of industrial-layer energy' },
    { id: 'shell', predicate: 'the Shell Pernis refinery (OSM id: catalogue to record) is among the top 3 industrial contributors' },
  ] }
  const points = [{ guard: 'velsen', label: 'point', lat: 52.474, lng: 4.633, model, error: null }]
  const results = evaluateGuardSubchecks(guard, points, [], null, criteria)
  assert.deepEqual(results.map(result => result.state), ['pass', 'pass', 'fail', 'not_scored'])
  assert.equal(results[1].quantity_db, 0.36)
  // Once the catalogue records the refinery at that point, the top-3 check is scored.
  const catalogued = { ...row('guards/shell', 50), lat: 52.474, lng: 4.633, guard_osm: 'way/6320127' }
  assert.equal(evaluateGuardSubchecks(guard, points, [catalogued], null, criteria)[3].state, 'pass')
})
