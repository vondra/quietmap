/** Offline parser checks for the committed LKPR TANOS monthly text extract. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { parseLkprMonthlyReport } from './snapshot-lkpr.ts'

test('LKPR monthly sample parses all 14 fixed NMTs without unit conversion', () => {
  const rows = parseLkprMonthlyReport(readFileSync(resolve(import.meta.dirname, 'fixtures/lkpr-2026-01.txt'), 'utf8'))
  assert.equal(rows.length, 14)
  assert.deepEqual(rows[0], { station_id: 'mp01', name: 'Jeneč', month: '2026-01', laeq_aircraft_day_0622: 55.2, laeq_aircraft_night_2206: 48.6 })
  assert.equal(rows[11].laeq_aircraft_night_2206, null, 'published missing aircraft-night mean remains missing')
})

test('Czech March and June names normalize before station validation', () => {
  for (const [name, expectedMonth] of [['březen', '2026-03'], ['červen', '2026-06']] as const) {
    const report = readFileSync(resolve(import.meta.dirname, 'fixtures/lkpr-2026-01.txt'), 'utf8').replaceAll('leden', name)
    assert.equal(parseLkprMonthlyReport(report)[0].month, expectedMonth)
  }
})
