/** Actual IPC proof of national building refinements, unchanged shape, and missing input gates. */

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { Binary, Field, makeTable, RecordBatch, Schema, Table, Utf8, vectorFromArray, tableFromIPC, tableToIPC } from 'apache-arrow'
import { gridToLonLat } from './lib/prepared-grid.js'
import { writeBuildingEnrichment } from './lib/buildings-arrow.js'
import { NATIONAL_BUILDING_SOURCES, indexNationalBuildings } from './lib/buildings-national-source.js'
import { enrichNationalBuildings } from './enrich-buildings-national.js'

function buildingTable(gx: number[], gy: number[], floors: number[], types: number[], sources: number[]) {
  const n = gx.length
  const table = makeTable({ osm_id: BigInt64Array.from(gx, (_, i) => BigInt(i + 1)),
    centroid_gx: Int32Array.from(gx), centroid_gy: Int32Array.from(gy),
    floors: Uint8Array.from(floors), building_type: Uint8Array.from(types), source_id: Uint16Array.from(sources),
    area_m2: Float32Array.from(gx, () => 123), building_use: new Uint8Array(n),
    height: Float32Array.from(gx, () => 0), name: vectorFromArray(gx.map((_, i) => `building-${i}`), new Utf8()),
    addr_street: vectorFromArray(gx.map(() => ''), new Utf8()), addr_housenumber: vectorFromArray(gx.map(() => ''), new Utf8()),
    opening_hours_frac: new Uint8Array(n), geom: vectorFromArray(gx.map(() => new Uint8Array([7, 8, 9])), new Binary()),
  } as never) as unknown as Table
  const fields = table.schema.fields.map(f => new Field(f.name, f.type, f.name === 'height', new Map([['field-note', f.name]])))
  const schema = new Schema(fields, new Map([
    ['grid', 'z30'], ['buildings_contract', 'buildings_v3'], ['qm_batch_bboxes', '["batch-a","batch-b"]'], ['extra', 'preserve'],
  ]))
  const parts = n > 1 ? [table.slice(0, 2), table.slice(2)] : [table]
  return new Table(schema, parts.flatMap(part => part.batches.map(batch => new RecordBatch(schema, batch.data))))
}

function store(path: string, table: Table) { mkdirSync(resolve(path, '..'), { recursive: true }); writeFileSync(path, tableToIPC(table, 'file')) }
function assertUntouched(before: Table, after: Table) {
  before = tableFromIPC(tableToIPC(before, 'file'))
  assert.deepEqual(after.schema.metadata, before.schema.metadata)
  assert.deepEqual(after.schema.fields, before.schema.fields)
  assert.deepEqual(after.batches.map(b => b.numRows), before.batches.map(b => b.numRows))
  for (const field of before.schema.fields) {
    if (['floors', 'building_type', 'source_id'].includes(field.name)) continue
    assert.deepEqual(after.getChild(field.name)!.toArray(), before.getChild(field.name)!.toArray(), field.name)
  }
}

test('CZ seam matches preserve specific types, existing floors, source rank and exact IPC reruns', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'national-cz-ipc-'))
  const path = resolve(work, 'z9/275/173/buildings.arrow')
  // A building west of the x=276 boundary matches an original observation east of it.
  const gx = Array.from({ length: 6 }, () => 276 * 2 ** 21 - 50)
  const gy = Array.from({ length: 6 }, (_, i) => 709600000 + i * 20000)
  const original = buildingTable(gx, gy, [0, 0, 9, 0, 0, 0], [0, 11, 2, 0, 0, 0], [0, 0, 0, 201, 0, 0])
  store(path, original)
  const points = gx.slice(0, 5).map((x, i) => ({ ...gridToLonLat(x + 100, gy[i]), floors: [3, 4, 5, 6, 0][i], useCode: [10, 7, 9, 10, 7][i] }))
  const source = resolve(work, 'cz.json'); writeFileSync(source, JSON.stringify(points))
  const index = await indexNationalBuildings(source, resolve(work, 'cz.sqlite'), NATIONAL_BUILDING_SOURCES[0])
  try {
    const result = await enrichNationalBuildings(work, index)
    assert.equal(result.matched, 3); assert.equal(result.floorsAdded, 2); assert.equal(result.typesChanged, 2)
    assert.equal(result.typeDowngradesBlocked, 1)
    const after = tableFromIPC(readFileSync(path))
    assert.deepEqual([...after.getChild('floors')!], [3, 4, 9, 0, 0, 0])
    assert.deepEqual([...after.getChild('building_type')!], [1, 11, 9, 0, 0, 0])
    assert.deepEqual([...after.getChild('source_id')!], [200, 200, 200, 201, 0, 0])
    assertUntouched(original, after)
    const bytes = readFileSync(path), before = statSync(path, { bigint: true })
    assert.equal((await enrichNationalBuildings(work, index)).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), bytes)
    const later = statSync(path, { bigint: true })
    for (const field of ['ino', 'size', 'mtimeNs', 'ctimeNs'] as const) assert.equal(later[field], before[field])
    const missing = resolve(work, 'z9/276/173/roads.arrow')
    store(missing, original)
    await assert.rejects(enrichNationalBuildings(work, index), /missing buildings.arrow/)
    assert.deepEqual(readFileSync(path), bytes, 'all expected building tables checked before writes')
  } finally { index.close(); rmSync(work, { recursive: true, force: true }) }
})

test('ES fills floors only; current empty IPC is valid but missing/legacy/malformed contracts fail', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'national-es-ipc-'))
  const path = resolve(work, 'z9/251/194/buildings.arrow')
  const gx = [527900000, 527920000, 527940000], gy = [668000000, 668000000, 668000000]
  const original = buildingTable(gx, gy, [0, 8, 0], [12, 13, 10], [0, 0, 0])
  store(path, original)
  const source = resolve(work, 'es.json')
  writeFileSync(source, gx.map((x, i) => JSON.stringify({ ...gridToLonLat(x, gy[i]), floors: 5 })).join('\n'))
  const index = await indexNationalBuildings(source, resolve(work, 'es.sqlite'), NATIONAL_BUILDING_SOURCES[1])
  try {
    assert.equal((await enrichNationalBuildings(work, index)).floorsAdded, 2)
    const after = tableFromIPC(readFileSync(path))
    assert.deepEqual([...after.getChild('floors')!], [5, 8, 5])
    assert.deepEqual([...after.getChild('source_id')!], [201, 0, 201])
    assertUntouched(original, after)
    const empty = resolve(work, 'z9/252/194/buildings.arrow')
    store(empty, buildingTable([], [], [], [], []))
    const bytes = readFileSync(empty)
    assert.equal((await writeBuildingEnrichment(empty, () => { throw new Error('empty callback'); })).rows, 0)
    assert.deepEqual(readFileSync(empty), bytes)
    await assert.rejects(writeBuildingEnrichment(resolve(work, 'missing.arrow'), () => null), /ENOENT/)
    const legacy = resolve(work, 'legacy.arrow'); store(legacy, makeTable({ floors: new Uint8Array([0]) }))
    await assert.rejects(writeBuildingEnrichment(legacy, () => null), /buildings_v3/)
    const prior = readFileSync(path)
    await assert.rejects(writeBuildingEnrichment(path, () => ({ floors: NaN, sourceId: 201 })), /invalid building refinement/)
    assert.deepEqual(readFileSync(path), prior)
  } finally { index.close(); rmSync(work, { recursive: true, force: true }) }
})


test('CLI requires both source admissions before the first prepared write', () => {
  const work = mkdtempSync(resolve(tmpdir(), 'national-cli-admission-'))
  try {
    const path = resolve(work, 'prepared/z9/275/173/buildings.arrow')
    const gx = 276 * 2 ** 21 - 50, gy = 709600000
    store(path, buildingTable([gx], [gy], [0], [0], [0]))
    const cache = resolve(work, 'source/cz/ruian-buildings.json')
    mkdirSync(resolve(cache, '..'), { recursive: true })
    writeFileSync(cache, JSON.stringify([{ ...gridToLonLat(gx, gy), floors: 7, useCode: 10 }]))
    const before = readFileSync(path), stat = statSync(path, { bigint: true })
    const run = spawnSync(process.execPath, ['--import', import.meta.resolve('tsx'),
      fileURLToPath(new URL('./enrich-buildings-national.ts', import.meta.url)),
      '--prepared-dir', resolve(work, 'prepared'), '--enrichment-dir', resolve(work, 'source')], { encoding: 'utf8' })
    assert.equal(run.status, 1)
    assert.match(run.stderr, /es.*catastro-buildings\.json/)
    assert.deepEqual(readFileSync(path), before)
    assert.equal(statSync(path, { bigint: true }).mtimeNs, stat.mtimeNs)
  } finally { rmSync(work, { recursive: true, force: true }) }
})
