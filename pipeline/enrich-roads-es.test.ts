/** ES MITMA tests for source truth, multipart matching and z9 provenance. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import {
  copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichSpanishRoads, indexMitmaRoadCensus, matchMitmaRoadSection,
} from './enrich-roads-es.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  loadMitmaRoadCensus, parseMitmaRoadSource,
  type GeoJsonCoordinate, type MitmaRoadSection,
} from './lib/roads-es-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_ES_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-es-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

interface FeatureOptions {
  id?: string
  properties?: Record<string, unknown>
  geometry?: unknown
}

function feature(options: FeatureOptions = {}): Record<string, unknown> {
  return {
    type: 'Feature',
    id: options.id ?? 'tramos_Final.1',
    properties: {
      provincia: 'Madrid', via: 'A-5', pkinicio_t: 1, pkfin_t: 2,
      longitud: 1, clase: 'Autopista libre y autovía',
      imdtot: '1000', imdlig: '800', imdpes: '200',
      ...options.properties,
    },
    geometry: options.geometry ?? {
      type: 'MultiLineString',
      coordinates: [[[-4, 40], [-3, 40]]],
    },
  }
}

function wrappedSource(features: readonly Record<string, unknown>[], declared = features.length): string {
  return `var tramos_data = ${JSON.stringify({ type: 'FeatureCollection', totalFeatures: declared, features })};`
}

function section(overrides: Partial<MitmaRoadSection> = {}): MitmaRoadSection {
  return {
    featureId: 'tramos_Final.1', province: 'Madrid', via: 'A-1', ref: 'A1',
    lines: [[[-4, 40], [-3.99, 40.01]]],
    pkStart: 1, pkEnd: 2, lengthKm: 1, roadType: 'Autovía',
    imdTotal: 1000, imdLight: 800, imdHeavy: 200,
    aadt_light: 790, aadt_medium: 10, aadt_heavy: 190, aadt_moto: 10,
    ...overrides,
  }
}

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 40, startLon: -3.5, endLat: 40.001, endLon: -3.499,
    midLat: 40, midLon: -3.5, ref: 'A-5', name: null,
    osmId: 1, roadClass: 0, existingSourceId: 0, ...overrides,
  }
}

test('MITMA parser preserves published totals, derived classes and separate source lines', () => {
  const lines: GeoJsonCoordinate[][] = [
    [[-4, 40], [-3.9, 40]],
    [[-3, 40], [-2.9, 40]],
  ]
  const parsed = parseMitmaRoadSource(wrappedSource([feature({
    id: 'tramos_Final.7',
    properties: { provincia: 'Toledo', via: 'A-14-R2', pkinicio_t: 4.5, pkfin_t: 6.5, longitud: 2 },
    geometry: { type: 'MultiLineString', coordinates: lines },
  })]))
  assert.deepEqual(
    { rows: parsed.sourceRows, accepted: parsed.accepted, multipart: parsed.multipartSections },
    { rows: 1, accepted: 1, multipart: 1 },
  )
  const result = parsed.sections[0]
  assert.deepEqual(
    {
      featureId: result.featureId, province: result.province, via: result.via, ref: result.ref,
      pkStart: result.pkStart, pkEnd: result.pkEnd, lengthKm: result.lengthKm,
      imdTotal: result.imdTotal, imdLight: result.imdLight, imdHeavy: result.imdHeavy,
      classes: [result.aadt_light, result.aadt_medium, result.aadt_heavy, result.aadt_moto],
    },
    {
      featureId: 'tramos_Final.7', province: 'Toledo', via: 'A-14-R2', ref: 'A14R2',
      pkStart: 4.5, pkEnd: 6.5, lengthKm: 2,
      imdTotal: 1000, imdLight: 800, imdHeavy: 200,
      classes: [790, 10, 190, 10],
    },
  )
  assert.deepEqual(result.lines, lines)
  assert.equal(result.aadt_light + result.aadt_medium + result.aadt_heavy + result.aadt_moto, result.imdTotal)
})

test('MITMA parser rejects unavailable, inconsistent, malformed and out-of-scope measurements', () => {
  const parsed = parseMitmaRoadSource(wrappedSource([
    feature({ id: 'missing', properties: { imdtot: 'NULL', imdlig: 'NULL', imdpes: 'NULL' } }),
    feature({ id: 'mismatch', properties: { imdlig: '900', imdpes: '200' } }),
    feature({ id: 'scientific', properties: { imdtot: '1e3', imdlig: '800', imdpes: '200' } }),
    feature({ id: 'negative-derived-light', properties: { imdtot: '100', imdlig: '0', imdpes: '100' } }),
    feature({ id: 'metadata', properties: { via: '' } }),
    feature({ id: 'geometry', geometry: { type: 'MultiLineString', coordinates: [[[-4, 40]]] } }),
    feature({ id: 'outside', geometry: { type: 'MultiLineString', coordinates: [[[-4, 40], [-20, 30]]] } }),
  ]))
  assert.deepEqual(
    {
      accepted: parsed.accepted,
      missingTraffic: parsed.missingTrafficSkipped,
      invalidTraffic: parsed.invalidTrafficSkipped,
      invalidMetadata: parsed.invalidMetadataSkipped,
      invalidGeometry: parsed.invalidGeometrySkipped,
      outsideCoverage: parsed.outsideCoverageSkipped,
    },
    {
      accepted: 0, missingTraffic: 1, invalidTraffic: 3,
      invalidMetadata: 1, invalidGeometry: 1, outsideCoverage: 1,
    },
  )
  assert.throws(
    () => parseMitmaRoadSource(wrappedSource([feature()], 2)),
    /totalFeatures does not match 1 features/,
  )
  assert.throws(
    () => parseMitmaRoadSource(wrappedSource([feature()]).replace('var tramos_data', 'const tramos_data')),
    /must be a 'var tramos_data = \{\.\.\.\}' assignment/,
  )
})

test('MITMA loader rejects noncanonical bytes before enrichment', async () => {
  const enrichmentDirectory = join(TEST_DIRECTORY, 'noncanonical-source')
  mkdirSync(join(enrichmentDirectory, 'es'), { recursive: true })
  writeFileSync(join(enrichmentDirectory, 'es', 'mitma-tramos-2022.js'), wrappedSource([feature()]))
  await assert.rejects(
    loadMitmaRoadCensus({
      preparedDirectory: join(TEST_DIRECTORY, 'unused'),
      enrichmentDirectory,
      enrichOnly: true,
      forceDownload: false,
    }),
    /does not match immutable 2022 census/,
  )
})

test('MITMA matcher normalizes first OSM ref and enforces the strict 30 kilometre cap', () => {
  const measured = section({ ref: 'A5', via: 'A-5', lines: [[[-4, 40], [-3, 40]]] })
  const census = indexMitmaRoadCensus([measured])
  assert.equal(matchMitmaRoadSection(road({ ref: ' A-5 ; N-I ' }), census), measured)
  assert.equal(matchMitmaRoadSection(road({ ref: 'A-5R' }), census), null)
  assert.equal(matchMitmaRoadSection(road({ midLat: 40.269 }), census), measured)
  assert.equal(matchMitmaRoadSection(road({ midLat: 40.273 }), census), null)
})

test('MITMA matcher never treats the gap between distinct source lines as a road', () => {
  const measured = section({
    ref: 'A5', via: 'A-5',
    lines: [[[0, 0], [0, 0.1]], [[1, 0.1], [1, 0]]],
  })
  const census = indexMitmaRoadCensus([measured])
  assert.equal(matchMitmaRoadSection(road({ midLat: 0.1, midLon: 0.5 }), census), null)
})

test('z9 ES pass writes exact classes, retracts stale claims and respects baked ownership', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '250', '193')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('es-loader.arrow', [0, 0, 0], {
    origin: [-4, 40], refs: ['A-1', 'A-1', 'A-2'],
    countryCodes: [iso2Code('ES'), iso2Code('PT'), iso2Code('ES')],
    sourceIds: [0, SOURCE_ID_ES_NATIONAL_ROADS, SOURCE_ID_ES_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)

  const result = await enrichSpanishRoads(prepared, [section()])
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual(
    [...Array(3)].map((_, index) => table.getChild('source_id')!.get(index)),
    [SOURCE_ID_ES_NATIONAL_ROADS, 0, 0],
  )
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [790, 10, 190, 10],
  )
  assert.deepEqual(
    [1, 2].map(index =>
      ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
        .map(name => table.getChild(name)!.get(index))),
    [[0, 0, 0, 0], [0, 0, 0, 0]],
  )
})
