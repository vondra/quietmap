/** Native windows and indicators map onto the model's END periods without silent assumptions. */
import assert from 'node:assert/strict'
import test from 'node:test'
import { parseIndicator, validateStation, type LayerSelector, type MeasuredValue } from './catalogue.ts'
import { ldenFromPeriods, nativeLdenFromModelPeriods, windowLevelFromModelPeriods, END_PERIOD_WINDOWS } from './lib.ts'

const levels = { day: 60, evening: 55, night: 50 }
const FRENCH_PERIODS = { day: '06-18', evening: '18-22', night: '22-06' }

test('a window of whole END periods is exact; any other window spreads each period over its hours', () => {
  assert.deepEqual(windowLevelFromModelPeriods(levels, END_PERIOD_WINDOWS.night), { level: 50, exact: true })
  const allDay = windowLevelFromModelPeriods(levels, { start: 0, end: 24 })
  assert.equal(allDay.exact, true)
  // 06–22: one night hour, twelve day hours, three evening hours.
  const german = windowLevelFromModelPeriods(levels, { start: 6, end: 22 })
  assert.equal(german.exact, false)
  assert.ok(Math.abs(german.level! - 59.1145) < 1e-4, String(german.level))
  assert.deepEqual(windowLevelFromModelPeriods({ day: null, evening: null, night: null }, { start: 6, end: 22 }), { level: null, exact: false })
})

test('a native Lden re-averages the model periods over the native windows', () => {
  assert.ok(Math.abs(ldenFromPeriods(60, 55, 50)! - 60) < 1e-9)
  const end = nativeLdenFromModelPeriods(levels, END_PERIOD_WINDOWS)
  assert.equal(end.exact, true)
  assert.ok(Math.abs(end.level! - 60) < 1e-9)
  // French END periods (06–18, 18–22, 22–06) on the same model: +0.58 dB.
  const french = nativeLdenFromModelPeriods(levels, { day: { start: 6, end: 18 }, evening: { start: 18, end: 22 }, night: { start: 22, end: 6 } })
  assert.equal(french.exact, false)
  assert.ok(Math.abs(french.level! - 60.5793) < 1e-4, String(french.level))
})

test('indicators carry their window, layer and band, or say why the model has no counterpart', () => {
  const indicator = (key: string, value: MeasuredValue, periods: Record<string, unknown> | null, layer: LayerSelector = null) => {
    const result = parseIndicator(key, value, periods, layer)
    return 'indicator' in result ? result.indicator : result.unsupported
  }
  assert.deepEqual(indicator('Lnight', 63.4, FRENCH_PERIODS), {
    key: 'Lnight', value: 63.4, band: null, layer: null, kind: 'window', period: 'night', window: { start: 22, end: 6 }, periods_assumed_end: false,
  })
  assert.deepEqual(indicator('LAeq_22_06_non_aircraft', 51.3, { day: '06-22', night: '22-06' }), {
    key: 'LAeq_22_06_non_aircraft', value: 51.3, band: null, layer: 'non_aircraft', kind: 'window', period: '22-06', window: { start: 22, end: 6 }, periods_assumed_end: false,
  })
  const official = indicator('Lden_class', [60, 65], null, 'railway')
  assert.ok(typeof official === 'object' && official.kind === 'weighted' && official.layer === 'railway' && official.periods_assumed_end)
  assert.deepEqual(typeof official === 'object' && official.band, [60, 65])
  const cnel = indicator('CNEL_aircraft', 61.8, { day: '07-19', evening: '19-22', night: '22-07' })
  assert.ok(typeof cnel === 'object' && cnel.kind === 'weighted' && Math.abs(cnel.evening_penalty_db - 4.771) < 1e-3)
  assert.equal(indicator('Levening', 60, { day: '06-22', night: '22-06' }), 'native_periods names no evening window')
  assert.match(String(indicator('LAeq_day_stationary_source', 52.5, { day: '06-22 (8 loudest hours)' })), /not a whole local hour/)
  assert.equal(typeof indicator('L90', 54, null) === 'object' && (indicator('L90', 54, null) as { kind: string }).kind, 'percentile')
  assert.deepEqual(indicator('AADT (veh/day)', 81103, null), { key: 'AADT (veh/day)', value: 81103, band: null, kind: 'traffic', period: 'total', layer: 'road', periods_assumed_end: false })
  assert.equal(indicator('LAeq_T', 33.9, null), 'no model counterpart for this indicator name')
})

test('a station without its holdout flag or with a malformed indicator is refused', () => {
  const station = {
    station_id: 'madrid-sivca-2025/RF-01', set: 'madrid-sivca-2025', name: 'Paseo de Recoletos', lat: 40.42, lng: -3.69,
    mic_height_m: null, truth_kind: 'measured', expected_source: 'unknown', guard: false, holdout: false,
    indicators: { Lden: 71.5, Lden_class: [70, null] }, native_periods: null,
  }
  assert.doesNotThrow(() => validateStation(station, 'fixture'))
  assert.throws(() => validateStation({ ...station, holdout: undefined }, 'fixture'), /holdout/)
  assert.throws(() => validateStation({ ...station, indicators: { Lden: '71' } }, 'fixture'), /number, a \[low, high\] band/)
})
