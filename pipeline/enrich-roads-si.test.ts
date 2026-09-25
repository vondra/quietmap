/** Slovenian PLDP parser, matcher and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichSlovenianRoads } from './enrich-roads-si.js'
import { iso2Code } from './lib/prepared-grid.js'
import { loadSlovenianPldpSource, parseSlovenianPldpSource } from './lib/roads-si-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_SI_NATIONAL_ROADS } from './lib/source-ids.generated.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-si-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const COUNTS_HEADER =
  'Kat. ceste;Štev. ceste;Štev. odseka;Prometni odsek;Stac. začetka;Stac. konca;Števno mesto;Ime števnega mesta;Tip   štetja;Vsa vozila (PLDP);Motorji;Osebna vozila;Avtobusi;Lah. tov.  < 3,5t;Sr. tov.  3,5-7t;Tež. tov. nad 7t;Tov. s prik.;Vlačilci;Dnevni NOO;Tip'
const SITES_HEADER =
  'Števno mesto;Številka ceste;Številka odseka;Števna točka;Ime števnega mesta;Tip štetja;Šteje se ena smer;Leto roč. (avt.) štetja;Koordinata E;Koordinata N'

const countRow = (overrides: Record<string, string> = {}): string => {
  const fields: Record<string, string> = {
    'Kat. ceste': 'AC',
    'Štev. ceste': 'A1',
    'Štev. odseka': '0031',
    'Prometni odsek': 'ŠENTILJ - PESNICA',
    'Stac. začetka': '0',
    'Stac. konca': '9.761',
    'Števno mesto': '837',
    'Ime števnega mesta': 'Pesnica AC',
    'Tip   štetja': 'QLTC8',
    'Vsa vozila (PLDP)': '33.289',
    Motorji: '81',
    'Osebna vozila': '25.837',
    Avtobusi: '180',
    'Lah. tov.  < 3,5t': '3.455',
    'Sr. tov.  3,5-7t': '205',
    'Tež. tov. nad 7t': '94',
    'Tov. s prik.': '484',
    Vlačilci: '2.953',
    'Dnevni NOO': '2979',
    Tip: 'PLDP',
    ...overrides,
  }
  return COUNTS_HEADER.split(';')
    .map(name => fields[name])
    .join(';')
}

// Ljubljana D96/TM: E 461761, N 102019 is 14.5058 E, 46.0569 N.
const siteRow = (site = '837', east = '461761', north = '102019'): string =>
  [site, 'A1', '0031', '9.060', 'Pesnica AC', 'QLTC8', '', '', east, north].join(';')

test('Slovenian parser joins sections to sites and maps the eight classes', () => {
  const parsed = parseSlovenianPldpSource(
    [COUNTS_HEADER, countRow(), 'Tip štetja;;;;;;;;;;;;;;;;;;;'].join('\r\n'),
    [SITES_HEADER, siteRow()].join('\r\n'),
  )
  assert.deepEqual(
    {
      counts: parsed.countRows,
      sites: parsed.siteRows,
      observations: parsed.observations.length,
    },
    { counts: 1, sites: 1, observations: 1 },
  )
  const [observation] = parsed.observations
  assert.ok(Math.abs(observation.latitude - 46.0569) < 0.001)
  assert.ok(Math.abs(observation.longitude - 14.5058) < 0.001)
  assert.deepEqual(
    {
      light: observation.light,
      medium: observation.medium,
      heavy: observation.heavy,
      moto: observation.moto,
      estimated: observation.estimatedClasses,
      rank: observation.rank,
    },
    { light: 29292, medium: 385, heavy: 3531, moto: 81, estimated: 0, rank: 0 },
  )
})

test('Slovenian parser skips estimated totals and marks imputed classes', () => {
  const parsed = parseSlovenianPldpSource(
    [
      COUNTS_HEADER,
      countRow({ 'Števno mesto': '', 'Tip   štetja': 'P' }),
      countRow({ 'Števno mesto': '838', 'Tip   štetja': 'MWTC1' }),
      countRow({ 'Števno mesto': '839', 'Kat. ceste': 'R3-NK' }),
      countRow({ 'Števno mesto': '999' }),
    ].join('\r\n'),
    [SITES_HEADER, siteRow('838'), siteRow('839')].join('\r\n'),
  )
  assert.deepEqual(
    {
      estimated: parsed.estimatedSkipped,
      uncategorized: parsed.uncategorizedSkipped,
      unlocated: parsed.unlocatedSkipped,
      observations: parsed.observations.length,
    },
    { estimated: 1, uncategorized: 1, unlocated: 1, observations: 1 },
  )
  assert.equal(parsed.observations[0].estimatedClasses, 15)
  assert.throws(
    () =>
      parseSlovenianPldpSource(
        [COUNTS_HEADER, countRow({ 'Vsa vozila (PLDP)': '33.000' })].join('\r\n'),
        [SITES_HEADER, siteRow()].join('\r\n'),
      ),
    /do not sum to its total/,
  )
})

test('Slovenian pinned loader rejects bytes outside the admitted release source', () => {
  const enrichmentDirectory = join(DIRECTORY, 'source')
  mkdirSync(join(enrichmentDirectory, 'si'), { recursive: true })
  writeFileSync(join(enrichmentDirectory, 'si', 'pldp2024noo.csv'), [COUNTS_HEADER, countRow()].join('\r\n'))
  writeFileSync(join(enrichmentDirectory, 'si', 'stm2024.csv'), [SITES_HEADER, siteRow()].join('\r\n'))
  assert.throws(
    () =>
      loadSlovenianPldpSource({
        preparedDirectory: join(DIRECTORY, 'unused'),
        enrichmentDirectory,
        enrichOnly: true,
        forceDownload: false,
      }),
    /does not match the admitted release source/,
  )
})

test('z9 Slovenian pass writes classes, retracts stale claims and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '276', '181')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('si-loader.arrow', [0, 0, 0], {
    origin: [14.5058, 46.0569],
    refs: ['A1', 'A1', 'A1'],
    countryCodes: [iso2Code('SI'), iso2Code('HR'), iso2Code('SI')],
    sourceIds: [0, SOURCE_ID_SI_NATIONAL_ROADS, SOURCE_ID_SI_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const parsed = parseSlovenianPldpSource(
    [COUNTS_HEADER, countRow()].join('\r\n'),
    [SITES_HEADER, siteRow()].join('\r\n'),
  )
  const result = await enrichSlovenianRoads(prepared, parsed.observations)
  assert.deepEqual(
    {
      matched: result.matched,
      retracted: result.retracted,
      skippedForeign: result.skippedForeign,
    },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_SI_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [29292, 385, 3531, 81],
  )
})
