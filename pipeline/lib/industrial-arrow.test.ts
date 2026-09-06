/** Actual IPC proof of global site election, priority-safe suppression, and immutable native payload. */

import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { Binary, Field, makeTable, RecordBatch, Schema, Table, Utf8, vectorFromArray, tableFromIPC, tableToIPC } from 'apache-arrow'
import { GEM_COUNTRIES } from './industrial-gem-countries.js'
import { gemIndustrialOwnership } from './industrial-gem-source.js'
import { gridToLonLat, iso2Code } from './prepared-grid.js'
import { PROVENANCE_RANK, SOURCES_BY_ID } from './sources.js'
import { enrichIndustrialFacilities } from './industrial-arrow.js'
import { type MatchFacility } from './facility-match.js'

interface Row { gx: number; gy: number; subtype?: number; source?: number; nace?: number; area?: number; sourceType?: number; suppressed?: number; country?: string }
function stored(path: string, rows: Row[]): Table {
  let table = makeTable({
    osm_id: BigInt64Array.from(rows, (_, i) => BigInt(i + 1)),
    centroid_gx: Int32Array.from(rows, r => r.gx), centroid_gy: Int32Array.from(rows, r => r.gy),
    site_subtype: Uint8Array.from(rows, r => r.subtype ?? 0), source_type: Uint8Array.from(rows, r => r.sourceType ?? 0),
    area_m2: Float32Array.from(rows, r => r.area ?? 10_000), source_id: Uint16Array.from(rows, r => r.source ?? 0),
    nace_4digit: Uint16Array.from(rows, r => r.nace ?? 0), suppressed: Uint8Array.from(rows, r => r.suppressed ?? 0),
    name: vectorFromArray(rows.map((_, i) => `site-${i}`), new Utf8()),
    hub_height: Float32Array.from(rows, () => 80), rated_power_kw: Float32Array.from(rows, () => 2000),
    geom: vectorFromArray(rows.map(() => new Uint8Array([1, 2, 3])), new Binary()),
  } as never) as unknown as Table
  if (rows.some(row => row.country !== undefined)) {
    table = table.assign(makeTable({ country_iso: Uint16Array.from(rows, row => row.country ? iso2Code(row.country) : 0) }))
  }
  const fields = table.schema.fields.map(f => new Field(f.name, f.type, f.name === 'geom', new Map([['field', f.name]])))
  const parts = rows.length > 1 ? [table.slice(0, 1), table.slice(1)] : [table]
  const schema = new Schema(fields, new Map([['grid', 'z30'], ['qm_batch_bboxes', JSON.stringify(parts.map(() => [49, 13, 51, 16]))], ['native', 'preserve'], ...(rows.some(row => row.country !== undefined) ? [['industrial_contract', 'country_land_baked_v1'] as [string, string]] : [])]))
  const result = new Table(schema, parts.flatMap(part => part.batches.map(batch => new RecordBatch(schema, batch.data))))
  mkdirSync(resolve(path, '..'), { recursive: true }); writeFileSync(path, tableToIPC(result, 'file'))
  return tableFromIPC(readFileSync(path))
}
function facility(gx: number, gy: number, id = 300, nace4 = 3511): MatchFacility {
  const source = SOURCES_BY_ID.get(id)!
  return { ...gridToLonLat(gx, gy), id, nace4, rank: PROVENANCE_RANK[source.provenance], year: source.year ?? 0 }
}
const values = (path: string, name: string) => [...tableFromIPC(readFileSync(path)).getChild(name)!]
function unchangedNative(before: Table, after: Table) {
  assert.deepEqual(after.schema.fields, before.schema.fields)
  assert.deepEqual(after.schema.metadata, before.schema.metadata)
  assert.deepEqual(after.batches.map(b => b.numRows), before.batches.map(b => b.numRows))
  for (const field of before.schema.fields) {
    if (['nace_4digit', 'source_id', 'suppressed'].includes(field.name)) continue
    assert.deepEqual(after.getChild(field.name)!.toArray(), before.getChild(field.name)!.toArray(), field.name)
  }
}

test('one original facility elects one site across z9 seam; native payload, retraction and exact rerun survive', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-ipc-'))
  try {
    const gx = 276 * 2 ** 21, gy = 709600000
    const a = resolve(work, 'z9/275/173/industrial.arrow'), b = resolve(work, 'z9/276/173/industrial.arrow')
    const original = stored(a, [{ gx: gx - 500, gy }, { gx: gx - 10000, gy, source: 300, nace: 3511 },
      { gx: gx - 50, gy, sourceType: 10 }])
    stored(b, [{ gx: gx + 100, gy }])
    const f = facility(gx + 50, gy)
    const run = await enrichIndustrialFacilities(work, [f], [300])
    assert.equal(run.winners, 1); assert.equal(run.stamped, 1)
    assert.deepEqual(values(a, 'source_id'), [0, 0, 0]); assert.deepEqual(values(b, 'source_id'), [300])
    unchangedNative(original, tableFromIPC(readFileSync(a)))
    const before = [a, b].map(path => ({ bytes: readFileSync(path), stat: statSync(path, { bigint: true }) }))
    assert.equal((await enrichIndustrialFacilities(work, [f], [300])).squaresUpdated, 0)
    for (const [i, path] of [a, b].entries()) {
      assert.deepEqual(readFileSync(path), before[i].bytes)
      for (const field of ['ino', 'mtimeNs', 'ctimeNs'] as const) assert.equal(statSync(path, { bigint: true })[field], before[i].stat[field])
    }
    assert.equal((await enrichIndustrialFacilities(work, [], [300])).stamped, 0)
    assert.deepEqual(values(b, 'source_id'), [0], 'admitted source with no current winner heals stale own stamps')
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('duplicate suppression uses published incumbent authority in both directions and preserves payload', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-dedup-'))
  try {
    const gx = 578800000, gy = 709600000
    const path = resolve(work, 'z9/275/173/industrial.arrow')
    const rows = [{ gx, gy, subtype: 6, area: 1_200_000, source: 320, nace: 2410 },
      { gx, gy: gy + 1500, area: 1_200_000 }]
    const original = stored(path, rows)
    const facilities = [facility(gx, gy, 331, 2410), facility(gx, gy + 1500, 310, 3511)]
    await enrichIndustrialFacilities(work, facilities, [310, 331])
    assert.deepEqual(values(path, 'source_id'), [320, 310])
    assert.deepEqual(values(path, 'suppressed'), [0, 1], 'national320 defeats applied310; rejected331 has no authority')
    assert.deepEqual(values(path, 'nace_4digit'), [2410, 3511])
    unchangedNative(original, tableFromIPC(readFileSync(path)))
    const nationalBytes = readFileSync(path), nationalStat = statSync(path, { bigint: true })
    assert.equal((await enrichIndustrialFacilities(work, facilities, [310, 331])).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), nationalBytes)
    assert.equal(statSync(path, { bigint: true }).mtimeNs, nationalStat.mtimeNs)
    for (const retainedId of [330, 9000]) {
      const retained = stored(path, [{ ...rows[0], source: retainedId }, rows[1],
        { gx, gy: gy + 1_000_000, source: 330, nace: 2410, suppressed: 1 },
        { gx, gy, source: 330, nace: 3511, sourceType: 10, suppressed: 1 }])
      await enrichIndustrialFacilities(work, [facilities[1]], [310, 331])
      assert.deepEqual(values(path, 'source_id'), [retainedId, 310, 330, 330])
      assert.deepEqual(values(path, 'nace_4digit'), [2410, 3511, 2410, 3511])
      assert.deepEqual(values(path, 'suppressed'), [1, 0, 1, 1], 'applied310 defeats the retained classification')
      const lowerBytes = readFileSync(path)
      assert.equal((await enrichIndustrialFacilities(work, [facilities[1]], [310, 331])).squaresUpdated, 0)
      assert.deepEqual(readFileSync(path), lowerBytes)
      await enrichIndustrialFacilities(work, [], [310, 331])
      assert.deepEqual(values(path, 'source_id'), [retainedId, 0, 330, 330])
      assert.deepEqual(values(path, 'nace_4digit'), [2410, 0, 2410, 3511])
      assert.deepEqual(values(path, 'suppressed'), [0, 1, 1, 1], 'retired global310 wakes its verified incumbent; its duplicate baseline stays suppressed')
      unchangedNative(retained, tableFromIPC(readFileSync(path)))
      const retiredBytes = readFileSync(path), retiredStat = statSync(path, { bigint: true })
      assert.equal((await enrichIndustrialFacilities(work, [], [310, 331])).squaresUpdated, 0)
      assert.deepEqual(readFileSync(path), retiredBytes)
      assert.equal(statSync(path, { bigint: true }).mtimeNs, retiredStat.mtimeNs)
    }
    stored(path, [rows[0], rows[1], { gx, gy: gy + 1_000_000, source: 330, nace: 2410, suppressed: 1 }])
    await enrichIndustrialFacilities(work, facilities, [310, 331])
    assert.deepEqual(values(path, 'suppressed'), [0, 1, 1], 'nonparticipant suppression is untouched')
    stored(path, [{ ...rows[0], source: 0, nace: 0 }, rows[1]])
    await enrichIndustrialFacilities(work, facilities, [310, 331])
    assert.deepEqual(values(path, 'source_id'), [331, 310])
    assert.deepEqual(values(path, 'suppressed'), [1, 0])
    await enrichIndustrialFacilities(work, [facilities[0]], [310, 331])
    assert.deepEqual(values(path, 'suppressed'), [0, 1], 'retired registry clears its stamp while its verified duplicate baseline stays suppressed')
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('corrupt later IPC aborts before any earlier write; valid empty IPC remains valid', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-admission-'))
  try {
    const path = resolve(work, 'z9/275/173/industrial.arrow')
    stored(path, [{ gx: 578800000, gy: 709600000 }]); const bytes = readFileSync(path)
    const corrupt = resolve(work, 'z9/276/173/industrial.arrow')
    mkdirSync(resolve(corrupt, '..'), { recursive: true }); writeFileSync(corrupt, 'broken')
    await assert.rejects(enrichIndustrialFacilities(work, [facility(578800000, 709600000)], [300]))
    assert.deepEqual(readFileSync(path), bytes)
    stored(corrupt, []); const empty = readFileSync(corrupt)
    await enrichIndustrialFacilities(work, [facility(578800000, 709600000)], [300])
    assert.deepEqual(readFileSync(corrupt), empty)
  } finally { rmSync(work, { recursive: true, force: true }) }
})


test('shared330 scopes survive sibling reruns, foreign rows and retirement restores an incumbent emitter', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-country-'))
  try {
    // All rows deliberately share one tile: the baked original row owns the write.
    const lon = -2, lat = 12
    const gx = Math.floor(lon / 360 * 2 ** 30) + 2 ** 29
    const gy = Math.floor(Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360)) / (2 * Math.PI) * 2 ** 30) + 2 ** 29
    const path = resolve(work, 'z9/253/238/industrial.arrow')
    const rows: Row[] = [
      { gx, gy, country: 'BF', source: 330, nace: 3511, area: 1_200_000 },
      { gx, gy: gy + 1500, country: 'BF', source: 300, nace: 3511, area: 1_200_000, suppressed: 1 },
      { gx, gy, country: 'ML', source: 330, nace: 3512 },
      { gx, gy, country: 'BR', source: 330, nace: 3512, suppressed: 1 },
      { gx, gy, country: '', source: 330, nace: 3512 },
      { gx, gy: gy + 1_000_000, country: 'BF', source: 300, nace: 3511, area: 1_200_000, suppressed: 1 },
      { gx, gy, country: 'BF', source: 330, nace: 3511, sourceType: 10, suppressed: 1 },
    ]
    const original = stored(path, rows)
    const bf = GEM_COUNTRIES.find(p => p.country === 'BF')!, ml = GEM_COUNTRIES.find(p => p.country === 'ML')!
    await enrichIndustrialFacilities(work, [facility(gx, gy, 330)], [330], gemIndustrialOwnership([bf], ['BF']))
    assert.deepEqual(values(path, 'source_id'), [330, 300, 330, 330, 330, 300, 330])
    assert.deepEqual(values(path, 'suppressed'), [0, 1, 0, 1, 0, 1, 1])
    await enrichIndustrialFacilities(work, [], [330], gemIndustrialOwnership([bf], []))
    assert.deepEqual(values(path, 'source_id'), [0, 300, 330, 330, 330, 300, 330])
    assert.deepEqual(values(path, 'suppressed'), [1, 0, 0, 1, 0, 1, 1], 'retired330 releases300 while its duplicate OSM baseline stays suppressed; foreign and unowned rows stay exact')
    unchangedNative(original, tableFromIPC(readFileSync(path)))
    const before = readFileSync(path)
    assert.equal((await enrichIndustrialFacilities(work, [], [330], gemIndustrialOwnership([bf], []))).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), before)
    await enrichIndustrialFacilities(work, [facility(gx, gy, 330, 3599)], [330], gemIndustrialOwnership([ml], ['ML']))
    assert.deepEqual(values(path, 'nace_4digit'), [0, 3511, 3599, 3512, 3512, 3511, 3511])
    stored(path, [{ gx, gy }]); const missing = readFileSync(path)
    await assert.rejects(enrichIndustrialFacilities(work, [facility(gx, gy, 330)], [330], gemIndustrialOwnership([bf], ['BF'])), /country_land_baked_v1/)
    assert.deepEqual(readFileSync(path), missing)
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('national facility horizon reaches Paraguay border registry beyond the global default, without claiming foreign rows', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-horizon-'))
  try {
    const gx = Math.round((-58 / 360 + .5) * 2 ** 30)
    const gy = Math.round((Math.log(Math.tan(Math.PI / 4 - 25 * Math.PI / 360)) / (2 * Math.PI) + .5) * 2 ** 30)
    const origin = facility(gx, gy, 330, 3512)
    // 0.025 degrees longitude is between two and three kilometres here.
    const targetGx = gx + Math.round(.025 / 360 * 2 ** 30)
    const path = resolve(work, 'z9/173/292/industrial.arrow')
    const before = stored(path, [{ gx: targetGx, gy, country: 'PY' }, { gx, gy, country: 'BR', source: 330, nace: 3511 }])
    const scope = [{ country: 'PY', bbox: [-27.7, -62.7, -19.3, -54.2] as const }]
    const ownership = gemIndustrialOwnership(scope, ['PY'])
    await enrichIndustrialFacilities(work, [{ ...origin, searchRadiusM: 3000 }], [330], ownership)
    assert.deepEqual(values(path, 'source_id'), [330, 330])
    assert.deepEqual(values(path, 'nace_4digit'), [3512, 3511])
    unchangedNative(before, tableFromIPC(readFileSync(path)))
    const bytes = readFileSync(path)
    assert.equal((await enrichIndustrialFacilities(work, [{ ...origin, searchRadiusM: 3000 }], [330], ownership)).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), bytes)
    await enrichIndustrialFacilities(work, [{ ...origin, searchRadiusM: 1500 }], [330], ownership)
    assert.deepEqual(values(path, 'source_id'), [0, 330], 'each facility retains its own horizon and admitted retirement scope')
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('final containment tier participates in one priority election and retirement without changing foreign or native rows', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-tier-'))
  try {
    const gx = Math.round((-74 / 360 + .5) * 2 ** 30)
    const gy = Math.round((Math.log(Math.tan(Math.PI / 4 + 5 * Math.PI / 360)) / (2 * Math.PI) + .5) * 2 ** 30)
    const path = resolve(work, 'z9/150/248/industrial.arrow')
    const original = stored(path, [{ gx, gy, country: 'CO', area: 1_200_000 },
      { gx, gy: gy + 1500, country: 'CO', source: 320, nace: 2410, area: 1_200_000 },
      { gx, gy, country: 'VE', source: 330, nace: 1900, suppressed: 1 },
      { gx, gy, country: 'CO', sourceType: 10, source: 330, nace: 3511, suppressed: 1 }])
    const scope = [{ country: 'CO', bbox: [-4.3, -82, 13.5, -66.8] as const }]
    const ownership = gemIndustrialOwnership(scope, ['CO'])
    ownership.rowClassification = (_country, polygon) => ({ ...facility(gx, gy, 330, 700), lat: polygon.lat, lon: polygon.lon })
    const point = facility(gx, gy, 330, 3599)
    await enrichIndustrialFacilities(work, [point], [330], ownership)
    assert.deepEqual(values(path, 'source_id'), [330, 320, 330, 330])
    assert.deepEqual(values(path, 'nace_4digit'), [700, 2410, 1900, 3511], 'final concession replaces point class; higher stored authority survives')
    assert.deepEqual(values(path, 'suppressed'), [1, 0, 1, 1])
    unchangedNative(original, tableFromIPC(readFileSync(path)))
    const bytes = readFileSync(path)
    assert.equal((await enrichIndustrialFacilities(work, [point], [330], ownership)).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), bytes)
    await enrichIndustrialFacilities(work, [], [330], gemIndustrialOwnership(scope, []))
    assert.deepEqual(values(path, 'source_id'), [0, 320, 330, 330])
    assert.deepEqual(values(path, 'suppressed'), [1, 0, 1, 1])
  } finally { rmSync(work, { recursive: true, force: true }) }
})
