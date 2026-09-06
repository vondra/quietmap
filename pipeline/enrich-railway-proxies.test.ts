/** Dev1 behavior parity and z9 integration tests for the shared railway proxy family. */

import assert from 'node:assert/strict'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { after, test } from 'node:test'
import { tableFromIPC } from 'apache-arrow'
import {
  RAILWAY_PROXY_SPECS, enrichAllRailwayProxies, enrichRailwayProxyCountry,
  validateRailwayProxyCatalog,
} from './enrich-railway-proxies.js'
import { writeRailwaysFixture } from './lib/rail-test-fixture.js'
import type { RailwayRow } from './lib/railways-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'railway-proxies-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

function row(
  latitude: number,
  longitude: number,
  railType = 0,
  usage = 0,
  name = '',
): RailwayRow {
  return {
    startLat: latitude,
    startLon: longitude,
    endLat: latitude,
    endLon: longitude,
    midLat: latitude,
    midLon: longitude,
    railType,
    usage,
    service: 0,
    name,
    existingSourceId: 0,
    existingPassenger: 0,
    existingFreight: 0,
    existingDivisor: 1,
  }
}

type Golden = readonly [iso2: string, input: RailwayRow, passenger: number | null, freight: number | null]

function assertGoldens(goldens: readonly Golden[]): void {
  for (const [iso2, input, passenger, freight] of goldens) {
    const spec = RAILWAY_PROXY_SPECS.find(candidate => candidate.iso2 === iso2)!
    const actual = spec.classify?.(input) ?? null
    assert.deepEqual(actual, passenger === null ? null : { passenger, freight }, `${iso2} at ${input.midLat},${input.midLon}`)
  }
}

test('catalog is exactly the complete 17-country dev1 proxy/default family', () => {
  validateRailwayProxyCatalog()
  assert.deepEqual(RAILWAY_PROXY_SPECS.map(spec => spec.iso2), [
    'CD', 'DZ', 'EG', 'ET', 'IQ', 'IR', 'KE', 'KR', 'KZ', 'MA', 'NG', 'RU', 'SD', 'TR', 'TZ', 'UA', 'UZ',
  ])
  assert.equal(RAILWAY_PROXY_SPECS.find(spec => spec.iso2 === 'KR')!.classify, null)
})

test('African classifiers preserve dev1 family, corridor and fallback precedence', () => {
  assertGoldens([
    ['CD', row(-5.82, 13.45), 2, 4],
    ['CD', row(-11.66, 27.48), 1, 3],
    ['CD', row(0, 20, 0, 2), 0, 2],
    ['CD', row(0, 20, 3), 1, 1],
    ['CD', row(0, 20, 1), null, null],

    ['DZ', row(36.75, 3.06, 2), 150, 0],
    ['DZ', row(36.75, 3.06, 1), 80, 0],
    ['DZ', row(35.70, -0.63, 1), 60, 0],
    ['DZ', row(34, 5, 1), 40, 0],
    ['DZ', row(34.70, 7.95), 1, 18],
    ['DZ', row(36.00, 1.01), 15, 12],
    ['DZ', row(30, 0, 0, 2), 0, 6],
    ['DZ', row(30, 0, 0, 1), 1, 3],
    ['DZ', row(30, 0), 4, 6],

    ['EG', row(30, 31, 1), 250, 0],
    ['EG', row(30, 31, 2), 400, 0],
    ['EG', row(31.19, 29.90), 100, 30],
    ['EG', row(24.09, 32.90), 40, 15],
    ['EG', row(29.97, 32.55), 20, 15],
    ['EG', row(25, 30, 0, 2), 0, 6],
    ['EG', row(25, 30, 0, 1), 1, 4],
    ['EG', row(25, 30), 10, 8],

    ['ET', row(8, 40, 1), 150, 0],
    ['ET', row(8, 40, 4), 2, 0],
    ['ET', row(8, 40, 3), 1, 1],
    ['ET', row(8.92, 38.60, 0, 0, 'Awash–Weldiya Railway'), 1, 4],
    ['ET', row(11.72, 39.60), 1, 4],
    ['ET', row(9.59, 41.86), 4, 12],

    ['KE', row(-4.03, 39.55), 8, 20],
    ['KE', row(-0.95, 36.35), 4, 8],
    ['KE', row(-1.10, 36.80), 20, 4],
    ['KE', row(1, 38, 0, 2), 0, 4],
    ['KE', row(1, 38), 1, 4],

    ['MA', row(33.57, -7.59, 1), 300, 0],
    ['MA', row(34.02, -6.82, 2), 250, 0],
    ['MA', row(30, -5, 1), 200, 0],
    ['MA', row(32.88, -6.91), 1, 60],
    ['MA', row(34.21, -4.01), 40, 15],
    ['MA', row(35.77, -5.80), 40, 8],
    ['MA', row(31.63, -8.01), 30, 10],
    ['MA', row(25, -12, 0, 2), 0, 8],
    ['MA', row(25, -12, 0, 1), 4, 3],
    ['MA', row(25, -12), 8, 6],

    ['NG', row(6.50, 3.40, 1), 250, 0],
    ['NG', row(9.00, 7.40, 2), 60, 0],
    ['NG', row(6.50, 3.40), 250, 20],
    ['NG', row(7.45, 3.90), 16, 20],
    ['NG', row(10.52, 7.45), 8, 6],
    ['NG', row(7.60, 6.30), 2, 20],
    ['NG', row(8, 10, 3, 2), 0, 6],
    ['NG', row(8, 10, 3), 1, 4],

    ['SD', row(10, 30, 1), null, null],
    ['SD', row(10, 30, 3, 2), 0, 1],
    ['SD', row(19.62, 37.22, 3), 1, 3],
    ['SD', row(13.18, 30.22), 1, 2],
    ['SD', row(10, 30), 1, 1],

    ['TZ', row(-5, 35, 1), null, null],
    ['TZ', row(-5, 35, 0, 2), 0, 4],
    ['TZ', row(-6.173, 35.742), 10, 20],
    ['TZ', row(-8.91, 33.46), 3, 12],
    ['TZ', row(-2, 32), 2, 6],
  ])
})

test('Eurasian classifiers preserve dev1 family, corridor and fallback precedence', () => {
  assertGoldens([
    ['IQ', row(33.31, 44.36), 3, 4],
    ['IQ', row(35, 42), 1, 1],
    ['IQ', row(33, 44, 1), null, null],

    ['IR', row(35.70, 51.42, 1), 350, 0],
    ['IR', row(32, 53, 2), 80, 0],
    ['IR', row(30, 50, 0, 2), 0, 5],
    ['IR', row(36.30, 59.57), 15, 8],
    ['IR', row(29.59, 52.53), 8, 10],
    ['IR', row(38.08, 46.29), 5, 8],
    ['IR', row(27.18, 56.28), 3, 10],
    ['IR', row(30, 45), 3, 5],

    ['KR', row(37, 127), null, null],

    ['KZ', row(43.24, 76.92, 1), 80, 0],
    ['KZ', row(51.13, 71.43, 2), 50, 0],
    ['KZ', row(45, 70, 1), 60, 0],
    ['KZ', row(45, 70, 0, 2), 0, 6],
    ['KZ', row(52.29, 76.97), 2, 30],
    ['KZ', row(43.24, 76.92), 8, 20],
    ['KZ', row(45, 60, 0, 1), 1, 4],
    ['KZ', row(45, 60), 2, 10],

    ['RU', row(50, 50, 1), 200, 0],
    ['RU', row(50, 50, 0, 2), 0, 6],
    ['RU', row(59.94, 30.31), 35, 15],
    ['RU', row(53.69, 88.07), 12, 120],
    ['RU', row(52.29, 104.28), 30, 110],
    ['RU', row(55.15, 124.72), 6, 50],
    ['RU', row(44.7, 37.7), 8, 70],
    ['RU', row(53.20, 50.15), 60, 8],
    ['RU', row(50, 25, 0, 1), 4, 12],
    ['RU', row(50, 25), 12, 45],

    ['TR', row(41, 29, 4), 100, 0],
    ['TR', row(41, 29, 1), 350, 0],
    ['TR', row(41, 29, 2), 500, 0],
    ['TR', row(38, 30, 1), 250, 0],
    ['TR', row(38, 30, 2), 400, 0],
    ['TR', row(38, 30, 0, 2), 0, 5],
    ['TR', row(41.00, 29.06), 400, 0],
    ['TR', row(38.80, 26.97), 150, 4],
    ['TR', row(37.87, 32.48), 40, 0],
    ['TR', row(41.45, 31.80), 6, 20],
    ['TR', row(37.07, 37.38), 20, 12],
    ['TR', row(36, 27, 0, 1), 1, 3],
    ['TR', row(36, 27), 8, 6],

    ['UA', row(50.45, 30.52, 1), 180, 0],
    ['UA', row(45, 30, 1), 90, 0],
    ['UA', row(47.91, 33.39, 2), 150, 0],
    ['UA', row(45, 30, 2), 120, 0],
    ['UA', row(45, 30, 3), 2, 1],
    ['UA', row(45, 30, 0, 2), 0, 6],
    ['UA', row(45, 30, 0, 1), 5, 8],
    ['UA', row(45, 30), 20, 15],

    ['UZ', row(40, 60, 1), 200, 0],
    ['UZ', row(40, 60, 0, 2), 0, 6],
    ['UZ', row(41.31, 69.24), 8, 14],
    ['UZ', row(40.78, 72.34), 5, 11],
    ['UZ', row(43.06, 58.54), 5, 11],
    ['UZ', row(37.22, 67.28), 4, 12],
    ['UZ', row(44, 56, 0, 1), 2, 5],
    ['UZ', row(44, 56), 4, 8],
  ])
})

function z9Directory(prepared: string, latitude: number, longitude: number): string {
  const x = Math.floor((longitude + 180) / 360 * 512)
  const latitudeRadians = latitude * Math.PI / 180
  const y = Math.floor((1 - Math.asinh(Math.tan(latitudeRadians)) / Math.PI) / 2 * 512)
  return join(prepared, 'z9', String(x), String(y))
}

test('country runner consumes a disposable admin-baked z9 source and reruns byte-identically', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared-country')
  const square = z9Directory(prepared, -5.82, 13.45)
  mkdirSync(square, { recursive: true })
  const source = writeRailwaysFixture('proxy-loader-source.arrow', [
    { latitude: -5.82, longitude: 13.45, country: 'CD' },
    { latitude: -5.82, longitude: 13.45, country: 'CG' },
  ])
  const target = join(square, 'railways.arrow')
  copyFileSync(source, target)

  const first = await enrichRailwayProxyCountry(prepared, 'cd')
  assert.deepEqual({
    rows: first.rows,
    matched: first.matched,
    passenger: first.passengerTrainsPerDay,
    freight: first.freightTrainsPerDay,
    skippedForeign: first.skippedForeign,
    squaresUpdated: first.squaresUpdated,
  }, { rows: 2, matched: 1, passenger: 2, freight: 4, skippedForeign: 1, squaresUpdated: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [9181, 0])
  const before = readFileSync(target)
  const second = await enrichRailwayProxyCountry(prepared, 'CD')
  assert.deepEqual({ matched: second.matched, squaresUpdated: second.squaresUpdated }, { matched: 1, squaresUpdated: 0 })
  assert.deepEqual(readFileSync(target), before)
})

test('country runner preserves dev1 row-bbox eligibility inside a boundary square', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared-country-boundary')
  const square = z9Directory(prepared, -13.51, 13.45)
  mkdirSync(square, { recursive: true })
  const target = join(square, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('proxy-boundary-source.arrow', [
    { latitude: -13.51, longitude: 13.45, country: 'CD' },
  ]), target)
  const before = readFileSync(target)

  const result = await enrichRailwayProxyCountry(prepared, 'CD')
  assert.deepEqual({ rows: result.rows, matched: result.matched, squaresUpdated: result.squaresUpdated }, {
    rows: 1, matched: 0, squaresUpdated: 0,
  })
  assert.deepEqual(readFileSync(target), before)
})

test('KR no-source is an explicit zero-write result and family preflight prevents partial writes', async () => {
  const absent = join(TEST_DIRECTORY, 'absent')
  assert.equal((await enrichRailwayProxyCountry(absent, 'KR')).status, 'no-source')
  await assert.rejects(enrichRailwayProxyCountry(absent, 'CD'), /no CD railways.arrow source squares/)

  const prepared = join(TEST_DIRECTORY, 'partial-family')
  const square = z9Directory(prepared, -5.82, 13.45)
  mkdirSync(square, { recursive: true })
  const target = join(square, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('partial-family-source.arrow', [
    { latitude: -5.82, longitude: 13.45, country: 'CD' },
  ]), target)
  const before = readFileSync(target)
  await assert.rejects(enrichAllRailwayProxies(prepared), /no DZ railways.arrow source squares/)
  assert.deepEqual(readFileSync(target), before)
})
