/** DE BASt tests for source truth, matching and z9 provenance gates. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import {
  copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync,
} from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichGermanRoads, indexBastCensus, matchBastSection,
} from './enrich-roads-de.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  loadBastCensus, parseBastRows, type BastCensusSection,
} from './lib/roads-de-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-de-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

function section(overrides: Partial<BastCensusSection> = {}): BastCensusSection {
  return {
    road: 'A 1', ref: 'A1', tkzst: '1001', lat: 50, lon: 10, dtv: 100,
    aadt_light: 80, aadt_medium: 5, aadt_heavy: 14, aadt_moto: 1,
    ...overrides,
  }
}

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 50, startLon: 10, endLat: 50.001, endLon: 10.001,
    midLat: 50.0005, midLon: 10.0005, ref: 'A1', name: null,
    osmId: 1, roadClass: 0, existingSourceId: 0, ...overrides,
  }
}

test('BASt row parser preserves suffix refs, published total and all four source classes', () => {
  const headers = [
    'Str', 'TKZST', 'DTV', 'DTVLVm', 'DTVBus', 'DTVLoA', 'DTVLZ', 'DTVKrad',
    'X_Koordinate', 'Y_Koordinate',
  ]
  const parsed = parseBastRows([
    headers,
    ['A 1z', 1234, 100, 80, 3, 2, 14, 1, 634719.603, 5586391.482],
    ['B 4', 1235, 100, 80, 3, 2, 14, 1, 634719.603, 5586391.482],
    ['A 2', 1236, 100, 0, 0, 0, 0, 0, 634719.603, 5586391.482],
  ], 'A')
  assert.equal(parsed.sourceRows, 3)
  assert.equal(parsed.invalidRowsSkipped, 1)
  assert.equal(parsed.sections.length, 2)
  assert.equal(parsed.sections[1].ref, 'A2')
  assert.equal(parsed.sections[1].aadt_light + parsed.sections[1].aadt_medium +
    parsed.sections[1].aadt_heavy + parsed.sections[1].aadt_moto, 0)
  assert.deepEqual(
    Object.fromEntries(Object.entries(parsed.sections[0]).filter(([key]) =>
      ['road', 'ref', 'tkzst', 'dtv', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].includes(key))),
    {
      road: 'A 1z', ref: 'A1z', tkzst: '1234', dtv: 100,
      aadt_light: 80, aadt_medium: 5, aadt_heavy: 14, aadt_moto: 1,
    },
  )
})

test('BASt cache rejects unstamped zero and inconsistent class splits without inventing values', async () => {
  const enrichment = join(TEST_DIRECTORY, 'cache')
  mkdirSync(join(enrichment, 'de'), { recursive: true })
  writeFileSync(join(enrichment, 'de', 'svz-census.json'), JSON.stringify([
    section(),
    section({ ref: 'B4', road: 'B 4', tkzst: '1002', aadt_light: 0, aadt_medium: 0, aadt_heavy: 0, aadt_moto: 0 }),
    section({ ref: 'A2', road: 'A 2', tkzst: '1003', aadt_light: 70 }),
  ]))
  const census = await loadBastCensus({
    preparedDirectory: TEST_DIRECTORY,
    enrichmentDirectory: enrichment,
    enrichOnly: true,
    forceDownload: false,
  })
  assert.deepEqual(
    {
      sourceRows: census.sourceRows,
      accepted: census.sections.length,
      zero: census.zeroClassSplitsSkipped,
      inconsistent: census.inconsistentClassTotalsSkipped,
    },
    { sourceRows: 3, accepted: 1, zero: 1, inconsistent: 1 },
  )
  writeFileSync(join(enrichment, 'de', 'svz-census.json'), '[]')
  await assert.rejects(loadBastCensus({
    preparedDirectory: TEST_DIRECTORY, enrichmentDirectory: enrichment,
    enrichOnly: true, forceDownload: false,
  }), /no usable traffic measurements/)
})

test('BASt matcher preserves exact-ref precedence and class-compatible two-kilometre fallback', () => {
  const autobahn = section()
  const bundesstrasse = section({ road: 'B 4', ref: 'B4', tkzst: '2001', lon: 10.3 })
  const census = indexBastCensus([autobahn, bundesstrasse])
  assert.equal(matchBastSection(road({ ref: ' A 1 ' }), census), autobahn)
  assert.equal(matchBastSection(road({ ref: null, roadClass: 1, midLon: 10.3 }), census), bundesstrasse)
  assert.equal(matchBastSection(road({ ref: null, roadClass: 0, midLon: 10.3 }), census), null)
  // A known but distant exact ref does not silently fall back to another road.
  assert.equal(matchBastSection(road({ ref: 'A1', roadClass: 1, midLon: 10.3 }), census), null)
  assert.equal(matchBastSection(road({ ref: 'A1', midLat: 50.134, midLon: 10 }), census), autobahn)
  assert.equal(matchBastSection(road({ ref: 'A1', midLat: 50.136, midLon: 10 }), census), null)
  assert.equal(matchBastSection(road({ ref: null, midLat: 50.0178, midLon: 10 }), census), autobahn)
  assert.equal(matchBastSection(road({ ref: null, midLat: 50.0182, midLon: 10 }), census), null)
})

test('z9 DE pass writes each true source id, updates an existing B stamp and skips foreign rows', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '275', '173')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('de-loader.arrow', [0, 1, 0], {
    refs: ['A1', 'B4', 'A1'],
    countryCodes: [iso2Code('DE'), iso2Code('DE'), iso2Code('CZ')],
    sourceIds: [23, 23, 0],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)
  const sections = [
    section({ lat: 50.00025, lon: 14.00025 }),
    section({ road: 'B 4', ref: 'B4', tkzst: '2001', lat: 50.00125, lon: 14.00125,
      dtv: 200, aadt_light: 150, aadt_medium: 10, aadt_heavy: 38, aadt_moto: 2 }),
  ]

  const result = await enrichGermanRoads(prepared, sections)
  assert.deepEqual(
    {
      matched: result.matched,
      autobahn: result.matchedAutobahn,
      bundesstrasse: result.matchedBundesstrasse,
      skippedForeign: result.skippedForeign,
    },
    { matched: 2, autobahn: 1, bundesstrasse: 1, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(3)].map((_, index) => table.getChild('source_id')!.get(index)), [22, 23, 0])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [80, 5, 14, 1],
  )
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(1)),
    [150, 10, 38, 2],
  )
  assert.equal(result.retracted, 1) // A current A claim replaces an obsolete B-owned stamp.
  const unrelated = [section({ ref: 'A99', road: 'A 99', lat: 47.2, lon: 6 })]
  const second = await enrichGermanRoads(prepared, unrelated)
  assert.equal(second.matched, 0); assert.equal(second.retracted, 2)
  const cleared = tableFromIPC(readFileSync(target))
  assert.deepEqual([...cleared.getChild('source_id')!], [0, 0, 0])
  for (const field of table.schema.fields) {
    if (['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].includes(field.name)) continue
    assert.deepEqual(cleared.getChild(field.name)!.toArray(), table.getChild(field.name)!.toArray())
  }
  const bytes = readFileSync(target), inode = statSync(target).ino
  assert.equal((await enrichGermanRoads(prepared, unrelated)).squaresUpdated, 0)
  assert.deepEqual(readFileSync(target), bytes); assert.equal(statSync(target).ino, inode)
})


test('invalid successful BASt responses never install or replace source cache bytes', async t => {
  t.mock.method(globalThis, 'fetch', async () => new Response('<html>upstream failure</html>', { status: 200 }))
  for (const retained of [false, true]) {
    const enrichment = join(TEST_DIRECTORY, `invalid-download-${retained}`)
    const directory = join(enrichment, 'de')
    const names = ['svz-autobahnen-2021.xlsx', 'svz-bundesstrassen-2021.xlsx', 'svz-census.json']
    if (retained) {
      mkdirSync(directory, { recursive: true })
      for (const name of names) writeFileSync(join(directory, name), `retained bytes: ${name}`)
    }
    await assert.rejects(loadBastCensus({ preparedDirectory: TEST_DIRECTORY,
      enrichmentDirectory: enrichment, enrichOnly: false, forceDownload: true }), /xlsx/)
    for (const name of names) {
      const path = join(directory, name)
      if (retained) assert.equal(readFileSync(path, 'utf8'), `retained bytes: ${name}`)
      else assert.equal(existsSync(path), false)
    }
  }
})
