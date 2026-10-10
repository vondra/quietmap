// Traffic wording: counted or estimated road classes, known, estimated or unknown train categories.
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  railTrafficDescription,
  railTrafficLabel,
  railTrainSourceLine,
  roadCategoryEstimated,
  roadCategoryLine,
} from '../src/components/noise/provenance.ts'

test('rail categories preserve unknown, known zero and fractional estimates', () => {
  const passenger = { periods: [0, 0.125, 0], status: 2 }
  const freight = { periods: [0, 0, 0], status: 0 }
  assert.equal(railTrainSourceLine(passenger), 'Estimated traffic')
  assert.equal(railTrainSourceLine(freight), 'Unknown traffic; no count available')
  assert.equal(railTrainSourceLine({ ...freight, status: 1 }), 'Known count')
  assert.equal(railTrafficLabel({ passenger, freight }), '0.1 passenger/day · freight unknown')
  assert.equal(
    railTrafficDescription({ passenger, freight }),
    'Passenger: Estimated traffic\nDay / evening / night: 0 / 0.125 / 0\n\n' +
      'Freight: Unknown traffic; no count available',
  )
})

test('a line reads its trains by category beside the row\'s sum; a category without trains is left out', () => {
  // Praha – Chomutov at Šárka, and a Prague tram.
  const estimated = (periods) => ({ periods, status: 2 })
  assert.equal(railTrafficLabel({ passenger: estimated([59.5, 17, 9.5]), freight: estimated([7.54, 2.64, 5.93]) }),
    '86 passenger · 16 freight/day')
  assert.equal(railTrafficLabel({ passenger: estimated([42, 15, 3]), freight: estimated([0, 0, 0]) }), '60 passenger/day')
  assert.equal(railTrafficLabel({ passenger: { periods: [40, 8, 6], status: 1 }, freight: { periods: [0, 0, 0], status: 0 } }, true),
    '54 soundings/day')
})

test('horn approaches read as soundings, one category', () => {
  const soundings = { periods: [40, 8, 6], status: 1 }
  assert.equal(
    railTrafficDescription({ passenger: soundings, freight: { periods: [0, 0, 0], status: 0 } }, true),
    'Soundings: Known count\nDay / evening / night: 40 / 8 / 6',
  )
})

test('the estimated bitmask marks per-category priors without touching values', () => {
  const mixed = { traffic_estimated: 0b1111 }
  assert.equal(roadCategoryEstimated(mixed, 0b0001), true)
  assert.equal(roadCategoryEstimated({ traffic_estimated: 0b1000 }, 0b0001), false)
  assert.equal(roadCategoryLine('Medium', 200, true), 'Medium: 200/day — estimated')
  // Bit clear keeps a positive value a counted observation...
  assert.equal(roadCategoryLine('Heavy', 500, false), 'Heavy: 500/day — counted')
  // ...and only an observed zero reads as "counted zero".
  assert.equal(roadCategoryLine('Moto', 0, false), 'Moto: 0/day — counted zero')
  assert.equal(roadCategoryLine('Moto', 0, true), 'Moto: 0/day — estimated')
})
