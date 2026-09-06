/**
 * Guard rail for the USWTDB reader: the upstream CSV quotes commas inside
 * project names, so a `split(',')` parser shifts every field after them.
 *
 * Run: `cd pipeline && npx tsx --test enrich-global-windturbines.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { cellToLatLng, gridDisk } from 'h3-js'
import { parseUswtdbCsv, registryRecordsAround, type Turbine } from './enrich-global-windturbines.js'

const HEADER = 'case_id,faa_ors,p_name,t_manu,t_model,t_cap,t_hh,t_rd,xlong,ylat'

test('a quoted comma in p_name keeps every later column in its own place', () => {
  // case_id 3034117 verbatim from the shipped cache: `split(',')` turns this
  // 28-column row into 29 and drops the record (380 of 72,445 records lost).
  const csv = [
    HEADER,
    '3034117,,"Adams Wind Generations, LLC",Vestas,V47,660,55,47,-94.6,43.7',
    '3005001,,Plain Name,GE,1.5-77,1500,80,77,-101.2,35.1',
  ].join('\r\n')

  const turbines = parseUswtdbCsv(csv)
  assert.equal(turbines.length, 2)
  assert.deepEqual(
    turbines.map((t) => [t.name, t.manu, t.model, t.ratedPowerKw, t.hubHeight, t.rotorDiam, t.lon, t.lat]),
    [
      ['Adams Wind Generations, LLC', 'Vestas', 'V47', 660, 55, 47, -94.6, 43.7],
      ['Plain Name', 'GE', '1.5-77', 1500, 80, 77, -101.2, 35.1],
    ],
  )
})

test('a hex sees the registry records of its neighbours, so the radius works across hex edges', () => {
  // A 500 m radius reaches past an R4 hex edge; the registry files a record
  // under the hex that holds it, which is not always the hex of the OSM row.
  const home = '8426297ffffffff'
  const neighbour = gridDisk(home, 1).find((hex) => hex !== home)!
  const far = gridDisk(home, 2).find((hex) => !gridDisk(home, 1).includes(hex))!
  const record = (hex: string): Turbine => {
    const [lat, lon] = cellToLatLng(hex)
    return { lat, lon, hubHeight: 84, ratedPowerKw: 4200, rotorDiam: 0, name: hex, model: '', manu: '', h3r4: hex }
  }
  const byHex = new Map([[neighbour, [record(neighbour)]], [far, [record(far)]]])
  assert.deepEqual(registryRecordsAround(byHex, home).map((t) => t.name), [neighbour])
})

