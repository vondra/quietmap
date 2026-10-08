// What flies over a point in words: the bands any flight reaches, a day from one a day up and a year
// below, the night's share, the height in kilometres and the type in words.
import assert from 'node:assert/strict'
import test from 'node:test'

import { aircraftEventRows, eventCount } from '../src/components/noise/aircraft-events.ts'

test('a rate reads a day from one a day up, else a year', () => {
  assert.equal(eventCount(412.4), '412 a day')
  assert.equal(eventCount(1.04), '1 a day')
  assert.equal(eventCount(2.46), '2.5 a day')
  assert.equal(eventCount(0.0082), '3 a year')
  assert.equal(eventCount(0.0005), '<1 a year')
})

test('the bands a flight reaches become rows; a band without flights is none', () => {
  const rows = aircraftEventRows({
    above_db: [50, 60, 70],
    per_day: [412.4, 0.0082, 0],
    night: [21.2, 0, 0],
    height_m: [640, 1210, null],
    type: ['A320', 'B738', null],
    helicopters_per_day: 0,
  })
  assert.deepEqual(rows.map(row => [row.above, row.count, row.night, row.height]), [
    ['50 dB', '412 a day', '21 a day', '0.6 km'],
    ['60 dB', '3 a year', '', '1.2 km'],
  ])
  assert.ok(rows[0].type.length > 0)
})
