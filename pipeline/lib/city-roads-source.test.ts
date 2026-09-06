/** Municipal source arithmetic and admission regressions, independent of retained caches. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parsePrahaRows, parseBrno, parseWien } from './city-roads-source.js'
import { municipalityFromGeoJson } from './city-polygon.js'

const rectangle = (west: number, south: number, east: number, north: number) =>
  [[west, south], [east, south], [east, north], [west, north], [west, south]]

export function testMunicipality() {
  return municipalityFromGeoJson(JSON.stringify({ type: 'FeatureCollection', features: [{
    properties: { shapeName: 'Test City' }, geometry: { type: 'Polygon', coordinates: [
      rectangle(14.42, 50.07, 14.44, 50.09), rectangle(14.4309, 50.0809, 14.4317, 50.0817),
    ] },
  }] }), 'Test City')
}

test('TSK weights whole-street components by section length and preserves the published working-day split', () => {
  const row = (name: string, length: number, cars: number, slow: number, bus: number) => [1, 2, name, '', '', length, cars, slow, cars + slow, bus]
  const rows = Array.from({ length: 500 }, (_, i) => row(`Street${i}`, 100, 100, 10, 1))
  rows.push(row('LEGEROVA', 100, 100, 20, 4), row('LEGEROVA', 300, 300, 40, 8), row('BARRAND.MOST', 50, 10, 2, 1))
  const source = parsePrahaRows(rows)
  assert.deepEqual(source.records.find(r => r.street === 'LEGEROVA'), { street: 'LEGEROVA', light: 250, medium: 7, heavy: 35, moto: 0 })
  assert.ok(source.records.some(r => r.street === 'Barrandovský most'))
  assert.equal(source.sections, 503)
  assert.throws(() => parsePrahaRows(rows.slice(0, 499)), /only499|only 499/)
  assert.throws(() => parsePrahaRows([...rows, row('broken', 1, NaN, 1, 1)]), /invalid Praha cars/)
})

test('BKOM selects a populated non-pandemic edition, preserves truck percent and keeps disconnected parts separate', () => {
  const features = Array.from({ length: 500 }, (_, id) => ({ properties: {
    id, car_2021: 100, truc_2021: 30, car_2023: 10, truc_2023: 25, car_2024: id < 449 ? 100 : null,
  }, geometry: { type: 'LineString', coordinates: [[16.6, 49.2], [16.601, 49.2]] } }))
  const source = parseBrno(JSON.stringify({ type: 'FeatureCollection', features }))
  assert.equal(source.year, 2023)
  assert.deepEqual(source.records[0], { street: 'BKOM section 0', light: 7500, medium: 0, heavy: 2500, moto: 0, line: features[0].geometry.coordinates })
  const multipart = structuredClone(features) as Array<{ properties: Record<string, unknown>; geometry: { type: string; coordinates: unknown } }>
  multipart[0].geometry = { type: 'MultiLineString', coordinates: [[[16.6, 49.2], [16.601, 49.2]], [[16.7, 49.2], [16.701, 49.2]]] }
  assert.equal(parseBrno(JSON.stringify({ type: 'FeatureCollection', features: multipart })).records.length, 501)
  assert.throws(() => parseBrno(JSON.stringify({ type: 'FeatureCollection', features, exceededTransferLimit: true })), /complete/)
  features[0].properties.truc_2023 = 101
  assert.throws(() => parseBrno(JSON.stringify({ type: 'FeatureCollection', features })), /bounds/)
})

test('Wien uses days-weighted total-minus-trucks, rejects invented clamps and requires enough observed months', () => {
  const locations = JSON.stringify({ type: 'FeatureCollection', features: Array.from({ length: 60 }, (_, id) => ({
    properties: { ZST_ID: id }, geometry: { type: 'Point', coordinates: [16.37, 48.2] },
  })) })
  const months = ['JAN.', 'FEB.', 'MÄRZ', 'APRIL', 'MAI', 'JUNI', 'JULI', 'AUG.', 'SEP.', 'OKT', 'NOV', 'DEZ.']
  const rows = ['JAHR;MONAT;ZNR;ZNAME;RINAME;FZTYP;DTVMS']
  for (let id = 0; id < 50; id++) for (const [month, name] of months.entries()) {
    rows.push(`2025;${name};${id};Street${id};Gesamt;Kfz;${month === 1 ? 200 : 100}`)
    rows.push(`2025;${name};${id};Street${id};Gesamt;LkwÄ;10`)
    rows.push(`2026;${name};${id};Street${id};Gesamt;Kfz;${month < 9 ? 500 : 0}`)
  }
  const input = Buffer.from(rows.join('\n'), 'latin1'), source = parseWien(input, locations)
  assert.equal(source.year, 2025)
  assert.equal(source.records.length, 50)
  assert.equal(source.records[0].light, Math.round((100 * 337 + 200 * 28) / 365 - 10))
  assert.equal(source.records[0].heavy, 10)
  assert.equal(source.invalidValuesSkipped, 150)
  assert.throws(() => parseWien(Buffer.from(rows.join('\n').replaceAll(';LkwÄ;10', ';LkwÄ;1000'), 'latin1'), locations), /exceeds all vehicles/)
  assert.throws(() => parseWien(Buffer.from('wrong;header\n1;2'), locations), /columns/)
})

test('municipal ownership uses the complete polygon and holes, not its rectangular enumeration envelope', () => {
  const gate = testMunicipality()
  assert.equal(gate.contains(50.08025, 14.43025), true)
  assert.equal(gate.contains(50.08125, 14.43125), false)
  assert.equal(gate.contains(50.1, 14.43), false)
  assert.throws(() => municipalityFromGeoJson('{"type":"FeatureCollection","features":[]}', 'missing'), /exactly one/)
})
