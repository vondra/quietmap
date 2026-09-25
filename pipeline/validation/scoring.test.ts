/** Popup answers become outdoor model levels, and guards judge source identity whatever the total. */
import assert from 'node:assert/strict'
import test from 'node:test'
import type { CatalogueStation } from './catalogue.ts'
import { compareIndicator, evaluateGuard, parseGuardExpectation } from './comparison.ts'
import { END_PERIOD_WINDOWS } from './lib.ts'
import { readPopupAnswer, roadTrafficProvenance, type PopupAnswer } from './popup.ts'

const road = (osm_id: number, received_lden: number, metadata: Record<string, unknown>) => ({
  source_type: 'road', osm_id, name: `road ${osm_id}`, subtype: 'secondary', distance_m: 8, received_lden,
  metadata: { kind: 'road', road_class: 'secondary', aadt_light: 1400, aadt_medium: 50, aadt_heavy: 50, aadt_moto: 0, ...metadata },
})

const indoorAnswer: PopupAnswer = {
  center: [48.87, 2.34],
  receiver: { lat: 48.87002, lng: 2.34, height_m: 4 },
  total_lden: 40,
  sources: [
    { source_type: 'road', lden: 39.5, ld: 37, le: 36, ln: 31 },
    { source_type: 'aircraft', lden: 30.5, ld: 28, le: 27, ln: 0 },
  ],
  top_contributors: [road(7, 39, { traffic_estimated: 15, dominant_source_id: 0, provenance: null })],
  envelope_class: 'residential',
  envelope_delta_db: 30,
  facade_lden: 70,
}

test('an indoor answer is restored to its facade levels, and a floored indoor level is unknown', () => {
  const model = readPopupAnswer(indoorAnswer, { lat: 48.87, lng: 2.34 })
  assert.equal(model.inside_footprint, true)
  assert.equal(model.total.lden, 70)
  assert.deepEqual(model.layers.road.periods, { day: 67, evening: 66, night: 61 })
  assert.equal(model.layers.aircraft.periods.night, null, 'indoor 0 dB is a floor, not facade − Δ')
  assert.equal(model.receiver.click_to_receiver_m, 2.2)
  assert.equal(model.dominant_layer, 'road')
  assert.equal(model.contributors[0].received_lden, 69)
})

test('the dominant road is counted by its dataset tier, not by per-class estimate bits', () => {
  const tier = (value: string) => ({ tier: value })
  assert.equal(roadTrafficProvenance({ dominant_source_id: 10, traffic_estimated: 15, provenance: tier('continental-measured') }), 'measured_count')
  assert.equal(roadTrafficProvenance({ dominant_source_id: 11, traffic_estimated: 15, provenance: tier('heuristic') }), 'service_tree')
  assert.equal(roadTrafficProvenance({ dominant_source_id: 12, traffic_estimated: 15, provenance: tier('heuristic') }), 'continuity_fill')
  assert.equal(roadTrafficProvenance({ dominant_source_id: 0, traffic_estimated: 15, provenance: null }), 'class_default')
  const local = readPopupAnswer({ ...indoorAnswer, top_contributors: [road(8, 50, {
    road_class: 'residential', traffic_estimated: 15, dominant_source_id: 11, provenance: tier('heuristic'),
  })] }, { lat: 48.87, lng: 2.34 })
  assert.equal(local.dominant_road?.traffic_provenance, 'service_tree')
  assert.equal(local.dominant_road?.vehicle_split_estimated, true)
})

const station = (fields: Partial<CatalogueStation>): CatalogueStation => ({
  station_id: 'guards/ind_tata_ijmuiden_velsen', set: 'guards', name: 'Tata Steel IJmuiden', lat: 52.474, lng: 4.633, mic_height_m: 4,
  truth_kind: 'official_map_modelled', expected_source: 'industry', guard: true, guard_expected_source: null,
  holdout: false, indicators: { Lden: 65 }, native_periods: null, ...fields,
})
const lden = { key: 'Lden', value: 65, band: null, kind: 'weighted', period: 'lden', windows: END_PERIOD_WINDOWS, evening_penalty_db: 5, periods_assumed_end: true } as const

test('a guard fails when another source of the expected layer dominates, whatever the total', () => {
  const answer: PopupAnswer = {
    center: [52.474, 4.633], total_lden: 66.4,
    sources: [{ source_type: 'industrial', lden: 66.3, ld: 66, le: 66, ln: 66 }, { source_type: 'road', lden: 50, ld: 48, le: 46, ln: 40 }],
    top_contributors: [
      { source_type: 'industrial', osm_id: 256624883, name: 'Vattenfall Cluster Velsen', subtype: 'industrial', distance_m: 28, received_lden: 66.2 },
      { source_type: 'industrial', osm_id: 4242, name: 'Tata Steel IJmuiden', subtype: 'steel', distance_m: 423, received_lden: 41.9 },
    ],
  }
  const model = readPopupAnswer(answer, { lat: 52.474, lng: 4.633 })
  assert.equal(compareIndicator({ ...lden, layer: 'industrial' }, model).delta_db, 1.3)
  assert.equal(compareIndicator({ ...lden, layer: null }, model).delta_db, 1.4)
  assert.equal(compareIndicator({ ...lden, value: null, band: [60, 65], layer: 'industrial' }, model).delta_db, 1.3)
  assert.equal(compareIndicator({ ...lden, value: null, band: [null, 70], layer: 'industrial' }, model).delta_db, 0)
  const tata = 'industry (evidence: the official layer covers industry only) - facility identity (Tata Steel) unverified'
  assert.deepEqual(parseGuardExpectation(tata, 'industry'), { layer: 'industrial', name: 'Tata Steel', identity_verified_by_catalogue: false })
  const byName = evaluateGuard(station({ guard_expected_source: tata }), model)
  assert.equal(byName.passed, false)
  assert.match(byName.reason, /Vattenfall/)
  assert.equal(evaluateGuard(station({ guard_expected_source: 'industry: Velsen cluster (evidence: fixture)' }), model).passed, true)
  assert.equal(evaluateGuard(station({ guard_expected_source: 'rail (evidence: fixture)', expected_source: 'rail' }), model).passed, false)
})
