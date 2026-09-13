import assert from 'node:assert/strict'
import test from 'node:test'

import {
  railTrainSourceLine,
  railTrafficLabel,
  roadCategoryEstimated,
  roadCategoryLine,
  roadTrafficDescription,
  roadTrafficLabel,
  roadTrafficSourceLine,
} from '../src/components/noise/provenance.ts'

function dataset(tier, overrides = {}) {
  return {
    tier,
    name: 'Example dataset',
    year: 2024,
    license: 'CC-BY-4.0',
    url: null,
    ...overrides,
  }
}

const traffic = (overrides = {}) => ({
  aadt_light: 8_824,
  aadt_medium: 516,
  aadt_heavy: 1_497,
  aadt_moto: 0,
  traffic_estimated: 0,
  ...overrides,
})

test('rail categories preserve unknown, known zero, fractional estimates and matching evidence', () => {
  const passenger = { periods: [0, 0.125, 0], status: 2, source_id: 7, matching: 3 }
  const freight = { periods: [0, 0, 0], status: 0, source_id: 0, matching: 0 }
  assert.match(railTrainSourceLine(passenger, dataset('national-measured')), /Estimated traffic/)
  assert.match(railTrainSourceLine(passenger, null), /relation alignment; estimated graph alignment/)
  assert.equal(railTrainSourceLine(freight, null), 'Unknown traffic; no count available')
  assert.equal(railTrainSourceLine({ ...freight, status: 1 }, null), 'Known count')
  assert.equal(railTrafficLabel({ passenger, freight }), '0.125/day + unknown')
})

test('prepared counts render per category with counted or estimated status', () => {
  const description = roadTrafficDescription(traffic(), dataset('national-measured'))
  assert.equal(
    description,
    'Source: Example dataset (2024) · CC-BY-4.0\n' +
      '\n' +
      'Prepared daily traffic, this road:\n' +
      '  Light: 8,824/day — counted\n' +
      '  Medium: 516/day — counted\n' +
      '  Heavy: 1,497/day — counted\n' +
      '  Moto: 0/day — counted zero',
  )
})

test('the estimated bitmask marks per-category priors without touching values', () => {
  const mixed = traffic({ aadt_medium: 200, traffic_estimated: 0b1111 })
  assert.equal(roadCategoryEstimated(mixed, 0b0001), true)
  assert.equal(roadCategoryLine('Medium', mixed.aadt_medium, true), 'Medium: 200/day — estimated')
  // Bit clear keeps a positive value a counted observation...
  assert.equal(roadCategoryLine('Heavy', 500, false), 'Heavy: 500/day — counted')
  // ...and only an observed zero reads as "counted zero".
  assert.equal(roadCategoryLine('Moto', 0, false), 'Moto: 0/day — counted zero')
  assert.equal(roadCategoryLine('Moto', 0, true), 'Moto: 0/day — estimated')
})

test('a prior without a dataset names the class prior, never a fake dataset', () => {
  assert.equal(roadTrafficSourceLine(null), 'Source: class prior (no observation dataset)')
  const description = roadTrafficDescription(traffic({ traffic_estimated: 15 }), null)
  assert.match(description, /class prior/)
  assert.doesNotMatch(description, /CNOSSOS/i)
  assert.doesNotMatch(description, /matched/i)
})

test('dataset attribution keeps source, units and url', () => {
  const line = roadTrafficSourceLine(dataset('national-measured', { url: 'https://example.org/aadt' }))
  assert.equal(
    line,
    'Source: Example dataset (2024) · CC-BY-4.0\n  https://example.org/aadt',
  )
})

test('the headline label totals the four prepared classes', () => {
  assert.equal(roadTrafficLabel(traffic()), '10,837/day')
  assert.equal(
    roadTrafficLabel(traffic({ aadt_light: 0, aadt_medium: 0, aadt_moto: 0, aadt_heavy: 500 })),
    '500/day',
  )
})
