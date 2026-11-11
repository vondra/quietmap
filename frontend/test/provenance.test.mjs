// Traffic wording: counted or estimated road classes, known, estimated or unknown train categories.
import assert from 'node:assert/strict'
import test from 'node:test'

import { readYears } from '../scripts/year-sources.ts'
import { datasetCitations } from '../scripts/dataset-citations.ts'
const built = readYears().find(year => year.manifest.built).manifest
const citation = code => { const [name, observed, measurement] = datasetCitations(built)[code]; return [name, observed, measurement].filter(Boolean).join(', ') }

import {
  datasetLine,
  railDatasetLines,
  railTrafficDescription,
  railTrafficLabel,
  railTrainSourceLine,
  roadCategoryEstimated,
  ROAD_ESTIMATED_LIGHT,
  ROAD_ESTIMATED_MOTO,
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

test('the estimated bitmask marks per-category priors', () => {
  const mixed = { traffic_estimated: 0b1111 }
  assert.equal(roadCategoryEstimated(mixed, ROAD_ESTIMATED_LIGHT), true)
  assert.equal(roadCategoryEstimated({ traffic_estimated: ROAD_ESTIMATED_MOTO }, ROAD_ESTIMATED_LIGHT), false)
})

test('a record names the dataset behind its traffic; a class prior or an unknown id names none', () => {
  assert.equal(datasetLine(20), citation(20))
  assert.equal(datasetLine(30), citation(30))
  assert.equal(datasetLine(0), null)
  assert.equal(datasetLine(null), null)
  assert.equal(datasetLine(65535), null)
})

test('a level crossing names the dataset of its soundings, never passenger trains or freight', () => {
  assert.deepEqual(railDatasetLines(111, 20, true), [`Soundings: ${citation(111)}`])
  assert.deepEqual(railDatasetLines(20, 0, false), [`Passenger trains: ${citation(20)}`])
})


// A timetable citation must expose the independently estimated missing-calendar share.
test('rail sources distinguish the missing-day estimate from the timetable', () => {
  assert.deepEqual(railDatasetLines(100, 0, false, [0.75, 0]), [`Passenger trains: ${citation(100)}; 75.0% estimated for dates without timetable data`])
  assert.deepEqual(railDatasetLines(65535, 0, false, [0.75, 0]), ['Passenger trains: 75.0% estimated for dates without timetable data'])
  assert.deepEqual(railDatasetLines(65535, 0, false, [0.0001, 0]), ['Passenger trains: <0.1% estimated for dates without timetable data'])
})
