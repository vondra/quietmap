// What flies over a point in words: the bands any flight reaches, their flights a day in one unit
// down to one a year (0.003), those of them at night, the height in kilometres, the type in words.
import assert from 'node:assert/strict'
import test from 'node:test'

import { aircraftEventRows } from '../src/components/noise/aircraft-events.ts'

test('the bands a flight reaches become rows, every count a day; a band without flights is none', () => {
  const rows = aircraftEventRows({
    above_db: [50, 60, 70],
    per_day: [412.4, 0.0082, 0],
    night: [21.2, 0, 0],
    height_m: [640, 1210, null],
    type: ['A320', 'ZZ9A', null],
    helicopters_per_day: 0,
  })
  assert.deepEqual(rows.map(row => [row.peak, row.perDay, row.night, row.heightKm]), [
    ['≥ 50 dB', '412', '21', '0.6'],
    ['≥ 60 dB', '0.008', '–', '1.2'],
  ])
  assert.equal(rows[0].type, 'Airbus A320')
  assert.equal(rows[0].typeTitle, 'Airbus A320 (A320)')
  // A designator without a name is its own title.
  assert.deepEqual([rows[1].type, rows[1].typeTitle], ['ZZ9A', 'ZZ9A'])
})

test('Meloneras: the table and the row say the same, a day', () => {
  const rows = aircraftEventRows({
    above_db: [50, 60, 70],
    per_day: [1.472, 0.102, 0.048],
    night: [0.105, 0, 0],
    height_m: [692, 185, 171],
    type: ['P208', 'P208', 'P208'],
    helicopters_per_day: 0,
  })
  assert.deepEqual(rows.map(row => [row.perDay, row.night]), [['1.5', '0.1'], ['0.1', '–'], ['0.05', '–']])
})
