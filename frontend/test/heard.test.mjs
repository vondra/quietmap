// Why a source is loud, short enough for a column: steady, or a road's vehicles an hour by day, a quiet
// one's too; trains, flights and events a day, as the flights table counts; steady sources say nothing.
import assert from 'node:assert/strict'
import test from 'node:test'

import { flightsText, heardText } from '../src/components/noise/heard.ts'

const heard = (day, evening, night, steady = false) => ({ per_hour: { day, evening, night }, steady })

test('a road reads as its vehicles an hour by day, a steady or a quiet one too', () => {
  // La Palma's lanes listed "19 veh/h" over "14 veh/day", a motorway "steady" (owner 2026-10-13):
  // one figure, one unit down the column.
  assert.equal(heardText('road', heard(916, 845, 317, true)), '916 veh/h')
  assert.equal(heardText('road', heard(76, 59, 20)), '76 veh/h')
  assert.equal(heardText('road', heard(3.78, 2.92, 0.97)), '3.8 veh/h')
  assert.equal(heardText('road', heard(0.59, 0.46, 0.15)), '0.6 veh/h')
  assert.equal(heardText('road', heard(0.01, 0.0, 0.0)), '0.01 veh/h')
  assert.equal(heardText('road', heard(0.003, 0.0, 0.0)), '0.003 veh/h')
})

test('a road\'s figure is both directions: its cross-section\'s day traffic', () => {
  // K Letišti at Ruzyně: each carriageway passes 214 an hour by day, the road 6,421 a day, 79.9 % by day.
  assert.equal(heardText('road', heard(213.8, 83.5, 38.5), { cross_section_aadt: 6421, period_shares: [0.799, 0.116, 0.085] }), '428 veh/h')
  assert.equal(heardText('road', undefined, { cross_section_aadt: 6421, period_shares: [0.799, 0.116, 0.085] }), '428 veh/h')
})

test('trains read by the day; a steady source says nothing', () => {
  assert.equal(heardText('railway', heard(4, 2, 0.5)), '60 trains/day')
  assert.equal(heardText('building', undefined), null)
})

test('church bells and calls to prayer read as how many times they sound', () => {
  // A German church: prayer ringing twice by day and the Sunday peal, the quarters struck by day
  // and evening, the morning ringing before 07 h.
  assert.equal(heardText('building', heard(50.14 / 12, 16 / 4, 1 / 8)), '67× a day')
  assert.equal(heardText('building', heard(2.14 / 12, 0, 1 / 8)), '3.1× a day')
  // Istanbul's calls: Fajr at night, two to three by day, the rest in the evening, the sala twice a week.
  assert.equal(heardText('building', heard((2.42 + 1 / 7) / 12, (1.58 + 1 / 7) / 4, 1 / 8)), '5.3× a day')
})

test('rarer events and trains read a day too, below one in a tenth or less', () => {
  // An Orthodox church rings before Saturday's and Sunday's services; a European mosque's Friday call.
  assert.equal(heardText('building', heard(2 / 7 / 12, 0, 0)), '0.3× a day')
  assert.equal(heardText('building', heard(1 / 7 / 12, 0, 0)), '0.1× a day')
  assert.equal(heardText('railway', heard(0.1 / 12, 0, 0)), '0.1 trains/day')
})

test('the aircraft layer reads as its flights a day above 50 dB, never a year; none without them', () => {
  const events = (perDay) => ({ above_db: [50, 60, 70], per_day: perDay, night: [0, 0, 0], height_m: [null, null, null], type: [null, null, null], helicopters_per_day: 0 })
  assert.equal(flightsText(events([1143, 300, 20])), '1,143 flights/day')
  assert.equal(flightsText(events([34.2, 3, 0])), '34 flights/day')
  // Meloneras, Gran Canaria: 538 a year above 50 dB.
  assert.equal(flightsText(events([1.472, 0.102, 0.048])), '1.5 flights/day')
  assert.equal(flightsText(events([0.3, 0, 0])), '0.3 flights/day')
  // One flight a year.
  assert.equal(flightsText(events([0.0027, 0, 0])), '0.003 flights/day')
  assert.equal(flightsText(events([0.0005, 0, 0])), '<0.001 flights/day')
  assert.equal(flightsText(events([0, 0, 0])), null)
  assert.equal(flightsText(undefined), null)
})
