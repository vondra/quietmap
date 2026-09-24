/** Criteria as data: frozen membership, per-station uncertainty, paired change rules and the C1–C6 verdict. */
import assert from 'node:assert/strict'
import test from 'node:test'
import type { IndicatorComparison } from './comparison.ts'
import { membership, uncertainty, type Criteria } from './criteria.ts'
import { changeRules, cohortTable, freezeMembership } from './gate.ts'
import type { StationModel } from './popup.ts'
import type { StationRow } from './report.ts'
import { bootstrap, mean } from './statistics.ts'
import { gate, movesOverThreshold } from './verdict.ts'

const valued = <T>(value: T) => ({ value, provenance: 'fixture' })
const road = (provenance: string, distance: number) => ({ membership: {
  site_class: ['traffic'], 'dominant_road.max_distance_m': 50, 'dominant_road.traffic_provenance': [provenance],
}, distance })
const criteria: Criteria = {
  criteria_version: 'fixture',
  constants: {
    n_min: valued(10), tau_db: valued(0.6), move_explain_db: valued(3), tail_thresholds_db: valued([6, 10] as [number, number]),
    objective_tail6_max_share: valued(0.1), objective_tail10_max_share: valued(0), u_method_db: valued(1.5),
    layer_relevance_db: valued(10), upper_bound_factor: valued(2),
  },
  bootstrap: { draws: 400, level: 0.95, seed: 20260924 },
  membership: { order: ['N-nightlife', 'A-nmt', 'R-counted', 'R-local', 'R-far'] },
  metrics: { periods_gated: ['lden', 'ln'], periods_reported: ['ld', 'le'] },
  uncertainty_model: { components: {
    instrument: { class2_or_unknown_db: 1.5 },
    window: { months_to_db: { 12: 0, 11: 0.32, 10: 0.48, 9: 0.62 } },
    year_gap: { k_definition: 'dominant input year; else |measurement year - 2026|', station_db: { 0: 0, 1: 1.01, 2: 1.21 }, network_mean_db: { 0: 0, 1: 0.32, 2: 0.42 }, diagnostic_years: [2020, 2021] },
    period: { shifted_end_periods_06_18_22: { lden_db: 0.34, ln_db: 0.81 } },
    receiver_convention: { free_field_db: 0, facade_0_5_to_2m_correction_db: -3, facade_flush_correction_db: -6, u_when_corrected_db: 0.87, unknown: 'no correction; one-sided allowance a_i = 3.0 dB' },
    height: { unknown: 'assume 4.0 m; term against 6.0 m' },
  } },
  cohorts: [
    { id: 'N-nightlife', role: 'named_gap', compare: 'total', membership: { site_class: ['nightlife'] }, metrics_reported: ['lden', 'ln'] },
    { id: 'A-nmt', role: 'reported', compare: 'layer:aircraft', membership: { site_class: ['aircraft_nmt'], dominance: 'event_classified' } },
    { id: 'R-counted', role: 'gated', compare: 'total', membership: road('measured_count', 50).membership, metrics_gated: ['lden', 'ln'] },
    { id: 'R-local', role: 'gated', compare: 'total', membership: road('service_tree', 50).membership, metrics_gated: ['lden', 'ln'] },
    { id: 'R-far', role: 'gated', compare: 'total', membership: { site_class: ['traffic'], 'dominant_road.min_distance_m_exclusive': 50 }, metrics_gated: ['lden', 'ln'] },
  ],
}

const lden = (model: number, measured: number, windows = '07-19/19-23/23-07'): IndicatorComparison => ({
  indicator: 'Lden', period: 'lden', kind: 'weighted', layer: null, measured, band: null, windows, model, delta_db: model - measured,
  unit: 'dB', period_mapping: windows === '07-19/19-23/23-07' ? 'exact' : 'piecewise_constant', periods_assumed_end: false,
})
function row(key: string, fields: Partial<StationRow> & { provenance?: string; distance?: number; model_lden?: number; measured?: number }): StationRow {
  const modelLden = fields.model_lden ?? 60
  const model = {
    total: { lden: modelLden, periods: { day: modelLden - 2, evening: modelLden - 3, night: modelLden - 8 } },
    layers: { road: { lden: modelLden, periods: { day: modelLden - 2, evening: modelLden - 3, night: modelLden - 8 }, share_lden: 1 } },
    dominant_layer: 'road', contributors: [{ source_type: 'road', osm_id: 1, name: 'street', subtype: 'secondary', distance_m: fields.distance ?? 8, received_lden: modelLden }],
    loudest_by_layer: {}, inside_footprint: false,
    dominant_road: { traffic_provenance: fields.provenance ?? 'measured_count', distance_m: fields.distance ?? 8, road_class: 'secondary', dataset_year: 2025 },
  } as unknown as StationModel
  return {
    key, set: key.split('/')[0], station_id: key, name: key, lat: 0, lng: 0, expected_source: 'road', truth_kind: 'measured',
    measurand: 'sound_level', year: 2025, months_covered: 12, holdout: false, holdout_square: false, scoring: 'accuracy', scoring_reason: null,
    mount: 'free_field', facade_distance_m: null, publisher_facade_correction_db: null, position_uncertainty_m: null, mic_height_m: 4,
    requested_receiver_height_m: 4, receiver_height_used_m: 4, height_matches_microphone: true, request_ms: 1, model,
    comparisons: [lden(modelLden, fields.measured ?? 62)], guard: null, position_samples: null, unsupported_indicators: [], error: null, ...fields,
  }
}

test('membership takes the first matching cohort and names why a station has none', () => {
  assert.equal(membership(row('madrid/RF-01', {}), criteria).cohort, 'R-counted')
  assert.equal(membership(row('madrid/RF-02', { distance: 80 }), criteria).cohort, 'R-far')
  assert.equal(membership(row('bcn/1', { expected_source: 'nightlife' }), criteria).cohort, 'N-nightlife')
  assert.match(membership(row('guards/map', { truth_kind: 'official_map_modelled' }), criteria).reason, /cross_check/)
  assert.match(membership(row('madrid/RF-03', { expected_source: 'unknown' }), criteria).reason, /no cohort/)
})

test('the uncertainty budget corrects documented facades, bounds unknown ones one-sidedly and adds pending heights', () => {
  const facade = uncertainty(row('paris/1', { mount: 'facade', facade_distance_m: 2 }), lden(60, 62), criteria)
  assert.equal(facade.correction_db, -3)
  assert.equal(facade.components.receiver_convention, 0.87)
  const ambiguous = uncertainty(row('paris/2', { mount: 'facade', facade_distance_m: 2, publisher_facade_correction_db: -3 }), lden(60, 62), criteria)
  assert.deepEqual([ambiguous.correction_db, ambiguous.one_sided_allowance_db], [0, 3])
  const french = uncertainty(row('paris/3', {}), lden(60, 62, '06-18/18-22/22-06'), criteria)
  assert.equal(french.components.period, 0.34)
  // 8 m from the road: a 1.2 m microphone scored at 4 m adds |10 lg(r(1.2)/r(4))| = 0.43 dB.
  // Measured in 2024 against 2025 counts: a one-year gap, 1.01 dB.
  const pending = uncertainty(row('eba/1', { mic_height_m: 1.2, year: 2024 }), lden(60, 62), criteria)
  assert.ok(Math.abs(pending.components.height - 0.43) < 0.01, String(pending.components.height))
  assert.ok(pending.flags.includes('height_pending'))
  assert.equal(pending.components.year_gap, 1.01)
  assert.ok(Math.abs(pending.u_i - Math.hypot(1.5, 1.01, pending.components.height)) < 1e-9)
  assert.equal(uncertainty(row('x/1', { year: 2020 }), lden(60, 62), criteria).eligible, false)
})

test('paired bootstrap draws the same stations for both runs', () => {
  const pairs = [1, 2, 3, 4].map(value => ({ a: value, b: value + 1, network: value < 3 ? 'n1' : 'n2' }))
  const { change } = bootstrap(pairs, pair => pair.network, criteria.bootstrap, { change: sample => mean(sample.map(pair => pair.b - pair.a)) })
  assert.deepEqual(change, { value: 1, ci95: [1, 1] })
})

test('a worse counted cohort fails C2 and blocks the verdict; an improved target alone passes', () => {
  const baseline = Array.from({ length: 12 }, (_, index) => row(`net${index % 2}/${index}`, { model_lden: 60 + (index % 3) * 0.2 }))
  const worse = baseline.map(entry => row(entry.key, { model_lden: entry.comparisons[0].model! - 4 }))
  const better = baseline.map(entry => row(entry.key, { model_lden: entry.comparisons[0].model! + 1.5 }))
  const frozen = freezeMembership(baseline, criteria, 'baseline')
  assert.equal(cohortTable(baseline, frozen, criteria).find(entry => entry.cohort === 'R-counted' && entry.metric === 'lden' && entry.column === 'all')?.statistics.n, 12)
  const identity = { server_cohort: { runtime_sha256: 'r' }, ops: { code: { native_sha256: 'n' }, prepared: { content_sha256: 'p', verified_current: true }, rasters: { station_square_rasters_sha256: 'x' }, definitions: {} } }
  const verdictOf = (after: StationRow[]) => {
    const rules = changeRules(baseline, after, frozen, criteria)
    const targetRules = rules.find(rule => rule.cohort === 'R-counted' && rule.metric === 'lden' && rule.stratum === 'all') ?? null
    return gate({ criteria, target: { cohort: 'R-counted' }, targetRules, predecessorRules: rules, baselineRules: [], moves: movesOverThreshold(baseline, after, criteria, []), guardRegressions: [], identity })
  }
  const failing = verdictOf(worse)
  assert.equal(failing.verdict, 'fail')
  assert.match(failing.C2.detail, /R-counted\/lden\/all vs predecessor: .*mae/)
  assert.equal(failing.C3.holds, false, 'a 4 dB move needs a cause record')
  const passing = verdictOf(better)
  assert.equal(passing.C1.holds, true)
  assert.equal(passing.verdict, 'pass')
  assert.equal(verdictOf(better).C6.holds, true)
  assert.equal(gate({ criteria, target: null, targetRules: null, predecessorRules: [], baselineRules: [], moves: [], guardRegressions: [], identity: {} }).C6.holds, false)
})
