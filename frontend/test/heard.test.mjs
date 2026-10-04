// How a source is heard, in words: steady, so many an hour, one every so many minutes, so many a
// day; trains and airport movements by the day; steady sources say nothing.
import assert from 'node:assert/strict'
import test from 'node:test'

import { heardText } from '../src/components/noise/heard.ts'

const heard = (day, evening, night, steady = false) => ({ per_hour: { day, evening, night }, steady })

test('a road reads as a steady hum, vehicles an hour, one every so many minutes or a day', () => {
  assert.equal(heardText('road', heard(916, 845, 317, true)), 'steady hum')
  assert.equal(heardText('road', heard(76, 59, 20)), '76 vehicles an hour')
  assert.equal(heardText('road', heard(3.78, 2.92, 0.97)), 'a vehicle every 16 min')
  assert.equal(heardText('road', heard(0.59, 0.46, 0.15)), '10 vehicles a day')
  assert.equal(heardText('road', heard(0.01, 0.0, 0.0)), 'a vehicle every 8 days')
})

test('trains and airport movements read by the day; a steady source says nothing', () => {
  assert.equal(heardText('railway', heard(4, 2, 0.5)), '60 trains a day')
  assert.equal(heardText('aircraft', heard(20, 15, 2)), '316 movements a day')
  assert.equal(heardText('building', undefined), null)
})

test('church bells read as how often they ring', () => {
  // A German church: prayer ringing twice by day and the Sunday peal, the quarters struck by day
  // and evening, the morning ringing before 07 h.
  assert.equal(heardText('building', heard(50.14 / 12, 16 / 4, 1 / 8)), 'rings 67 times a day')
  assert.equal(heardText('building', heard(2.14 / 12, 0, 1 / 8)), 'rings 3 times a day')
})
