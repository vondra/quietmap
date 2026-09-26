/** Dutch INWEVA parser, matcher and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichDutchRoads, enrichDutchTimeProfiles, indexDutchInweva, matchDutchInweva } from './enrich-roads-nl.js'
import { iso2Code } from './lib/prepared-grid.js'
import { loadDutchInwevaSource, parseDutchInwevaSource } from './lib/roads-nl-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_NL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-nl-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const section = (overrides: Record<string, unknown> = {}) => ({
  type: 'Feature',
  geometry: {
    type: 'LineString',
    coordinates: [
      [5, 52],
      [5.01, 52.01],
    ],
  },
  properties: {
    vbn_id: '100',
    vbn_id_tgn: null,
    bnsubsrt_b: 'HR',
    wegnrhmp_b: 'A2',
    wegnrhmp_e: 'A2',
    afst_mtw: 0,
    l1_e_wk: 2671,
    l2_e_wk: 143,
    l3_e_wk: 9,
    ...overrides,
  },
})

const featureCollection = (features: unknown[]): string => JSON.stringify({ type: 'FeatureCollection', features })

const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 52.004,
  startLon: 5.004,
  endLat: 52.006,
  endLon: 5.006,
  midLat: 52.005,
  midLon: 5.005,
  ref: 'A2',
  name: null,
  osmId: 1,
  roadClass: 0,
  existingSourceId: 0,
  ...overrides,
})

test('Dutch parser pairs reciprocal twins into one two-way observation', () => {
  const parsed = parseDutchInwevaSource(
    featureCollection([
      section({ vbn_id: '100', vbn_id_tgn: '200' }),
      section({
        vbn_id: '200',
        vbn_id_tgn: '100',
        l1_e_wk: 2500,
        l2_e_wk: 130,
        l3_e_wk: 11,
      }),
    ]),
  )
  assert.equal(parsed.observations.length, 1)
  assert.equal(parsed.pairedSections, 2)
  const [observation] = parsed.observations
  assert.equal(observation.countBasis, 'both-directions')
  assert.equal(observation.observationId, 'inweva2024:100-200')
  assert.equal(observation.lines.length, 2)
  // Moto is 1 % of the pair total (5,464), taken from the light class.
  assert.deepEqual(
    {
      light: observation.light,
      medium: observation.medium,
      heavy: observation.heavy,
      moto: observation.moto,
    },
    { light: 5171 - 55, medium: 273, heavy: 20, moto: 55 },
  )
})

test('Dutch parser keeps only measured sections on their own carriageway', () => {
  const parsed = parseDutchInwevaSource(
    featureCollection([
      section({ vbn_id: 'derived', afst_mtw: 1 }),
      section({ vbn_id: 'missing', l1_e_wk: null }),
      section({ vbn_id: 'bus', bnsubsrt_b: 'BUS' }),
      section({ vbn_id: 'lonely-n', wegnrhmp_b: 'N57', wegnrhmp_e: 'N57' }),
      section({ vbn_id: 'lonely-a' }),
      section({ vbn_id: 'ramp', bnsubsrt_b: 'OPR', vbn_id_tgn: null }),
    ]),
  )
  assert.deepEqual(
    {
      derived: parsed.derivedSectionsSkipped,
      missing: parsed.missingValuesSkipped,
      baansoort: parsed.unsupportedBaansoortSkipped,
      lonelyN: parsed.lonelyNationalRoadSkipped,
      lonely: parsed.lonelySections,
      observations: parsed.observations.length,
    },
    {
      derived: 1,
      missing: 1,
      baansoort: 1,
      lonelyN: 1,
      lonely: 2,
      observations: 2,
    },
  )
  const [carriageway, ramp] = parsed.observations
  assert.equal(carriageway.countBasis, 'directional')
  assert.equal(carriageway.isRamp, false)
  assert.equal(ramp.isRamp, true)
})

test('Dutch pinned loader rejects bytes outside the admitted release source', () => {
  const enrichmentDirectory = join(DIRECTORY, 'source')
  mkdirSync(join(enrichmentDirectory, 'nl'), { recursive: true })
  writeFileSync(join(enrichmentDirectory, 'nl', 'inweva-2024-weekdagen-ks.json'), featureCollection([section()]))
  assert.throws(
    () =>
      loadDutchInwevaSource({
        preparedDirectory: join(DIRECTORY, 'unused'),
        enrichmentDirectory,
        enrichOnly: true,
        forceDownload: false,
      }),
    /does not match the admitted release source/,
  )
})

test('Dutch matcher requires the road number, the heading and the class', () => {
  const parsed = parseDutchInwevaSource(
    featureCollection([
      section({ vbn_id: '100', vbn_id_tgn: '200' }),
      section({ vbn_id: '200', vbn_id_tgn: '100' }),
      section({
        vbn_id: 'ramp',
        bnsubsrt_b: 'OPR',
        wegnrhmp_b: 'A2',
        wegnrhmp_e: 'A2',
      }),
    ]),
  )
  const index = indexDutchInweva(parsed.observations)
  const pair = parsed.observations[0]
  assert.equal(matchDutchInweva(road(), index), pair)
  assert.equal(matchDutchInweva(road({ ref: 'A2;E25' }), index), pair)
  assert.equal(matchDutchInweva(road({ ref: 'A12' }), index), null)
  // A cross street over the section line takes nothing.
  assert.equal(
    matchDutchInweva(
      road({
        startLat: 52.004,
        startLon: 5.006,
        endLat: 52.006,
        endLon: 5.004,
      }),
      index,
    ),
    null,
  )
  // A residential row beside the motorway takes neither the pair nor the ramp.
  assert.equal(matchDutchInweva(road({ roadClass: 6 }), index), null)
  // The ramp stamps a slip road, never the mainline.
  assert.equal(matchDutchInweva(road({ roadClass: 10, ref: null }), index)?.isRamp, true)
  assert.equal(matchDutchInweva(road({ roadClass: 10 }), index)?.isRamp, true)
})

test('z9 Dutch pass writes classes, retracts stale claims and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '263', '169')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('nl-loader.arrow', [0, 0, 0], {
    origin: [5, 52],
    refs: ['A2', 'A2', 'A12'],
    countryCodes: [iso2Code('NL'), iso2Code('CZ'), iso2Code('NL')],
    sourceIds: [0, SOURCE_ID_NL_NATIONAL_ROADS, SOURCE_ID_NL_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const parsed = parseDutchInwevaSource(
    featureCollection([
      section({
        vbn_id: '100',
        vbn_id_tgn: '200',
        geometry: {
          type: 'LineString',
          coordinates: [
            [4.999, 51.999],
            [5.004, 52.004],
          ],
        },
      }),
      section({
        vbn_id: '200',
        vbn_id_tgn: '100',
        geometry: {
          type: 'LineString',
          coordinates: [
            [5.004, 52.004],
            [4.999, 51.999],
          ],
        },
      }),
    ]),
  )
  const result = await enrichDutchRoads(prepared, parsed.observations)
  assert.deepEqual(
    {
      matched: result.matched,
      retracted: result.retracted,
      skippedForeign: result.skippedForeign,
    },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_NL_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [5286, 286, 18, 56],
  )
  assert.deepEqual([...table.getChild('traffic_estimated')!], [8, 15, 15])
})

test('Dutch parser carries class-specific period shares, summed over twins, default where unpublished', () => {
  // l1 2671 = 2000+400+271, l2 143 = 100+30+13, l3 9 = 6+2+1.
  const periods = {
    l1_d_wk: 2000, l1_a_wk: 400, l1_n_wk: 271,
    l2_d_wk: 100, l2_a_wk: 30, l2_n_wk: 13,
    l3_d_wk: 6, l3_a_wk: 2, l3_n_wk: 1,
  }
  const parsed = parseDutchInwevaSource(
    featureCollection([
      section({ vbn_id: 'complete', ...periods }),
      section({ vbn_id: 'missing', l1_d_wk: null }),
      section({ vbn_id: 'twin-a', vbn_id_tgn: 'twin-b', ...periods }),
      section({
        vbn_id: 'twin-b',
        vbn_id_tgn: 'twin-a',
        l1_e_wk: 2671, l2_e_wk: 143, l3_e_wk: 9,
        l1_d_wk: 1000, l1_a_wk: 200, l1_n_wk: 1471,
        l2_d_wk: 100, l2_a_wk: 30, l2_n_wk: 13,
        l3_d_wk: 6, l3_a_wk: 2, l3_n_wk: 1,
      }),
    ]),
  )
  assert.equal(parsed.observations.length, 3)
  const [complete, missing, twins] = parsed.observations
  assert.deepEqual(complete.timeProfile?.shares, {
    light: [2000 / 2671, 400 / 2671, 271 / 2671],
    medium: [100 / 143, 30 / 143, 13 / 143],
    heavy: [6 / 9, 2 / 9, 1 / 9],
    moto: [2000 / 2671, 400 / 2671, 271 / 2671],
  })
  assert.equal(missing.timeProfile, undefined)
  // Twin volumes sum per class and period before the shares: l1 day 3000 of 5342.
  assert.deepEqual(twins.timeProfile?.shares.light, [3000 / 5342, 600 / 5342, 1742 / 5342])
  assert.deepEqual(twins.timeProfile?.shares.moto, twins.timeProfile?.shares.light)
})

test('Dutch parser caps the imputed moto share at the light class, conserving the total', () => {
  const parsed = parseDutchInwevaSource(
    featureCollection([section({ vbn_id: 'no-light', l1_e_wk: 0, l2_e_wk: 500, l3_e_wk: 500 })]),
  )
  const [observation] = parsed.observations
  assert.deepEqual(
    { light: observation.light, medium: observation.medium, heavy: observation.heavy, moto: observation.moto },
    { light: 0, medium: 500, heavy: 500, moto: 0 },
  )
})

test('Dutch time profiles stamp matched rows with the section shares, nothing else', async () => {
  const prepared = join(DIRECTORY, 'profiles')
  const square = join(prepared, 'z9', '263', '169')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('nl-profiles.arrow', [0, 0], {
    origin: [5, 52],
    refs: ['A2', 'A12'],
    countryCodes: [iso2Code('NL'), iso2Code('NL')],
    sourceIds: [0, 0],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const parsed = parseDutchInwevaSource(
    featureCollection([
      section({
        vbn_id: '100',
        l1_d_wk: 2000, l1_a_wk: 400, l1_n_wk: 271,
        l2_d_wk: 100, l2_a_wk: 30, l2_n_wk: 13,
        l3_d_wk: 6, l3_a_wk: 2, l3_n_wk: 1,
        geometry: {
          type: 'LineString',
          coordinates: [
            [4.999, 51.999],
            [5.004, 52.004],
          ],
        },
      }),
    ]),
  )
  const result = await enrichDutchTimeProfiles(prepared, parsed.observations)
  assert.equal(result.matched, 1)
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('traffic_profile_id')!], [1, 0])
  const dictionary = JSON.parse(table.schema.metadata.get('roads_time_profiles')!)
  assert.equal(dictionary.entries.length, 1)
  assert.equal(dictionary.entries[0].station, '100')
  assert.deepEqual(dictionary.entries[0].profile.light, [2000 / 2671, 400 / 2671, 271 / 2671])
  // Traffic columns are untouched by profile stamping.
  assert.deepEqual([...table.getChild('source_id')!], [0, 0])
})
