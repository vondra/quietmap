// Why a source is loud, short enough for a column: steady, vehicles an hour, so many a day, week or
// month; trains and airport movements by the day; events how many times; steady sources say nothing.
import assert from 'node:assert/strict'
import test from 'node:test'

import { heardText } from '../src/components/noise/heard.ts'

const heard = (day, evening, night, steady = false) => ({ per_hour: { day, evening, night }, steady })

test('a road reads as steady, vehicles an hour, or vehicles a day, week or month', () => {
  assert.equal(heardText('road', heard(916, 845, 317, true)), 'steady')
  assert.equal(heardText('road', heard(76, 59, 20)), '76 veh/h')
  assert.equal(heardText('road', heard(3.78, 2.92, 0.97)), '4 veh/h')
  assert.equal(heardText('road', heard(0.59, 0.46, 0.15)), '10 veh/day')
  assert.equal(heardText('road', heard(0.01, 0.0, 0.0)), '1 veh/week')
  assert.equal(heardText('road', heard(0.003, 0.0, 0.0)), '1 veh/month')
})

test('trains read by the day; a steady source says nothing', () => {
  assert.equal(heardText('railway', heard(4, 2, 0.5)), '60 trains/day')
  assert.equal(heardText('building', undefined), null)
})

test('church bells and calls to prayer read as how many times they sound', () => {
  // A German church: prayer ringing twice by day and the Sunday peal, the quarters struck by day
  // and evening, the morning ringing before 07 h.
  assert.equal(heardText('building', heard(50.14 / 12, 16 / 4, 1 / 8)), '67× a day')
  assert.equal(heardText('building', heard(2.14 / 12, 0, 1 / 8)), '3× a day')
  // Istanbul's calls: Fajr at night, two to three by day, the rest in the evening, the sala twice a week.
  assert.equal(heardText('building', heard((2.42 + 1 / 7) / 12, (1.58 + 1 / 7) / 4, 1 / 8)), '5× a day')
})

test('rarer events read by the week or the month', () => {
  // An Orthodox church rings before Saturday's and Sunday's services; a European mosque's Friday call.
  assert.equal(heardText('building', heard(2 / 7 / 12, 0, 0)), '2× a week')
  assert.equal(heardText('building', heard(1 / 7 / 12, 0, 0)), '1× a week')
  assert.equal(heardText('railway', heard(0.1 / 12, 0, 0)), '3 trains/month')
})
