/** Swedish NVDB parser, matcher and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichSwedishRoads, indexSwedishNvdb, matchSwedishNvdb } from './enrich-roads-se.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  decodeGpkgLineString,
  loadSwedishNvdbSource,
  parseSwedishNvdbSource,
  type SwedishNvdbRecord,
} from './lib/roads-se-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_SE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-se-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

/** Minimal GeoPackage Z LineString blob, like the extract: GP magic, XY envelope, little-endian WKB. */
const gpkgLine = (points: readonly (readonly [number, number])[]): Uint8Array => {
  const header = new ArrayBuffer(8 + 32)
  const head = new DataView(header)
  head.setUint16(0, 0x4750, false)
  head.setUint8(2, 0)
  head.setUint8(3, 0x02)
  head.setInt32(4, 3006, true)
  const eastings = points.map(point => point[0]),
    northings = points.map(point => point[1])
  const envelope = [Math.min(...eastings), Math.max(...eastings), Math.min(...northings), Math.max(...northings)]
  envelope.forEach((value, index) => head.setFloat64(8 + index * 8, value, true))
  const body = new ArrayBuffer(9 + points.length * 24)
  const wkb = new DataView(body)
  wkb.setUint8(0, 1)
  wkb.setUint32(1, 1002, true)
  wkb.setUint32(5, points.length, true)
  points.forEach(([east, north], index) => {
    wkb.setFloat64(9 + index * 24, east, true)
    wkb.setFloat64(9 + index * 24 + 8, north, true)
    wkb.setFloat64(9 + index * 24 + 16, 10, true)
  })
  return new Uint8Array([...new Uint8Array(header), ...new Uint8Array(body)])
}

// Stockholm SWEREF: E 674850, N 6580150 is 18.073 E, 59.3239 N. The line runs 45°
// through row 0 and within 38 m of row 1, while row 2 sits 76 m off it.
const STOCKHOLM_LINE: (readonly [number, number])[] = [
  [674829, 6580129],
  [674950, 6580250],
]

const record = (overrides: Partial<SwedishNvdbRecord> = {}): SwedishNvdbRecord => ({
  id: 761,
  role: 'Normal',
  method: 'Stickprovsmätning',
  total: 1477,
  lightPeriods: [1131, 193, 86],
  mediumPeriods: [43, 7, 3],
  heavyPeriods: [12, 1, 1],
  geometry: gpkgLine(STOCKHOLM_LINE),
  ...overrides,
})

const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 59.323617,
  startLon: 18.07275,
  endLat: 59.324117,
  endLon: 18.07325,
  midLat: 59.323867,
  midLon: 18.073,
  ref: 'E4',
  name: null,
  osmId: 1,
  roadClass: 0,
  existingSourceId: 0,
  ...overrides,
})

test('Swedish blob decoder reads the line and rejects anything else', () => {
  const decoded = decodeGpkgLineString(gpkgLine(STOCKHOLM_LINE))
  assert.equal(decoded.srsId, 3006)
  assert.deepEqual(decoded.line, [
    [674829, 6580129],
    [674950, 6580250],
  ])
  const blob = gpkgLine(STOCKHOLM_LINE)
  assert.throws(() => decodeGpkgLineString(blob.slice(0, 4)), /not a GeoPackage blob/)
  const point = new Uint8Array(blob)
  new DataView(point.buffer).setUint32(8 + 32 + 1, 1, true)
  assert.throws(() => decodeGpkgLineString(point), /not a line/)
})

test('Swedish parser splits classes and tells two-way links from sibling carriageways', () => {
  const parsed = parseSwedishNvdbSource([
    record(),
    record({
      id: 762,
      role: 'Syskon fram',
      total: 6055,
      lightPeriods: [5200, 300, 180],
      mediumPeriods: [200, 30, 20],
      heavyPeriods: [100, 20, 5],
    }),
    record({
      id: 763,
      role: 'Syskon bak',
      total: 6055,
      lightPeriods: [5200, 300, 180],
      mediumPeriods: [200, 30, 20],
      heavyPeriods: [100, 20, 5],
    }),
  ])
  assert.deepEqual(
    { observations: parsed.observations.length, twoWay: parsed.twoWayObservations, directional: parsed.directionalObservations },
    { observations: 3, twoWay: 1, directional: 2 },
  )
  const [twoWay] = parsed.observations
  assert.equal(twoWay.countBasis, 'both-directions')
  assert.equal(twoWay.observationId, 'nvdb2026:761')
  // Moto is 1 % of the 1,477 total, taken from the light class; the total stays exact.
  assert.deepEqual(
    { light: twoWay.light, medium: twoWay.medium, heavy: twoWay.heavy, moto: twoWay.moto },
    { light: 1395, medium: 53, heavy: 14, moto: 15 },
  )
  assert.ok(Math.abs(twoWay.line[0][0] - 18.073) < 0.01)
  assert.ok(Math.abs(twoWay.line[0][1] - 59.324) < 0.01)
  assert.deepEqual(
    parsed.observations.map(observation => observation.countBasis),
    ['both-directions', 'directional', 'directional'],
  )
})

test('Swedish parser keeps only measured link parts with consistent splits', () => {
  const parsed = parseSwedishNvdbSource([
    record({ id: 1, method: 'Bedömt flöde utan stödmätning' }),
    record({ id: 2, method: null }),
    record({ id: 3, lightPeriods: [1131, null, 86] }),
    record({ id: 4, total: 0, lightPeriods: [0, 0, 0], mediumPeriods: [0, 0, 0], heavyPeriods: [0, 0, 0] }),
    record({ id: 5, total: 1477, lightPeriods: [100, 193, 86] }),
    record({ id: 6, geometry: gpkgLine([[0, 0], [1, 1]]) }),
    record({ id: 7 }),
  ])
  assert.deepEqual(
    {
      assessed: parsed.assessedSkipped,
      incomplete: parsed.incompleteSplitsSkipped,
      zero: parsed.zeroTrafficSkipped,
      inconsistent: parsed.inconsistentClassesSkipped,
      geometry: parsed.invalidGeometrySkipped,
      observations: parsed.observations.length,
    },
    { assessed: 2, incomplete: 1, zero: 1, inconsistent: 1, geometry: 1, observations: 1 },
  )
  assert.throws(() => parseSwedishNvdbSource([record({ id: 1 }), record({ id: 1 })]), /invalid section identity/)
})

test('Swedish pinned loader rejects bytes outside the admitted release source', () => {
  const enrichmentDirectory = join(DIRECTORY, 'source')
  mkdirSync(join(enrichmentDirectory, 'se'), { recursive: true })
  writeFileSync(join(enrichmentDirectory, 'se', 'nvdb-trafik-2026.gpkg'), 'not a geopackage')
  assert.throws(
    () =>
      loadSwedishNvdbSource({
        preparedDirectory: join(DIRECTORY, 'unused'),
        enrichmentDirectory,
        enrichOnly: true,
        forceDownload: false,
      }),
    /does not match the admitted release source/,
  )
})

test('Swedish matcher requires the line, the heading and a through class', () => {
  const parsed = parseSwedishNvdbSource([record()])
  const index = indexSwedishNvdb(parsed.observations)
  const [observation] = parsed.observations
  assert.equal(matchSwedishNvdb(road(), index), observation)
  // A cross street over the link part takes nothing.
  assert.equal(
    matchSwedishNvdb(
      road({
        startLat: 59.323617,
        startLon: 18.07325,
        endLat: 59.324117,
        endLon: 18.07275,
      }),
      index,
    ),
    null,
  )
  // A slip road beside the link part keeps its default: no ramp flag, no stamp.
  assert.equal(matchSwedishNvdb(road({ roadClass: 10 }), index), null)
  // A parallel road beyond the 50 m radius takes nothing.
  assert.equal(matchSwedishNvdb(road({ midLat: 59.325867, midLon: 18.075 }), index), null)
})

test('z9 Swedish pass writes classes, retracts stale claims and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '280', '135')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('se-loader.arrow', [0, 0, 0], {
    origin: [18.07275, 59.323617],
    refs: ['E4', 'E4', 'E18'],
    countryCodes: [iso2Code('SE'), iso2Code('FI'), iso2Code('SE')],
    sourceIds: [0, SOURCE_ID_SE_NATIONAL_ROADS, SOURCE_ID_SE_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const parsed = parseSwedishNvdbSource([record()])
  const result = await enrichSwedishRoads(prepared, parsed.observations)
  assert.deepEqual(
    {
      matched: result.matched,
      retracted: result.retracted,
      skippedForeign: result.skippedForeign,
    },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_SE_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [1395, 53, 14, 15],
  )
  assert.deepEqual([...table.getChild('traffic_estimated')!], [8, 15, 15])
})
