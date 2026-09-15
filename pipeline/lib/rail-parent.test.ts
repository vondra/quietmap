/** Repeat enrichment recovers unique parents while retaining independent sidecar evidence. */

import assert from 'node:assert/strict'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'
import { Float64, RecordBatch, Schema, Table, Uint8, Uint16, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { RAIL_TEST_DIRECTORY, writePreparedRailwaySquare } from './rail-test-fixture.js'
import { writeTransportFixture } from './transport-test-fixture.js'
import { SourceTransportTopology } from './transport-topology.js'
import { collectZ9RailGraphSegments } from './rail-walk-enrich.js'
import { writeRailwayTraffic } from './railways-arrow.js'
import { listRailIntervals, openRailTrafficSidecar } from './rail-traffic-store.js'
import { restoreRailwayParentsForEnrichment } from './rail-parent.js'
import { lonLatToGrid } from './prepared-grid.js'

const SQUARE = 'z9/262/182'

function finalizedFixture(name: string, differentName = false, geometry: 'normal' | 'dateline' | 'gap' = 'normal'): { prepared: string; path: string } {
  const prepared = join(RAIL_TEST_DIRECTORY, name)
  const [start, middle, end] = geometry === 'dateline' ? [179.8, -180, -179.8] : [4.8, 4.801, 4.802]
  const path = writePreparedRailwaySquare(prepared, SQUARE, `${name}.arrow`, [
    { osmId: 50000, segmentIndex: 0, latitude: 45.75, longitude: start,
      endLatitude: 45.75, endLongitude: geometry === 'gap' ? 4.8009 : middle, name: 'line', country: 'FR' },
    { osmId: 50000, segmentIndex: 0, latitude: 45.75, longitude: middle,
      endLatitude: 45.75, endLongitude: end, name: differentName ? 'conflict' : 'line', country: 'FR' },
  ])
  writeTransportFixture(prepared,
    [{ id: '50000', nodes: [['1', [45.75, start]], ['2', [45.75, end]]] }],
    [{ way: '50000', segment: 0, square: SQUARE, start: [0, 0], end: [1, 0] }])
  const raw = tableFromIPC(readFileSync(path))
  const columns = Object.fromEntries(raw.schema.fields.filter(field => field.name !== 'source_id')
    .map(field => [field.name, raw.getChild(field.name)!]))
  for (const category of ['passenger', 'freight']) {
    for (const period of ['day', 'evening', 'night']) {
      columns[`trains_${category}_${period}`] = vectorFromArray([100, 200], new Float64())
    }
    columns[`${category}_status`] = vectorFromArray([2, 2], new Uint8())
    columns[`${category}_source_id`] = vectorFromArray([100, 100], new Uint16())
    columns[`${category}_matching`] = vectorFromArray([1, 1], new Uint8())
  }
  const table = new Table(columns)
  const schema = new Schema(table.schema.fields, new Map([...raw.schema.metadata, ['rail_traffic_contract', '1']]))
  writeFileSync(path, tableToIPC(new Table(schema, table.batches.map(batch => new RecordBatch(schema, batch.data))), 'file'))
  const sidecar = openRailTrafficSidecar(prepared)
  sidecar.close()
  return { prepared, path }
}

test('finalized child rows restore once for graph and source replay without duplicating sidecar evidence', async () => {
  const { prepared, path } = finalizedFixture('parent-replay')
  const sidecar = openRailTrafficSidecar(prepared)
  sidecar.prepare(`INSERT INTO rail_interval VALUES (?,50000,0,0,1000,0,9864,'FR',17,0,2,0,1)`).run(SQUARE)
  sidecar.close()
  const segments = collectZ9RailGraphSegments(prepared, [SQUARE])
  assert.equal(segments.length, 1)
  const restored = tableFromIPC(readFileSync(path))
  assert.equal(restored.numRows, 1)
  assert.equal(restored.schema.metadata.has('rail_traffic_contract'), false)
  assert.equal(restored.schema.metadata.has('qm_blocks'), false)
  assert.equal(restored.schema.metadata.get('railways_contract'), 'country_baked_v1')
  assert.equal(restored.getChild('trains_passenger_day'), null)
  assert.equal(restored.getChild('source_id')!.get(0), 0)
  assert.deepEqual([restored.getChild('start_gx')!.get(0), restored.getChild('start_gy')!.get(0)], lonLatToGrid(4.8, 45.75))
  assert.deepEqual([restored.getChild('end_gx')!.get(0), restored.getChild('end_gy')!.get(0)], lonLatToGrid(4.802, 45.75))
  const bytes = readFileSync(path)
  for (let retry = 0; retry < 2; retry++) {
    const result = await writeRailwayTraffic(path, () => ({ passenger: 6, freight: 0, sourceId: 100 }),
      undefined, { countryIso: 'FR', retract: { sourceIds: [100], when: () => true } })
    assert.equal(result.matched, 1)
    assert.deepEqual(listRailIntervals(prepared).map(row => [row.sourceId, row.passenger]).sort(), [[100, 6], [9864, 17]])
    assert.deepEqual(readFileSync(path), bytes)
  }
})

test('conflicting attributes or incomplete child coverage fail before replacing finalized evidence', () => {
  for (const [name, conflict, geometry, error] of [
    ['parent-conflict', true, 'normal', /children disagree on name/],
    ['parent-gap', false, 'gap', /children do not cover source parent/],
  ] as const) {
    const { prepared, path } = finalizedFixture(name, conflict, geometry)
    const before = readFileSync(path)
    using topology = new SourceTransportTopology(prepared)
    assert.throws(() => restoreRailwayParentsForEnrichment(path, prepared, SQUARE, topology), error)
    assert.deepEqual(readFileSync(path), before)
  }
})

test('dateline children retain the source endpoints across the grid edge', () => {
  const { prepared, path } = finalizedFixture('parent-dateline', false, 'dateline')
  using topology = new SourceTransportTopology(prepared)
  const restored = restoreRailwayParentsForEnrichment(path, prepared, SQUARE, topology)
  assert.equal(restored.numRows, 1)
  assert.deepEqual([restored.getChild('start_gx')!.get(0), restored.getChild('start_gy')!.get(0)], lonLatToGrid(179.8, 45.75))
  assert.deepEqual([restored.getChild('end_gx')!.get(0), restored.getChild('end_gy')!.get(0)], lonLatToGrid(-179.8, 45.75))
})
