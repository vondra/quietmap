/** CZ loader tests for strict refs, multipart geometry and baked ownership. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichCzechRoads, matchCensusSection, normalizeOsmRef, parseCensus,
  rsdRank, type CensusSection,
} from './enrich-roads-cz.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-cz-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

function censusFeature(options: {
  ref?: string
  category?: string
  paths?: number[][][]
  zero?: boolean
} = {}): unknown {
  const count = options.zero ? 0 : 100
  const secondaryCount = options.zero ? 0 : 10
  return {
    attributes: {
      PSILNICE: options.ref ?? '34', PKOD_R: options.category ?? '2',
      O: count, LN: secondaryCount, SN: secondaryCount, A: secondaryCount,
      TR: secondaryCount, TRP: secondaryCount, TN: secondaryCount,
      TNP: secondaryCount, SNP: secondaryCount, NSN: secondaryCount,
      AK: secondaryCount, M: secondaryCount,
    },
    geometry: { paths: options.paths ?? [[[14, 50], [14.01, 50.01]]] },
  }
}

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 50, startLon: 14, endLat: 50.001, endLon: 14.001,
    midLat: 50.0005, midLon: 14.0005,
    ref: 'I/34', name: null, osmId: 1, roadClass: 1, existingSourceId: 0,
    ...overrides,
  }
}

test('OSM ref parsing admits whole road tokens and rejects free text and E-roads', () => {
  assert.deepEqual(
    ['I/34', 'II/150', 'III/11620', 'D1', '150', '104a'].map(normalizeOsmRef),
    ['34', '150', '11620', 'D1', '150', '104'],
  )
  assert.deepEqual(
    ['Zelená 20', 'K Šeberovu 33', 'E50', '', 'ulice 5. května'].map(normalizeOsmRef),
    ['', '', '', '', ''],
  )
})

test('ŘSD category ranks preserve the class-compatibility gate', () => {
  assert.deepEqual(
    [['D1', '1'], ['D4', '5'], ['3', '2'], ['34', '6'], ['603', '3'], ['11628', '4']]
      .map(([ref, category]) => rsdRank(ref, category)),
    [0, 0, 1, 1, 3, 4],
  )
})

test('census parsing skips zero surveys per invocation and rejects invalid counts', () => {
  const feature = censusFeature({ zero: true })
  assert.equal(parseCensus([feature]).zeroSectionsSkipped, 1)
  assert.equal(parseCensus([feature]).zeroSectionsSkipped, 1)
  const invalid = censusFeature() as { attributes: Record<string, unknown> }
  invalid.attributes.O = -1
  assert.throws(() => parseCensus([invalid]), /invalid ŘSD count/)
})

test('matcher does not invent a bridge between disconnected ArcGIS paths', () => {
  const parsed = parseCensus([censusFeature({
    paths: [[[14, 50], [14.01, 50]], [[16, 50], [16.01, 50]]],
  })])
  assert.equal(matchCensusSection(road({ midLon: 15 }), parsed.byRef), null)
})

test('matcher filters incompatible road rank before choosing the nearest section', () => {
  const incompatible: CensusSection = {
    ref: '34', rank: 4, light: 1, medium: 1, heavy: 1, moto: 1,
    paths: [[[14.0005, 50.0005]]],
  }
  const compatible: CensusSection = {
    ref: '34', rank: 1, light: 2, medium: 2, heavy: 2, moto: 2,
    paths: [[[14.001, 50.0005]]],
  }
  assert.equal(matchCensusSection(road(), new Map([['34', [incompatible, compatible]]])), compatible)
})

test('z9 CZ pass writes domestic data and heals a matching foreign CZ stamp', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '276', '173')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('cz-loader.arrow', [1, 1], {
    refs: ['I/34', 'I/34'],
    countryCodes: [iso2Code('CZ'), iso2Code('RU')],
    sourceIds: [0, 20],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)
  const census = parseCensus([censusFeature()])

  const result = await enrichCzechRoads(prepared, census.byRef)
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 1, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [20, 0])
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('aadt_light')!.get(index)), [110, 0])
  assert.equal(table.schema.metadata.get('roads_contract'), 'country_baked_v1')
})
