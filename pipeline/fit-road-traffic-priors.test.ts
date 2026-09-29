/** Prior fit: length-weighted medians per class, direction and place, holdout rows excluded, engine table shape. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { Bool, Float32, Float64, RecordBatch, Schema, Table, Uint16, Uint8, tableToIPC, vectorFromArray } from 'apache-arrow'
import { fitPriors, predict, priorTableRust, squareSamples, type Sample } from './fit-road-traffic-priors.js'

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

test('squareSamples keeps only observed truth: fully estimated rows never train or score', () => {
  // A counted GB row, a DfT scaled row (every class estimated) and a prior row.
  const rows = [
    { source: 1041, estimated: 0 },
    { source: 1041, estimated: 15 },
    { source: 0, estimated: 15 },
  ]
  const table = new Table({
    road_class: vectorFromArray(rows.map(() => 1), new Uint8()),
    oneway: vectorFromArray(rows.map(() => 0), new Uint8()),
    junction: vectorFromArray(rows.map(() => 0), new Uint8()),
    lanes: vectorFromArray(rows.map(() => 2), new Uint8()),
    built_up: vectorFromArray(rows.map(() => 2), new Uint8()),
    access: vectorFromArray(rows.map(() => 0), new Uint8()),
    source_id: vectorFromArray(rows.map(row => row.source), new Uint16()),
    length_m: vectorFromArray(rows.map(() => 100), new Float32()),
    tunnel: vectorFromArray(rows.map(() => false), new Bool()),
    aadt_light: vectorFromArray(rows.map(() => 8000), new Float64()),
    aadt_medium: vectorFromArray(rows.map(() => 500), new Float64()),
    aadt_heavy: vectorFromArray(rows.map(() => 1000), new Float64()),
    aadt_moto: vectorFromArray(rows.map(() => 100), new Float64()),
    traffic_estimated: vectorFromArray(rows.map(row => row.estimated), new Uint8()),
  })
  const directory = mkdtempSync(join(tmpdir(), 'fit-priors-test-'))
  try {
    const square = join(directory, 'z9', '276', '173') // Prague: a training square
    mkdirSync(square, { recursive: true })
    const schema = new Schema(table.schema.fields, new Map([['road_traffic_contract', '1']]))
    const stored = new Table(schema, table.batches.map(batch => new RecordBatch(schema, batch.data)))
    writeFileSync(join(square, 'roads.arrow'), Buffer.from(tableToIPC(stored, 'file')))
    const packed = squareSamples(directory, 'z9/276/173')
    assert.deepEqual([...packed], [1, 0, 2, 2, 9600, 100, 0])
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test('one-way secondary shares the two-way section: no one-way arm is fitted and prediction halves', () => {
  // The retired arm read +3.0 dB on holdout genuine one-way secondary streets
  // and doubled split-mapped two-way streets (w3-priors, 2026-09-25).
  const samples: Sample[] = []
  for (let roadClass = 0; roadClass < 5; roadClass++) {
    for (const oneWay of [true, false]) for (const builtUp of [1, 2]) {
      samples.push(sample({ roadClass, oneWay, builtUp, lanes: 2, count: 2000 * builtUp, length: 3000 }),
        sample({ roadClass, oneWay, builtUp, lanes: 0, count: 1000 * builtUp, length: 2000 }))
    }
  }
  const table = fitPriors(samples)
  assert.deepEqual(table[3][0][2], { vehiclesPerLane: 0, untagged: 0, km: 0 })
  assert.equal(predict(table, sample({ roadClass: 3, oneWay: true, builtUp: 2, lanes: 0 })), table[3][1][2].untagged / 2)
})
