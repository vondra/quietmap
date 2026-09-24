/** Prior fit: length-weighted medians per class, direction and place, holdout rows excluded, engine table shape. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { fitPriors, predict, priorTableRust, type Sample } from './fit-road-traffic-priors.js'

const sample = (overrides: Partial<Sample>): Sample => ({
  roadClass: 0, oneWay: true, builtUp: 2, lanes: 2, count: 20_000, length: 1000, holdout: false, ...overrides,
})

test('priors are length-weighted medians of training rows only, per lane for main roads and whole for the rest', () => {
  const samples: Sample[] = []
  for (let roadClass = 0; roadClass < 5; roadClass++) {
    for (const oneWay of [true, false]) for (const builtUp of [1, 2]) {
      samples.push(sample({ roadClass, oneWay, builtUp, lanes: 2, count: 1000 * builtUp, length: 3000 }),
        sample({ roadClass, oneWay, builtUp, lanes: 0, count: 500 * builtUp, length: 2000 }),
        sample({ roadClass, oneWay, builtUp, lanes: 0, count: 9_000_000, length: 100_000, holdout: true }))
    }
  }
  const table = fitPriors(samples)
  assert.deepEqual(table[0][0][2], { vehiclesPerLane: 1000, untagged: 1000, km: 5 })
  // Secondary: no per-lane rate; the whole-count median weighs the 3 km row over the 2 km one.
  assert.deepEqual(table[3][1][1], { vehiclesPerLane: 0, untagged: 1000, km: 5 })
  assert.equal(predict(table, sample({ roadClass: 0, lanes: 3, builtUp: 1 })), 3 * 500)
  assert.equal(predict(table, sample({ roadClass: 4, lanes: 3, builtUp: 0, oneWay: false })), table[4][1][0].untagged)
  assert.match(priorTableRust(table, 'fixture'), /pub const MEASURED_CARRIAGEWAY_PRIORS: \[\[\[CarriagewayPrior; 3\]; 2\]; 5\] = \[/)
})
