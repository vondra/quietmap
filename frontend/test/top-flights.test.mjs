// A loudest flight's cells: level, kilometres, the UTC start day with the period letter, the type in
// words, and the link to the trace of the day the flight started.
import assert from 'node:assert/strict'
import test from 'node:test'

import { topFlightCells } from '../src/components/noise/top-flights.ts'

// A visitor east of Greenwich, where 23:58 UTC is already the next day.
process.env.TZ = 'Europe/Prague'

const dayFlight = {
  icao: '4b0a1c', callsign: 'CSA123', type: 'A320', start_unix: Date.UTC(2025, 8, 2, 14, 26, 40) / 1000,
  period: 'day', sel_db: 79.1, lmax_db: 70.2, closest_m: 444, altitude_m: 255,
}

test('a flight reads as its level, kilometres, UTC start day and period, type in words, and trace link', () => {
  assert.deepEqual(topFlightCells(dayFlight), {
    lmax: '70',
    closestKm: '0.44',
    altitudeKm: '0.26',
    date: '09-02 D',
    period: 0,
    startUtc: '2025-09-02 14:26:40 UTC',
    aircraft: 'Airbus A320',
    aircraftTitle: 'Type: Airbus A320 (A320)\nCallsign: CSA123\nICAO address: 4B0A1C\n\n' +
      'Opens the flight\'s trace of 2025-09-02 on adsb.lol.',
    href: 'https://adsb.lol/?icao=4b0a1c&showTrace=2025-09-02',
  })
})

test('the date and the trace link are the UTC day of the start, never the visitor\'s local day', () => {
  const cells = topFlightCells({ ...dayFlight, start_unix: Date.UTC(2025, 8, 1, 23, 58, 20) / 1000, period: 'night' })
  assert.equal(cells.date, '09-01 N')
  assert.equal(cells.period, 2)
  assert.equal(cells.startUtc, '2025-09-01 23:58:20 UTC')
  assert.equal(cells.href, 'https://adsb.lol/?icao=4b0a1c&showTrace=2025-09-01')
})

test('a type without a name shows its designator; a flight without type or callsign says so', () => {
  const unnamed = topFlightCells({ ...dayFlight, type: 'ZZ9A', period: 'evening' })
  assert.equal(unnamed.aircraft, 'ZZ9A')
  assert.match(unnamed.aircraftTitle, /^Type: ZZ9A\n/)
  assert.equal(unnamed.date, '09-02 E')
  const anonymous = topFlightCells({ ...dayFlight, type: '', callsign: '' })
  assert.equal(anonymous.aircraft, 'unknown type')
  assert.match(anonymous.aircraftTitle, /^Type: unknown\nCallsign: —\n/)
  // An inherited property name is no type name.
  assert.equal(topFlightCells({ ...dayFlight, type: 'toString' }).aircraft, 'toString')
})

test('kilometres round to 10 m, and a point a few metres below the receiver reads 0.00', () => {
  const cells = topFlightCells({ ...dayFlight, closest_m: 11_895, altitude_m: -4 })
  assert.equal(cells.closestKm, '11.90')
  assert.equal(cells.altitudeKm, '0.00')
})

test('a period the popup has no label for is marked, not guessed', () => {
  const cells = topFlightCells({ ...dayFlight, period: '' })
  assert.equal(cells.date, '09-02 ?')
  assert.equal(cells.period, -1)
})
