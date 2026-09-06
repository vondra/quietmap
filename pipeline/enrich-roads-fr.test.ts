/** FR Cerema tests for source correction, line matching and z9 provenance. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichFrenchRoads, indexCeremaCensus, matchCeremaSection,
} from './enrich-roads-fr.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  loadCeremaCensus, parseCeremaCsvFiles, type CeremaCensusSection,
} from './lib/roads-fr-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-fr-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

const HEADER = 'route;TMJA;ratio_PL;xD;yD;xF;yF'
const csv = (rows: readonly string[]): string => [HEADER, ...rows].join('\r\n')

function section(overrides: Partial<CeremaCensusSection> = {}): CeremaCensusSection {
  return {
    route: 'A0001', ref: 'A1', lat: 48.8, lon: 2.3,
    coords: [[2.3, 48.8], [2.31, 48.81]],
    tmja: 1000, ratio_pl: 0.17,
    aadt_light: 820, aadt_medium: 3, aadt_heavy: 167, aadt_moto: 10,
    ...overrides,
  }
}

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 50, startLon: 14, endLat: 50.001, endLon: 14.001,
    midLat: 50.0005, midLon: 14.0005, ref: 'A1', name: null,
    osmId: 1, roadClass: 0, existingSourceId: 0, ...overrides,
  }
}

test('Cerema parser reads the CRLF ratio field and applies only the published 2019 correction', () => {
  const parsed = parseCeremaCsvFiles(
    csv([
      'A0001;1000;17,192;665481,08;6878892,47;665966,61;6880317,6',
      'A0001;900;18,5;665481,08;6878892,47;665966,61;6880317,6',
      'A0002;0,4;17;665481,08;6878892,47;665966,61;6880317,6',
      'A0043;12000;12724;953398,44;6500285,14;956825,71;6491273,87',
      'N0007;5000;0;700000;6600000;701000;6601000',
      'N0006;1000;17;720000;6600000;721000;6601000',
    ]),
    csv([
      'A0001;800;125;665481,08;6878892,47;665966,61;6880317,6',
      'A0003;2000;112;661115,54;6866753,89;661436,37;6873324,45',
      'N0007;1000;12,5;700000;6600000;701000;6601000',
      'A0004;3000;647;662254,01;6858982,54;663631,57;6858755,68',
      'N0010;4000;0;710000;6610000;711000;6611000',
    ]),
  )
  assert.deepEqual(parsed.files, [
    {
      year: 2024, sourceRows: 6, accepted: 3, noTrafficSkipped: 1,
      missingHeavyRatioSkipped: 1, invalidHeavyRatioSkipped: 1,
      invalidCoordinatesSkipped: 0, outsideMetropolitanFranceSkipped: 0, duplicateSkipped: 0,
    },
    {
      year: 2019, sourceRows: 5, accepted: 2, noTrafficSkipped: 0,
      missingHeavyRatioSkipped: 1, invalidHeavyRatioSkipped: 1,
      invalidCoordinatesSkipped: 0, outsideMetropolitanFranceSkipped: 0, duplicateSkipped: 1,
    },
  ])
  assert.deepEqual(parsed.sections.map(value => value.ref), ['A1', 'A1', 'N6', 'A3', 'N7'])
  assert.ok(Math.abs(parsed.sections[0].ratio_pl - 0.17192) < 1e-12)
  assert.ok(Math.abs(parsed.sections[2].ratio_pl - 0.17) < 1e-12)
  assert.ok(Math.abs(parsed.sections[3].ratio_pl - 0.112) < 1e-12)
  assert.ok(Math.abs(parsed.sections[4].ratio_pl - 0.125) < 1e-12)
  assert.deepEqual(
    [parsed.sections[0].aadt_light, parsed.sections[0].aadt_medium,
      parsed.sections[0].aadt_heavy, parsed.sections[0].aadt_moto],
    [818, 3, 169, 10],
  )
  assert.deepEqual(
    parsed.sections.map(value => value.aadt_light + value.aadt_medium + value.aadt_heavy + value.aadt_moto),
    parsed.sections.map(value => value.tmja),
  )
})

test('Cerema matcher uses the full section line, normalized suffix ref and strict 20 km cap', () => {
  const measured = section({ route: 'A0005A', ref: 'A5A', coords: [[2, 48], [3, 48]] })
  const census = indexCeremaCensus([measured])
  assert.equal(matchCeremaSection(road({ ref: ' A 5A ', midLat: 48, midLon: 2.99 }), census), measured)
  assert.equal(matchCeremaSection(road({ ref: 'A5', midLat: 48, midLon: 2.99 }), census), null)
  assert.equal(matchCeremaSection(road({ ref: 'A5A', midLat: 48.179, midLon: 2.5 }), census), measured)
  assert.equal(matchCeremaSection(road({ ref: 'A5A', midLat: 48.181, midLon: 2.5 }), census), null)
  assert.equal(matchCeremaSection(road({ ref: 'A5A', midLat: 49, midLon: 2.99 }), census), null)
})

test('z9 FR pass writes source-derived classes, retracts stale claims and never crosses baked ownership', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '259', '176')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('fr-loader.arrow', [0, 0, 0], {
    origin: [2.3, 48.8], refs: ['A1', 'A1', 'A2'],
    countryCodes: [iso2Code('FR'), iso2Code('BE'), iso2Code('FR')],
    sourceIds: [0, 24, 24],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)

  const result = await enrichFrenchRoads(prepared, [section()])
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(3)].map((_, index) => table.getChild('source_id')!.get(index)), [24, 0, 0])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [820, 3, 167, 10],
  )
})

// An empty source file must not turn a retraction pass into traffic deletion.
test('Cerema empty or missing yearly census fails before retracting existing Arrow traffic', () => {
  const prepared = join(TEST_DIRECTORY, 'empty-census-prepared')
  const enrichment = join(TEST_DIRECTORY, 'empty-census-source')
  const square = join(prepared, 'z9', '259', '176')
  mkdirSync(square, { recursive: true })
  mkdirSync(join(enrichment, 'fr'), { recursive: true })
  const target = join(square, 'roads.arrow')
  copyFileSync(writeRoadsFixture('fr-empty-census.arrow', [0], {
    origin: [2.3, 48.8], countryCodes: [iso2Code('FR')], sourceIds: [24],
  }), target)
  const before = readFileSync(target)
  const run = () => spawnSync(process.execPath, ['--import', 'tsx',
    new URL('./enrich-roads-fr.ts', import.meta.url).pathname,
    '--prepared-dir', prepared, '--enrichment-dir', enrichment, '--enrich-only',
  ], { encoding: 'utf8', cwd: new URL('.', import.meta.url) })
  for (const newest of [HEADER, csv(['A0001;1000;17;665481;6878892;665966;6880317'])]) {
    writeFileSync(join(enrichment, 'fr', 'tmja-2024.csv'), newest)
    writeFileSync(join(enrichment, 'fr', 'tmja-2019.csv'), HEADER)
    const result = run()
    assert.notEqual(result.status, 0, result.stdout)
    assert.match(result.stderr, /empty/)
    assert.deepEqual(readFileSync(target), before)
  }
  rmSync(join(enrichment, 'fr', 'tmja-2019.csv'))
  const missing = run()
  assert.notEqual(missing.status, 0)
  assert.match(missing.stderr, /missing/)
  assert.deepEqual(readFileSync(target), before)
})


test('nearby same-year Cerema observations remain distinct while newer-year coverage wins', () => {
  const row = (route: string, total: number, x: number) => `${route};${total};12,5;${x};6878892;${x + 100};6878992`
  const parsed = parseCeremaCsvFiles(csv([row('A0001', 1000, 665400), row('A0001', 1400, 665450)]),
    csv([row('A0001', 700, 665400), row('N0007', 2000, 700000), row('N0007', 2600, 700050)]))
  assert.deepEqual(parsed.sections.map(section => section.tmja), [1000, 1400, 2000, 2600])
  assert.deepEqual(parsed.files.map(file => file.duplicateSkipped), [0, 1])
})

test('invalid successful Cerema responses never install or replace either yearly source', async t => {
  const valid = csv(['A0001;1000;17,2;665481;6878892;665966;6880317'])
  for (const bad of ['<html>upstream failure</html>', HEADER]) {
    for (const retained of [false, true]) {
      const enrichment = mkdtempSync(join(TEST_DIRECTORY, 'download-'))
      const directory = join(enrichment, 'fr')
      const names = ['tmja-2024.csv', 'tmja-2019.csv']
      if (retained) {
        mkdirSync(directory, { recursive: true })
        for (const name of names) writeFileSync(join(directory, name), valid)
      }
      const mocked = t.mock.method(globalThis, 'fetch', async (url: string | URL | Request) =>
        new Response(String(url).includes('2024') ? valid : bad, { status: 200 }))
      await assert.rejects(loadCeremaCensus({ preparedDirectory: TEST_DIRECTORY,
        enrichmentDirectory: enrichment, enrichOnly: false, forceDownload: true }), /CSV|empty/)
      mocked.mock.restore()
      for (const name of names) {
        const path = join(directory, name)
        if (retained) assert.equal(readFileSync(path, 'utf8'), valid)
        else assert.equal(existsSync(path), false)
      }
    }
  }
})
