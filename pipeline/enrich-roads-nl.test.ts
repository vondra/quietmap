/** NL Amsterdam tests for exact AADT classes, proximity and z9 provenance. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import {
  copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichNetherlandsRoads, indexAmsterdamTraffic, matchAmsterdamTrafficRecord,
} from './enrich-roads-nl.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  loadAmsterdamTrafficCensus, parseAmsterdamTrafficSource,
  type AmsterdamTrafficRecord,
} from './lib/roads-nl-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import {
  SOURCE_ID_DE_BAST_AUTOBAHN, SOURCE_ID_EU_CITY_TRAFFIC,
} from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-nl-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

interface FeatureOptions {
  properties?: unknown
  geometry?: unknown
  type?: string
}

function feature(options: FeatureOptions = {}): Record<string, unknown> {
  return {
    type: options.type ?? 'Feature',
    properties: options.properties ?? { AADT: 1025, raw_name: 'Vrije Geer' },
    geometry: options.geometry ?? {
      type: 'LineString',
      coordinates: [[4.79, 52.34], [4.7994853, 52.345035], [4.80, 52.35]],
    },
  }
}

const source = (features: readonly unknown[]): string =>
  JSON.stringify({ type: 'FeatureCollection', features })

function record(overrides: Partial<AmsterdamTrafficRecord> = {}): AmsterdamTrafficRecord {
  return {
    sourceRow: 0, latitude: 50, longitude: 14, aadtTotal: 1025,
    aadt_light: 1004, aadt_medium: 21, aadt_heavy: 0, aadt_moto: 0,
    ...overrides,
  }
}

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 50, startLon: 14, endLat: 50.001, endLon: 14.001,
    midLat: 50, midLon: 14, ref: null, name: null,
    osmId: 1, roadClass: 5, existingSourceId: 0, ...overrides,
  }
}

test('Amsterdam parser uses the middle vertex and exact published AADT total', () => {
  const parsed = parseAmsterdamTrafficSource(source([
    feature(),
    feature({ properties: { AADT: 1000 }, geometry: {
      type: 'LineString', coordinates: [[4.8, 52.3], [4.9, 52.4]],
    } }),
  ]))
  assert.deepEqual(
    { sourceRows: parsed.sourceRows, accepted: parsed.accepted },
    { sourceRows: 2, accepted: 2 },
  )
  assert.deepEqual(
    parsed.records.map(value => ({
      coordinate: [value.longitude, value.latitude],
      total: value.aadtTotal,
      classes: [value.aadt_light, value.aadt_medium, value.aadt_heavy, value.aadt_moto],
    })),
    [
      { coordinate: [4.7994853, 52.345035], total: 1025, classes: [1004, 21, 0, 0] },
      { coordinate: [4.9, 52.4], total: 1000, classes: [980, 20, 0, 0] },
    ],
  )
  for (const value of parsed.records) {
    assert.equal(
      value.aadt_light + value.aadt_medium + value.aadt_heavy + value.aadt_moto,
      value.aadtTotal,
    )
  }
})

test('Amsterdam parser accounts for every malformed source row', () => {
  const parsed = parseAmsterdamTrafficSource(source([
    feature({ type: 'Road' }),
    feature({ properties: { AADT: '1025' } }),
    feature({ geometry: { type: 'LineString', coordinates: [[4.8, 52.3]] } }),
  ]))
  assert.deepEqual(parsed, {
    records: [], sourceRows: 3, accepted: 0,
    invalidMetadataSkipped: 1, invalidTrafficSkipped: 1, invalidGeometrySkipped: 1,
  })
  assert.throws(
    () => parseAmsterdamTrafficSource(JSON.stringify({ type: 'Feature', features: [] })),
    /must be a GeoJSON FeatureCollection/,
  )
})

test('Amsterdam loader rejects bytes other than the pinned 2025 source', async () => {
  const enrichmentDirectory = join(TEST_DIRECTORY, 'noncanonical-source')
  const directory = join(enrichmentDirectory, 'global', 'eu-city-traffic')
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, 'Amsterdam_AADT_2025.geojson'), source([feature()]))
  await assert.rejects(
    loadAmsterdamTrafficCensus({
      preparedDirectory: join(TEST_DIRECTORY, 'unused'),
      enrichmentDirectory,
      enrichOnly: true,
      forceDownload: false,
    }),
    /does not match immutable 2025 census/,
  )
})

test('Amsterdam matcher chooses the nearest record under the strict 50 metre cap', () => {
  const farther = record({ sourceRow: 1, latitude: 50, longitude: 14.0004 })
  const nearest = record({ sourceRow: 2, latitude: 50, longitude: 14.0001 })
  const grid = indexAmsterdamTraffic([farther, nearest])
  assert.equal(matchAmsterdamTrafficRecord(road(), grid), nearest)

  const neighboringCell = record({ sourceRow: 3, latitude: 50, longitude: 14.0015 })
  const outsideCap = record({ sourceRow: 4, latitude: 50, longitude: 14.0018 })
  assert.equal(
    matchAmsterdamTrafficRecord(
      road({ midLon: 14.00099 }), indexAmsterdamTraffic([outsideCap, neighboringCell]),
    ),
    neighboringCell,
  )

  const fortyNineMetres = record({ latitude: 50 + 49 / 110_540, longitude: 14 })
  assert.equal(matchAmsterdamTrafficRecord(road(), indexAmsterdamTraffic([fortyNineMetres])), fortyNineMetres)
  const fiftyMetres = record({ latitude: 50 + 50 / 110_540, longitude: 14 })
  assert.equal(matchAmsterdamTrafficRecord(road(), indexAmsterdamTraffic([fiftyMetres])), null)
})

test('z9 NL pass writes exact classes and preserves higher-priority provenance', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '275', '173')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('nl-loader.arrow', [5, 5], {
    countryCodes: [iso2Code('NL'), iso2Code('NL')],
    sourceIds: [0, SOURCE_ID_DE_BAST_AUTOBAHN],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const measurements = [
    record({ latitude: 50.00025, longitude: 14.00025 }),
    record({ sourceRow: 1, latitude: 50.00125, longitude: 14.00125, aadtTotal: 2000,
      aadt_light: 1960, aadt_medium: 40 }),
  ]

  const result = await enrichNetherlandsRoads(prepared, measurements)
  assert.deepEqual(result, { rows: 2, matched: 1, squares: 1, squaresUpdated: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual(
    [...Array(2)].map((_, index) => table.getChild('country_iso')!.get(index)),
    [iso2Code('NL'), iso2Code('NL')],
  )
  assert.deepEqual(
    [...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)),
    [SOURCE_ID_EU_CITY_TRAFFIC, SOURCE_ID_DE_BAST_AUTOBAHN],
  )
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
      .map(name => [...Array(2)].map((_, index) => table.getChild(name)!.get(index))),
    [[1004, 1001], [21, 2001], [0, 3001], [0, 41]],
  )

  const bytes = readFileSync(target)
  const repeated = await enrichNetherlandsRoads(prepared, measurements)
  assert.deepEqual(repeated, { rows: 2, matched: 1, squares: 1, squaresUpdated: 0 })
  assert.deepEqual(readFileSync(target), bytes)
})
