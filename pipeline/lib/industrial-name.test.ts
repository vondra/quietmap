/** Native IPC name priors retain measured authority, duplicate suppression and repeatable lifecycle. */

import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { Field, makeTable, RecordBatch, Schema, Table, Utf8, vectorFromArray, tableFromIPC, tableToIPC } from 'apache-arrow'
import { enrichIndustrialNames, industrialNameRule } from './industrial-name.js'

interface Row { name: string | null; source?: number; nace?: number; wind?: boolean; suppressed?: number }
function store(path: string, rows: Row[], fresh = false) {
  let table = makeTable({ osm_id: BigInt64Array.from(rows, (_, i) => BigInt(i + 1)),
    centroid_gx: Int32Array.from(rows, () => 578800000), centroid_gy: Int32Array.from(rows, () => 709600000),
    source_id: Uint16Array.from(rows, r => r.source ?? 0), source_type: Uint8Array.from(rows, r => r.wind ? 10 : 0),
    suppressed: Uint8Array.from(rows, r => r.suppressed ?? 0),
    name: vectorFromArray(rows.map(r => r.name), new Utf8()),
    hub_height: Float32Array.from(rows, () => 80), rated_power_kw: Float32Array.from(rows, () => 2000),
  } as never) as unknown as Table
  if (!fresh) table = table.assign(makeTable({ nace_4digit: Uint16Array.from(rows, r => r.nace ?? 0) }))
  const parts = rows.length > 1 ? [table.slice(0, 1), table.slice(1)] : [table]
  const schema = new Schema(table.schema.fields.map(f => new Field(f.name, f.type, f.nullable, new Map([['original', f.name]]))),
    new Map([['grid', 'z30'], ['native', 'preserve'], ['qm_batch_bboxes', JSON.stringify(parts.map(() => [49, 13, 51, 16]))]]))
  const result = new Table(schema, parts.flatMap(p => p.batches.map(b => new RecordBatch(schema, b.data))))
  mkdirSync(resolve(path, '..'), { recursive: true }); writeFileSync(path, tableToIPC(result, 'file'))
  return tableFromIPC(readFileSync(path))
}
const read = (path: string) => tableFromIPC(readFileSync(path))
const values = (path: string, name: string) => [...read(path).getChild(name)!]
function preserved(before: Table, after: Table) {
  assert.deepEqual(after.schema.metadata, before.schema.metadata)
  assert.deepEqual(after.batches.map(b => b.numRows), before.batches.map(b => b.numRows))
  for (const field of before.schema.fields) {
    assert.deepEqual(after.schema.fields.find(f => f.name === field.name), field)
    if (!['source_id', 'nace_4digit'].includes(field.name)) assert.deepEqual(after.getChild(field.name)!.toArray(), before.getChild(field.name)!.toArray(), field.name)
  }
}

test('ordered multilingual rules retain wind skip, solar precedence and all original classes', () => {
  const examples: Array<[string, number | undefined]> = [
    ['SOLÁRNÍ elektrárna', 3599], ['wind farm power plant', 0], ['Elektrárna', 3511], ['Kamenolom ', 700],
    ['Pivovar', 1000], ['Textile', 1300], ['Sägewerk', 1600], ['Rafinérie', 2000], ['Betonárna', 2300],
    ['Foundry', 2400], ['Car factory', 2900], ['Čistírna', 3800], ['Logistics', 5200], ['Farma', 100],
    ['Wind turbine factory', 0], ['Unnamed industrial site', undefined],
  ]
  for (const [name, nace] of examples) assert.equal(industrialNameRule(name)?.nace4, nace, name)
})

test('native names preserve authority and suppression while owned retirement, wind and re-extraction converge', async () => {
  const root = mkdtempSync(resolve(tmpdir(), 'industrial-name-ipc-'))
  try {
    const path = resolve(root, 'z9/275/173/industrial.arrow')
    const rows: Row[] = [{ name: 'Solar power plant' }, { name: 'Brewery', source: 310, nace: 3511 },
      { name: 'Warehouse', source: 9000, nace: 3511, suppressed: 1 },
      { name: null, source: 9000, nace: 3511, suppressed: 1 },
      { name: 'Wind farm power plant', source: 9000, nace: 3511 },
      { name: 'Solar power plant', source: 330, nace: 3511, wind: true, suppressed: 1 },
      { name: 'Solar power plant', wind: true }]
    const original = store(path, rows)
    const first = await enrichIndustrialNames(root)
    assert.equal(first.classified, 2); assert.equal(first.retired, 2)
    assert.deepEqual(values(path, 'source_id'), [9000, 310, 9000, 0, 0, 330, 0])
    assert.deepEqual(values(path, 'nace_4digit'), [3599, 3511, 5200, 0, 0, 3511, 0])
    assert.deepEqual(values(path, 'suppressed'), [0, 0, 1, 1, 0, 1, 0])
    preserved(original, read(path))
    const bytes = readFileSync(path), stat = statSync(path, { bigint: true })
    assert.equal((await enrichIndustrialNames(root)).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), bytes)
    for (const field of ['ino', 'mtimeNs', 'ctimeNs'] as const) assert.equal(statSync(path, { bigint: true })[field], stat[field])
    store(path, rows); await enrichIndustrialNames(root)
    assert.deepEqual(readFileSync(path), bytes, 're-extracting identical native facts produces identical enrichment')
    const fresh = store(path, [{ name: 'Pivovar' }, { name: 'Unknown' }], true)
    await enrichIndustrialNames(root)
    assert.deepEqual(values(path, 'nace_4digit'), [1000, 0]); preserved(fresh, read(path))
  } finally { rmSync(root, { recursive: true, force: true }) }
})

test('missing scope or malformed native columns fail; valid empty native IPC is unchanged', async () => {
  const root = mkdtempSync(resolve(tmpdir(), 'industrial-name-admission-'))
  try {
    await assert.rejects(enrichIndustrialNames(root), /no industrial Arrow scope/)
    const path = resolve(root, 'z9/275/173/industrial.arrow')
    store(path, []); const empty = readFileSync(path)
    assert.equal((await enrichIndustrialNames(root)).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), empty)
    const valid = store(path, [{ name: 'Pivovar' }])
    const broken = valid.assign(makeTable({ source_type: Int16Array.of(10) }))
    const schema = new Schema(broken.schema.fields, valid.schema.metadata)
    writeFileSync(path, tableToIPC(new Table(schema, broken.batches.map(batch => new RecordBatch(schema, batch.data))), 'file'))
    const bad = readFileSync(path)
    await assert.rejects(enrichIndustrialNames(root), /source_type.*Uint8/)
    assert.deepEqual(readFileSync(path), bad)
  } finally { rmSync(root, { recursive: true, force: true }) }
})
