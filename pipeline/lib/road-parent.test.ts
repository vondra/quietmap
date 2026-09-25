/** A finalized road generation restores to raw parents that the shared traffic writer accepts. */
import { osmContract } from './osm-contract.js'

import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, test } from 'node:test'
import { Float32, Float64, Int16, Int32, Int64, RecordBatch, Schema, Table, Uint8, Uint16, Utf8, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { iso2Code, lonLatToGrid } from './prepared-grid.js'
import { restoreRoadParentsForEnrichment } from './road-parent.js'
import { writeRoadAadt } from './roads-arrow.js'
import { writeTransportFixture } from './transport-test-fixture.js'
import { SourceTransportTopology } from './transport-topology.js'

const SQUARE = 'z9/262/182'
const DIRECTORY = mkdtempSync(join(tmpdir(), 'road-parent-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

/** Way 60000 piece 0 was cut at 4.801; way 60001 piece 0 stayed whole and its subpixel piece 1 was dropped;
 *  way 60002 piece 0 lost a subpixel child, so its lone survivor spans the parent with a shortened length. */
function finalizedFixture(name: string, secondChildName: string, orphanWay?: string) {
  const prepared = join(DIRECTORY, name, '2026')
  mkdirSync(join(prepared, SQUARE), { recursive: true })
  const path = join(prepared, SQUARE, 'roads.arrow')
  const points = [[4.8, 4.801], [4.801, 4.802], [4.81, 4.811], [4.82, 4.821]].map(([from, to]) =>
    [...lonLatToGrid(from, 45.75), ...lonLatToGrid(to, 45.75)])
  const column = (axis: number) => vectorFromArray(points.map(point => point[axis]), new Int32())
  const finalized = new Table({
    osm_id: vectorFromArray([60000n, 60000n, 60001n, 60002n], new Int64()),
    segment_idx: vectorFromArray([0, 0, 0, 0], new Int16()),
    start_gx: column(0), start_gy: column(1), end_gx: column(2), end_gy: column(3),
    length_m: vectorFromArray([77.7, 77.7, 77.7, 77.66], new Float32()),
    road_class: vectorFromArray([0, 0, 5, 5], new Uint8()),
    name: vectorFromArray(['Ring', secondChildName, null, null], new Utf8()),
    country_iso: vectorFromArray([1, 1, 1, 1].map(() => iso2Code('FR')), new Uint16()),
    source_id: vectorFromArray([10, 10, 0, 0], new Uint16()),
    traffic_profile_id: vectorFromArray([1, 1, 0, 0], new Uint16()),
    aadt_light: vectorFromArray([500, 250, 40, 40], new Float64()),
    traffic_estimated: vectorFromArray([0, 15, 15, 15], new Uint8()),
  })
  const schema = new Schema(finalized.schema.fields, new Map([osmContract('roads'), ['grid', 'z30'], ['roads_contract', 'country_baked_v1'],
    ['road_traffic_contract', '1'], ['qm_blocks', 'stale'], ['roads_time_profiles', '{"source":"https://example.test/profiles","entries":[]}']]))
  writeFileSync(path, tableToIPC(new Table(schema, finalized.batches.map(batch => new RecordBatch(schema, batch.data))), 'file'))
  const whole = { segment: 0, square: SQUARE, start: [0, 0] as [number, number], end: [1, 0] as [number, number] }
  writeTransportFixture(prepared, [
    { id: '60000', family: 'roads', nodes: [['1', [45.75, 4.8]], ['2', [45.75, 4.802]]] },
    { id: '60001', family: 'roads', nodes: [['3', [45.75, 4.81]], ['4', [45.75, 4.811]], ['5', [45.75, 4.8110001]]] },
    { id: '60002', family: 'roads', nodes: [['6', [45.75, 4.82]], ['7', [45.75, 4.821]]] },
    ...(orphanWay ? [{ id: orphanWay, family: 'roads' as const, nodes: [['8', [45.75, 4.83]], ['9', [45.75, 4.8300001]]] as [string, [number, number]][] }] : []),
  ], [{ way: '60001', segment: 1, square: SQUARE, start: [1, 0], end: [2, 0] }, { way: '60000', ...whole },
    { way: '60001', ...whole }, { way: '60002', ...whole }, ...(orphanWay ? [{ way: orphanWay, ...whole }] : [])])
  return { prepared, path }
}

test('finalized children restore to raw parents that enrichment accepts, and a raw file stays byte-identical', async () => {
  const { prepared, path } = finalizedFixture('restore', 'Ring')

  using topology = new SourceTransportTopology(prepared, 'roads')
  assert.deepEqual(restoreRoadParentsForEnrichment(path, SQUARE, topology), { rows: 4, parents: 4, restored: true })
  const raw = tableFromIPC(readFileSync(path))
  assert.deepEqual(raw.schema.fields.map(field => field.name), ['osm_id', 'segment_idx', 'start_gx', 'start_gy',
    'end_gx', 'end_gy', 'length_m', 'road_class', 'name', 'country_iso', 'source_id'])
  assert.deepEqual([...raw.schema.metadata.keys()].sort(), ['grid', 'osm_roads_contract', 'roads_contract'])
  assert.deepEqual([...raw.getChild('source_id')!], [0, 0, 0, 0])
  assert.deepEqual([...raw.getChild('segment_idx')!], [0, 0, 0, 1])
  assert.deepEqual([...raw.getChild('road_class')!], [0, 5, 5, 5])
  assert.deepEqual(['start_gx', 'start_gy', 'end_gx', 'end_gy'].map(name => raw.getChild(name)!.get(0)),
    [...lonLatToGrid(4.8, 45.75), ...lonLatToGrid(4.802, 45.75)])
  // The merged parent and the lone survivor regain the extractor's one-decimal source length; the whole row keeps its own.
  assert.deepEqual([...raw.getChild('length_m')!], [Math.fround(155.4), Math.fround(77.7), Math.fround(77.7), 0])

  const bytes = readFileSync(path)
  assert.equal(restoreRoadParentsForEnrichment(path, SQUARE, topology).restored, false)
  assert.deepEqual(readFileSync(path), bytes)
  const written = await writeRoadAadt(path, () => ({ light: 900, medium: 0, heavy: 0, moto: 0, sourceId: 10,
    countBasis: 'unknown', observationId: 'fixture-count' }))
  assert.equal(written.matched, 4)
})

test('disagreeing children or a dropped parent without a donor fail before replacing finalized evidence', () => {
  for (const [name, secondChildName, orphanWay, error] of [
    ['conflict', 'Other', undefined, /road children disagree on name for 60000:0/],
    ['no-donor', 'Ring', '60009', /no finalized piece of way 60009/],
  ] as const) {
    const { prepared, path } = finalizedFixture(name, secondChildName, orphanWay)
    const before = readFileSync(path)
    using topology = new SourceTransportTopology(prepared, 'roads')
    assert.throws(() => restoreRoadParentsForEnrichment(path, SQUARE, topology), error)
    assert.deepEqual(readFileSync(path), before)
  }
})
