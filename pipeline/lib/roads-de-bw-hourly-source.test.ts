/** BW hourly parsing contracts: complete-day gating, flag handling, period windows. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  BW_HOURLY_SOURCE_URL, parseStationMonth, stationProfiles,
} from './roads-de-bw-hourly-source.js'
import { matchBwStation } from '../enrich-roads-de.js'
import type { RoadRow } from './roads-arrow.js'
import type { BwStationProfile } from './roads-de-bw-hourly-source.js'

const STATION = { ref: 'A98', klasse: 'A', lat: 47.83, lon: 8.95 }

/** Header: lanes 01+01, nine classes, per-lane class columns + quality flags. */
function csvForMonth(rows: string[], lanes = '01;01'): string {
  const classColumns = ['Mot', 'Pkw', 'Lfw', 'PmA', 'Bus', 'LoA', 'LmA', 'Sat', 'So']
  const header = ['Datum', 'Uhrzeit']
  for (const lane of [1, 2]) for (const cls of classColumns) header.push(`${cls}FS${lane}`, `K_${cls}FS${lane}`)
  return [
    '8119;91040;08;A;98;site;;;;;;;;',
    `${lanes};${lanes};site;N;;S;;;;;`,
    '02;09;KFZ;SV;Mot;Pkw;Lfw;PmA;Bus;LoA;LmA;Sat;So;;;;',
    header.join(';'),
    ...rows,
  ].join('\r\n')
}

/** One row: Pkw per lane halves to 10/4/1 vehicles by period; LmA 1/2/4; rest 0. */
function completeDay(month: number): string[] {
  const date = `25${String(month).padStart(2, '0')}01`
  return COMPLETE_DAY.map(row => row.replace('250101', date))
}

function hourRow(date: string, hour: number, pkw: number, lma: number, flag = '-'): string {
  const value = (cls: string, lane: number): number => {
    if (cls === 'Pkw') return Math.round(pkw / 2)
    if (cls === 'LmA') return Math.round(lma / 2)
    return 0
  }
  const cells = [date, `${String(hour).padStart(2, '0')}:00`]
  for (const lane of [1, 2]) {
    for (const cls of ['Mot', 'Pkw', 'Lfw', 'PmA', 'Bus', 'LoA', 'LmA', 'Sat', 'So']) {
      cells.push(cls === 'So' ? '3' : String(value(cls, lane)), cls === 'So' ? '-' : flag)
    }
  }
  return cells.join(';')
}

const COMPLETE_DAY = Array.from({ length: 24 }, (_, index) => {
  const hour = index + 1
  // Same period classification as the parser: hour columns are ENDING hours,
  // so day = 08..19, evening = 20..23, night = 24 + 01..07.
  const period = hour >= 8 && hour <= 19 ? 0 : hour >= 20 && hour <= 23 ? 1 : 2
  const pkwByPeriod = [10, 4, 2]
  const lmaByPeriod = [2, 4, 8]
  return hourRow('250101', hour, pkwByPeriod[period], lmaByPeriod[period])
})

test('complete observed days aggregate into per-class period shares', () => {
  const totals = new Map()
  const parsed = parseStationMonth(csvForMonth(completeDay(1)), 1, STATION, '81191040', totals)
  assert.equal(parsed.completeDays, 1)
  assert.equal(parsed.rejectedDays, 0)
  const dataset = stationProfiles(totals)
  const station = dataset.stations[0]
  // light: 12×10 + 4×4 + 8×2 = 152 → day/evening/night = 120/16/16
  assert.deepEqual(station.counts.light, [120, 16, 16])
  assert.ok(Math.abs(station.shares.light![0] - 120 / 152) < 1e-12)
  assert.ok(Math.abs(station.shares.light![2] - 16 / 152) < 1e-12)
  // heavy: 12×2 + 4×4 + 8×8 = 104 → night share 64/104 (truck night ≫ car night)
  assert.deepEqual(station.counts.heavy, [24, 16, 64])
  assert.ok(Math.abs(station.shares.heavy![2] - 64 / 104) < 1e-12)
  // So (unknown vehicles) is read but never mapped into a class; moto without
  // counts stays absent = genuinely unknown, never a zero-profile.
  assert.equal(station.shares.moto, undefined)
  assert.equal(station.shares.light!.reduce((a, b) => a + b, 0), 1)
  assert.equal(station.days, 1)
  assert.equal(station.daysByMonth['01'], 1)
  assert.ok(station.status.includes('combined both-direction'))
  assert.ok(station.status.includes('So unclassified vehicles 144 counted but excluded'), 'unknown vehicles documented: ' + station.status)
})

test('days with bad flags, duplicates, or missing hours are rejected, never zero-filled', () => {
  const rows = [
    ...COMPLETE_DAY.map(row => row.replace('250101', '250201')),
    // 250202: hours flagged missing ("a") → whole day rejected, never zero-filled.
    ...COMPLETE_DAY.map(row => row.replace('250101', '250202').replace(';-', ';a')),
    // 250203: duplicate hour 05.
    ...COMPLETE_DAY.map(row => row.replace('250101', '250203')),
    '250203;05:00;' + COMPLETE_DAY[4].split(';').slice(2).join(';'),
    // 250204: only 23 hours.
    ...COMPLETE_DAY.slice(0, 23).map(row => row.replace('250101', '250204')),
  ]
  const totals = new Map()
  const parsed = parseStationMonth(csvForMonth(rows.map(row => row.replace('250101', '250201'))), 2, STATION, '81191040', totals)
  assert.equal(parsed.completeDays, 1)
  assert.equal(parsed.rejectedDays, 3)
  const dataset = stationProfiles(totals)
  assert.equal(dataset.stations[0].days, 1)
})

test('station-months without all nine measured classes are skipped whole', () => {
  const text = csvForMonth(completeDay(3)).replace('02;09;', '02;07;')
  const totals = new Map()
  const parsed = parseStationMonth(text, 3, STATION, '81191040', totals)
  assert.deepEqual(parsed, { completeDays: 0, rejectedDays: 0 })
  assert.equal(totals.size, 0)
})

test('irregular-but-correct counts (u) are observations', () => {
  const totals = new Map()
  const parsed = parseStationMonth(
    csvForMonth(completeDay(4).map(row => row.replace(/;-/g, ';u'))), 4, STATION, '81191040', totals)
  assert.equal(parsed.completeDays, 1)
})

test('matchBwStation is exact-ref and locality-bound, never class-based', () => {
  const station = {
    svznr: '81191040', ref: 'B31', klasse: 'B', lat: 47.83, lon: 8.95,
    windowFrom: '2025-01', windowTo: '2025-12', daysByMonth: {}, days: 10,
    status: '', counts: {}, shares: { light: [0.8, 0.1, 0.1] },
  } as BwStationProfile
  const byRef = new Map([['B31', [station]]])
  const row: RoadRow = { ref: 'B 31', midLat: 47.8301, midLon: 8.9501, roadClass: 2 } as RoadRow
  assert.equal(matchBwStation(row, byRef), station)
  // A different road class but the same numbered ref still matches (the ref
  // names the road); a different ref never falls back to class or proximity.
  assert.equal(matchBwStation({ ...row, roadClass: 5 } as never, byRef), station)
  assert.equal(matchBwStation({ ...row, ref: 'K 2830' } as never, byRef), null)
  assert.equal(matchBwStation({ ...row, ref: null } as never, byRef), null)
  // Beyond the 200 m station locality the observation does not transfer.
  const far = matchBwStation({ ref: 'B31', midLat: 47.832, midLon: 8.952 } as never, byRef)
  assert.equal(far, null)
})

test('canonical source URL is the public dataset page', () => {
  assert.equal(BW_HOURLY_SOURCE_URL, 'https://mobidata-bw.de/de/dataset/stundenwerte_dauerzaehlstellen')
})

test('invalid calendar dates and empty counts never become observations', () => {
  assert.throws(() => parseStationMonth(
    csvForMonth(completeDay(2).map(row => row.replace('250201', '250229'))),
    2, STATION, '81191040', new Map()), /malformed row/)
  const missing = completeDay(2)
  missing[0] = missing[0].replace(';0;-', ';;-')
  assert.equal(parseStationMonth(csvForMonth(missing), 2, STATION, '81191040', new Map()).completeDays, 0)
})
