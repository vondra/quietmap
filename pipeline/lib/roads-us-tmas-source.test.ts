/** TMAS parsing contracts: complete-day gating, lane/direction scope, totals. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  parseStationMetadata, parseVolumeMonth, stationProfiles, TMAS_SOURCE_URL, TMAS_FORMAT_REFERENCE,
} from './roads-us-tmas-source.js'

const STA_HEADER = 'Record_Type|State_Code|Station_Id|Travel_Dir|Travel_Lane|Year_Record|F_System|Number_Lanes_In_Direction|Vehicle_Classification_Groupings|Calibration_of_Weighing_System|Type_Sensor_1|Type_Sensor_2|Latitude|Longitude|Prev_Station_Id|Year_Established|Year_Discontinued|County_Code|NHS|Posted_Route_Signing|Posted_Route_Sign_Number|Station_Location'
const VOL_HEADER = 'Record_Type|State_Code|F_System|Station_Id|Travel_Dir|Travel_Lane|Year_Record|Month_Record|Day_Record|Day_of_Week|Restrictions|Time_Increment|' +
  Array.from({ length: 24 }, (_, hour) => `Hour_${String(hour).padStart(2, '0')}`).join('|')

function staRow(station: string, dir: string, lane: string, route = '5', lat = '40.55', lon = '-122.38', fSystem = '1'): string {
  return `S|6|${station}|${dir}|${lane}|2025|${fSystem}|2|15| |P|N|${lat}|${lon}| |1997|2000|23|1|3|${route}|SITE`
}

/** One hour value per period: night (23,0–6)=1, day (7–18)=2, evening (19–22)=3. */
const PERIOD_HOURS: readonly number[] = Array.from({ length: 24 }, (_, hour) =>
  hour === 23 || hour < 7 ? 1 : hour < 19 ? 2 : 3)

function volRow(station: string, dir: string, lane: string, month: number, day: number,
  hours: readonly (number | string)[] = PERIOD_HOURS, restrictions = '', year = 2025, fSystem = '1'): string {
  return ['3', '6', fSystem, station, dir, lane, year, month, day, '3', restrictions, '',
    ...hours].join('|')
}

function loadMetadata(rows: string[], state = 'CA') {
  const metadata = new Map()
  parseStationMetadata([STA_HEADER, ...rows].join('\r\n'), state, 'CA_2025 (TMAS).STA', metadata)
  return metadata
}

test('complete observed days aggregate into TOTAL day/evening/night shares', () => {
  const metadata = loadMetadata([staRow('021560', '1', '0')])
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER, volRow('021560', '1', '0', 7, 1), volRow('021560', '1', '0', 7, 2)].join('\r\n'),
    'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  parseVolumeMonth([VOL_HEADER, volRow('021560', '1', '0', 8, 1, PERIOD_HOURS, '0')].join('\r\n'),
    'CA', 8, 'CA_Aug_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const [profile] = stationProfiles(metadata, directionMonths, rejected)
  // day 12×2=24, evening 4×3=12, night 8×1=8 over three days.
  assert.deepEqual(profile.counts, [72, 36, 24])
  assert.deepEqual(profile.shares.map(v => Math.round(v * 10000) / 10000), [0.5455, 0.2727, 0.1818])
  assert.equal(profile.station, 'CA021560:D1')
  assert.equal(profile.routeNumber, '5')
  assert.equal(profile.rank, 0)
  assert.equal(profile.windowFrom, '2025-07')
  assert.equal(profile.windowTo, '2025-08')
  assert.deepEqual(profile.daysByMonth, { '07': 2, '08': 1 })
  assert.equal(profile.days, 3)
  assert.ok(profile.status.includes('direction 1 (individual compass'))
  assert.ok(profile.status.includes('lanes 0=combined'))
  assert.ok(profile.status.includes('restriction flag blank/0 accepted'))
  assert.equal(TMAS_SOURCE_URL, 'https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/')
  assert.ok(TMAS_FORMAT_REFERENCE.includes('traffic-data-formats.cfm'))
})

test('incomplete hours, invalid dates, non-zero restrictions and duplicates never become observations', () => {
  const metadata = loadMetadata([staRow('011040', '1', '0')])
  const blank: (number | string)[] = [...PERIOD_HOURS]; blank[5] = ''
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  const text = [VOL_HEADER,
    volRow('011040', '1', '0', 7, 1), // complete
    volRow('011040', '1', '0', 7, 2, blank), // one blank hour: whole day rejected
    volRow('011040', '1', '0', 7, 3), volRow('011040', '1', '0', 7, 3), // duplicate: conflicting
    volRow('011040', '1', '0', 7, 32), // invalid calendar day
    volRow('011040', '1', '0', 7, 4, PERIOD_HOURS, '2'), // malfunction: excluded
    volRow('011040', '1', '0', 7, 5, PERIOD_HOURS, '', 2024), // wrong year
    volRow('011040', '1', '0', 8, 1), // wrong month for the member
  ].join('\r\n')
  const parsed = parseVolumeMonth(text, 'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  assert.equal(parsed.observationRows, 8)
  assert.equal(parsed.rejectedRows, 4)
  assert.deepEqual(parsed.rejected, { 'incomplete-hours': 1, 'duplicate-day': 1, 'invalid-date': 3, 'restricted-day': 1 })
  const [profile] = stationProfiles(metadata, directionMonths, rejected)
  assert.equal(profile.days, 1)
})

test('per-lane months missing a lane of the yearly scope are not complete coverage', () => {
  const metadata = loadMetadata([staRow('000105', '1', '1'), staRow('000105', '1', '2')], 'NY')
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER, volRow('000105', '1', '1', 7, 1), volRow('000105', '1', '2', 7, 1)].join('\r\n'),
    'NY', 7, 'NY_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  // August reports only lane 1: not the yearly {1,2} scope, excluded whole.
  parseVolumeMonth([VOL_HEADER, volRow('000105', '1', '1', 8, 1)].join('\r\n'),
    'NY', 8, 'NY_Aug_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const [profile] = stationProfiles(metadata, directionMonths, rejected)
  assert.equal(rejected['missing-declared-lane'], 1)
  assert.deepEqual(profile.daysByMonth, { '07': 1 })
  assert.deepEqual(profile.counts, [48, 24, 16]) // one day, both lanes summed
  assert.ok(profile.status.includes('lanes 1, 2'))
})

test('combined directions are totals, never mixed with the individuals they span', () => {
  const metadata = loadMetadata([
    staRow('000200', '0', '0'), staRow('000200', '3', '0'), staRow('000200', '7', '0'),
    staRow('000300', '9', '0'),
  ], 'TX')
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER,
    volRow('000200', '0', '0', 7, 1), // dir 0 combined beside individuals 3/7
    volRow('000200', '3', '0', 7, 1), volRow('000200', '7', '0', 7, 1),
    volRow('000300', '9', '0', 7, 1), // lone N–S combined: a valid total
  ].join('\r\n'), 'TX', 7, 'TX_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const profiles = new Map(stationProfiles(metadata, directionMonths, rejected)
    .map(profile => [profile.station, profile]))
  assert.equal(profiles.size, 3)
  assert.ok(!profiles.has('TX000200:D0'), 'combined beside individuals is dropped')
  assert.ok(profiles.has('TX000200:D3'))
  assert.ok(profiles.has('TX000200:D7'))
  const combined = profiles.get('TX000300:D9')!
  assert.ok(combined.status.includes('N–S (or NE–SW) COMBINED total'), combined.status)
})

test('all-lane rows beside per-lane rows and VOL keys without STA scope are rejected', () => {
  const metadata = loadMetadata([staRow('000400', '1', '0'), staRow('000400', '1', '1')])
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER,
    volRow('000400', '1', '0', 7, 1), volRow('000400', '1', '1', 7, 1), // lane 0 beside lane 1
    volRow('000999', '1', '0', 7, 1), // station direction absent from metadata
  ].join('\r\n'), 'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  assert.equal(stationProfiles(metadata, directionMonths, rejected).length, 0)
  assert.equal(rejected['conflicting-lane-scope'], 1)
  assert.equal(rejected['unscoped-observation'], 1)
})

test('per-direction coordinates never average across directions; unanimous routes only', () => {
  const metadata = loadMetadata([
    staRow('000500', '1', '0', '5', '40.5500', '-122.3800'),
    staRow('000500', '5', '0', '5', '40.5600', '-122.3900'),
    staRow('000600', '1', '0', '5'), staRow('000600', '3', '0', '99'),
  ])
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER,
    volRow('000500', '1', '0', 7, 1), volRow('000500', '5', '0', 7, 1),
    volRow('000600', '1', '0', 7, 1), volRow('000600', '3', '0', 7, 1),
  ].join('\r\n'), 'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const profiles = new Map(stationProfiles(metadata, directionMonths, rejected)
    .map(profile => [profile.station, profile]))
  assert.deepEqual([profiles.get('CA000500:D1')!.latitude, profiles.get('CA000500:D1')!.longitude], [40.55, -122.38])
  assert.deepEqual([profiles.get('CA000500:D5')!.latitude, profiles.get('CA000500:D5')!.longitude], [40.56, -122.39])
  assert.equal(profiles.get('CA000600:D1')!.routeNumber, '5')
  assert.equal(profiles.get('CA000600:D3')!.routeNumber, '99')
})

test('numeric F_System uses the official legacy crosswalk, including rural collectors', () => {
  const expected = new Map([['1', 0], ['2', 2], ['6', 3], ['7', 4], ['8', 5], ['9', 6],
    ['11', 0], ['12', 1], ['14', 2], ['16', 3], ['17', 4], ['19', 6], ['3', null], ['18', null]])
  for (const [code, rank] of expected) {
    const metadata = loadMetadata([staRow('000700', '1', '0', '5', '40.55', '-122.38', code)])
    const months = new Map(), rejected: Record<string, number> = {}
    parseVolumeMonth([VOL_HEADER, volRow('000700', '1', '0', 7, 1, PERIOD_HOURS, '', 2025, code)].join('\n'),
      'CA', 7, 'fixture', metadata, months, rejected)
    assert.equal(stationProfiles(metadata, months, rejected)[0].rank, rank, code)
  }
})

test('same station id in two states groups combined directions independently', () => {
  const metadata = loadMetadata([
    staRow('000700', '0', '0', '5', '40.55', '-122.38'),  // CA: combined beside individuals
    staRow('000700', '1', '0', '5', '40.55', '-122.38'),
  ])
  parseStationMetadata([STA_HEADER,
    `S|48|000700|9|0|2025|1|2|15| |P|N|31.10|-97.30| |1997|2000|23|1|3|5|SITE`, // TX: lone N-S combined
  ].join('\r\n'), 'TX', 'TX_2025 (TMAS).STA', metadata)
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER,
    volRow('000700', '0', '0', 7, 1), volRow('000700', '1', '0', 7, 1),
  ].join('\r\n'), 'CA', 7, 'CA_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const txHeader = VOL_HEADER.replace('State_Code', 'State_Code') // same columns
  const txVol = [txHeader,
    ['3', '48', '1', '000700', '9', '0', '2025', '7', '1', '3', '', '', ...PERIOD_HOURS].join('|'),
  ].join('\r\n')
  const stateCode = { stateCode: '48' }
  void stateCode
  parseVolumeMonth(txVol, 'TX', 7, 'TX_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const profiles = new Map(stationProfiles(metadata, directionMonths, rejected)
    .map(profile => [profile.station, profile]))
  assert.ok(!profiles.has('CA000700:D0'), 'CA combined dropped beside its individual')
  assert.ok(profiles.has('CA000700:D1'))
  assert.ok(profiles.has('TX000700:D9'), 'TX lone combined survives despite the CA collision')
  assert.equal(rejected['combined-direction-duplicated'], 1)
})

test('STA-declared lane scope gates VOL observations; official all-lane reporting survives', () => {
  // STA declares lane 0 beside per-lanes (the CA shape): VOL reporting the
  // combined lane 0 is the official summary and must count.
  const metadata = loadMetadata([staRow('021560', '1', '0'), staRow('021560', '1', '1'), staRow('021560', '1', '2')])
  const directionMonths = new Map()
  const rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER, volRow('021560', '1', '0', 7, 1)].join('\r\n'),
    'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
  const [profile] = stationProfiles(metadata, directionMonths, rejected)
  assert.equal(profile.station, 'CA021560:D1')
  assert.equal(profile.days, 1)
  assert.ok(profile.status.includes('lanes 0=combined'))
  assert.ok(profile.status.includes('STA declares 0,1,2'), profile.status)
  // A VOL lane the STA never declared is unsupported, not silently accepted.
  const months2 = new Map()
  const rejected2: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER, volRow('021560', '1', '3', 7, 1)].join('\r\n'),
    'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, months2, rejected2)
  assert.equal(stationProfiles(metadata, months2, rejected2).length, 0)
  assert.equal(rejected2['unsupported-lane'], 1)
})

test('a restricted duplicate invalidates the earlier valid day in any order', () => {
  const metadata = loadMetadata([staRow('001000', '1', '0')])
  const orders = [
    [volRow('001000', '1', '0', 7, 1), volRow('001000', '1', '0', 7, 1, PERIOD_HOURS, '2')],
    [volRow('001000', '1', '0', 7, 1, PERIOD_HOURS, '2'), volRow('001000', '1', '0', 7, 1)],
  ]
  for (const rows of orders) {
    const directionMonths = new Map()
    const rejected: Record<string, number> = {}
    parseVolumeMonth([VOL_HEADER, ...rows].join('\r\n'),
      'CA', 7, 'CA_Jul_2025 (TMAS).VOL', metadata, directionMonths, rejected)
    assert.equal(stationProfiles(metadata, directionMonths, rejected).length, 0,
      'malfunction duplicate must invalidate the day, never first-wins')
  }
})


test('a lane declared in STA but absent from the entire observed month is not full direction coverage', () => {
  const metadata = loadMetadata([staRow('000700', '1', '1'), staRow('000700', '1', '2')])
  const months = new Map(), rejected: Record<string, number> = {}
  parseVolumeMonth([VOL_HEADER, volRow('000700', '1', '1', 7, 1)].join('\n'),
    'CA', 7, 'fixture', metadata, months, rejected)
  assert.deepEqual(stationProfiles(metadata, months, rejected), [])
  assert.equal(rejected['missing-declared-lane'], 1)
})
