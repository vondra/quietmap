/**
 * Guard rail for the USWTDB reader: the upstream CSV quotes commas inside
 * project names, so a `split(',')` parser shifts every field after them.
 *
 * Run: `cd pipeline && npx tsx --test enrich-global-windturbines.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseUswtdbCsv } from './enrich-global-windturbines.js'

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
