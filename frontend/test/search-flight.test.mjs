import assert from 'node:assert/strict'
import test from 'node:test'

import { searchFlightArrived, SEARCH_FLIGHT_ZOOM } from '../src/lib/search-flight.ts'

const target = { lat: 50.0755, lon: 14.4378 }

test('only a moveend at the flight target counts as arrival — an interrupted flight does not', () => {
  // Landed (float error from project/unproject round trips).
  assert.equal(searchFlightArrived({ lat: 50.0755 + 1e-9, lng: 14.4378 - 1e-9, zoom: SEARCH_FLIGHT_ZOOM }, target), true)
  // Interrupted mid-flight by a hash jump: the map sits elsewhere, still zooming.
  assert.equal(searchFlightArrived({ lat: 50.02, lng: 14.40, zoom: 11.7 }, target), false)
  // Interrupted at the target's coordinates but before the zoom settled.
  assert.equal(searchFlightArrived({ lat: 50.0755, lng: 14.4378, zoom: 13.4 }, target), false)
})
