/** Native IPC regression for complete-chain wind field priority and immutable measured payload. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtempSync, readFileSync, writeFileSync, statSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { Float32, Field, makeTable, RecordBatch, Schema, Table, vectorFromArray, tableFromIPC, tableToIPC } from 'apache-arrow'
import { gridToLonLat } from './prepared-grid.js'
import { WIND_COUNTRIES, parseWindCsv, parseWindWorkbook, parseNorwegianWind, loadWindRegisters } from './industrial-wind-source.js'
import { enrichWindSquare, windParameterMatcher } from './industrial-wind-arrow.js'

function grid(lat: number, lon: number) {
  const size = 2 ** 30, radians = lat * Math.PI / 180
  return { gx: Math.round((lon + 180) / 360 * size), gy: Math.round((1 + Math.log(Math.tan(radians) + 1 / Math.cos(radians)) / Math.PI) / 2 * size) }
}

test('original US global then national priority, nullable payload, exact rerun and fresh extract converge', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'wind-ipc-'))
  try {
    const locations = [[50, -110], [40, -100], [60, 10], [40, -101], [50, -110]]
    const rows = locations.map(([lat, lon]) => grid(lat, lon))
    // Decode the same exact native point for source placement; no approximate geometry identity.
    const coordinates = rows.map(({ gx, gy }) => gridToLonLat(gx, gy))
    const registers = WIND_COUNTRIES.map(policy => ({ ...policy, observations: coordinates.flatMap((p, i) =>
      (policy.country === 'CA' && i === 0 || policy.country === 'US' && (i === 1 || i === 3) || policy.country === 'NO' && i === 2)
        ? [{ latitude: p.lat, longitude: p.lon + (i === 3 ? 0.004 : 0), hub: 100, power: 3500 }] : []) }))
    const input = makeTable({ osm_id: BigInt64Array.from([1n, 2n, 3n, 4n, 5n]),
      centroid_gx: Int32Array.from(rows, r => r.gx), centroid_gy: Int32Array.from(rows, r => r.gy),
      source_type: Uint8Array.from([10, 10, 10, 10, 0]), source_id: Uint16Array.from([320, 0, 330, 0, 300]),
      nace_4digit: Uint16Array.from([700, 0, 3511, 0, 2410]), suppressed: Uint8Array.from([1, 0, 0, 0, 0]),
      hub_height: vectorFromArray([70, 70, 70, 70, null], new Float32()),
      rated_power_kw: vectorFromArray([null, null, null, null, null], new Float32()),
    } as never) as unknown as Table
    const fields = input.schema.fields.map(f => new Field(f.name, f.type, f.nullable, new Map([['field', f.name]])))
    const schema = new Schema(fields, new Map([['grid', 'z30'], ['test', 'wind'], ['qm_batch_bboxes', '[[30,-120,70,20],[30,-120,70,20]]']]))
    const table = new Table(schema, [input.slice(0, 2), input.slice(2)].flatMap(t => t.batches.map(b => new RecordBatch(schema, b.data))))
    const bytes = tableToIPC(table, 'file'), path = resolve(work, 'industrial.arrow'), fresh = resolve(work, 'fresh.arrow')
    writeFileSync(path, bytes); writeFileSync(fresh, bytes)
    const match = windParameterMatcher(registers)
    assert.equal((await enrichWindSquare(path, match)).changed, 4)
    const output = tableFromIPC(readFileSync(path))
    assert.deepEqual([...output.getChild('hub_height')!], [100, 70, 70, 100, null])
    assert.deepEqual([...output.getChild('rated_power_kw')!], [3500, 3500, 3500, 3500, null])
    assert.deepEqual(output.schema, tableFromIPC(bytes).schema)
    assert.deepEqual(output.batches.map(b => b.numRows), [2, 3])
    for (const f of table.schema.fields) if (!['hub_height', 'rated_power_kw'].includes(f.name)) assert.deepEqual(output.getChild(f.name)!.toArray(), table.getChild(f.name)!.toArray())
    const after = readFileSync(path), stat = statSync(path, { bigint: true })
    assert.equal((await enrichWindSquare(path, match)).updated, false)
    assert.deepEqual(readFileSync(path), after); assert.equal(statSync(path, { bigint: true }).mtimeNs, stat.mtimeNs)
    await enrichWindSquare(fresh, match); assert.deepEqual(readFileSync(fresh), after)
    // Registry disappearance does not invalidate a retained turbine engineering measurement.
    await enrichWindSquare(path, windParameterMatcher(registers.map(r => ({ ...r, observations: [] }))))
    assert.deepEqual(readFileSync(path), after)
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('original workbook decommissioning and operational park-power join do not invent measurements', () => {
  const header = ['Møllenummer (GSRN)', '', '', 'Kapacitet (kW)', '', 'Navhøjde (m)']
  const row = ['57001', '', null, 3000, 90, 80, '', '', '', '', '', '', 500000, 6200000]
  assert.equal(parseWindWorkbook('DK', [header, row, [...row.slice(0, 2), 'retired', ...row.slice(3)]]).length, 1)
  const parks = [{ properties: { anleggsNr: 1, status: 'D', effekt_MW_idrift: 12, antallTurbiner: 4 } }]
  const turbines = ['D', 'retired'].map(status => ({ properties: { anleggsNr: 1, status }, geometry: { type: 'Point', coordinates: [10, 60] } }))
  assert.deepEqual(parseNorwegianWind(parks, turbines), [{ latitude: 60, longitude: 10, hub: 0, power: 3000 }])
  assert.throws(() => parseNorwegianWind({ error: 'missing' }, turbines))
  assert.throws(() => parseWindCsv('US', Buffer.from('ylat,xlong,t_cap,t_hh\n40,-100,Infinity,80\n')))
  assert.throws(() => parseWindCsv('DE', Buffer.from('lon,lat\n10,50\n')))
})

test('missing one required family cache fails before any native writer is called', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'wind-missing-'))
  try { await assert.rejects(loadWindRegisters(work), /ENOENT/) }
  finally { rmSync(work, { recursive: true, force: true }) }
})

test('the original 500 m match remains reachable beyond the old polar three-cell window', () => {
  const registers = WIND_COUNTRIES.map(p => ({ ...p, observations: p.country === 'NO'
    ? [{ latitude: 70, longitude: 10.021, hub: 0, power: 5000 }] : [] }))
  assert.deepEqual(windParameterMatcher(registers)(70, 10.009, 80, null), { hub: 80, power: 5000 })
  assert.deepEqual(windParameterMatcher(registers)(70, 10, 80, null), { hub: 80, power: null })
})


test('missing registry parameters never erase a positive native measurement or coerce an absent field to zero', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'wind-missing-fields-'))
  try {
    const rows = [[40, -100], [60, 18], [40, -101]].map(([lat, lon]) => grid(lat, lon))
    const coordinates = rows.map(({ gx, gy }) => gridToLonLat(gx, gy))
    const registers = WIND_COUNTRIES.map(policy => ({ ...policy, observations: coordinates.flatMap((p, i) =>
      policy.country === 'US' && i !== 1
        ? parseWindCsv('US', Buffer.from(`ylat,xlong,t_cap,t_hh\n${p.lat},${p.lon + 0.004},1500,\n`))
        : policy.country === 'SE' && i === 1 ? [{ latitude: p.lat, longitude: p.lon, hub: 80, power: 0 }] : []) }))
    const input = makeTable({ centroid_gx: Int32Array.from(rows, r => r.gx), centroid_gy: Int32Array.from(rows, r => r.gy),
      source_type: Uint8Array.from([10, 10, 10]), source_id: Uint16Array.from([320, 330, 0]),
      nace_4digit: Uint16Array.from([3511, 3511, 0]), suppressed: Uint8Array.from([1, 0, 0]),
      hub_height: vectorFromArray([80, null, null], new Float32()),
      rated_power_kw: vectorFromArray([null, 2300, null], new Float32()),
    } as never) as unknown as Table
    const schema = new Schema(input.schema.fields, new Map([['grid', 'z30'], ['test', 'missing-wind-fields']]))
    const table = new Table(schema, [input.slice(0, 1), input.slice(1)].flatMap(t => t.batches.map(b => new RecordBatch(schema, b.data))))
    const bytes = tableToIPC(table, 'file'), path = resolve(work, 'industrial.arrow'), fresh = resolve(work, 'fresh.arrow')
    writeFileSync(path, bytes); writeFileSync(fresh, bytes)
    const match = windParameterMatcher(registers)
    await enrichWindSquare(path, match)
    const output = tableFromIPC(readFileSync(path))
    assert.deepEqual([...output.getChild('hub_height')!], [80, 80, null])
    assert.deepEqual([...output.getChild('rated_power_kw')!], [1500, 2300, 1500])
    assert.deepEqual(output.schema, tableFromIPC(bytes).schema)
    assert.deepEqual(output.batches.map(b => b.numRows), [1, 2])
    for (const f of table.schema.fields) if (!['hub_height', 'rated_power_kw'].includes(f.name)) assert.deepEqual(output.getChild(f.name)!.toArray(), table.getChild(f.name)!.toArray())
    const after = readFileSync(path), stat = statSync(path, { bigint: true })
    assert.equal((await enrichWindSquare(path, match)).updated, false)
    assert.deepEqual(readFileSync(path), after); assert.equal(statSync(path, { bigint: true }).mtimeNs, stat.mtimeNs)
    await enrichWindSquare(fresh, match); assert.deepEqual(readFileSync(fresh), after)
  } finally { rmSync(work, { recursive: true, force: true }) }
})
