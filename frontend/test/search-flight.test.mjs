// A search's result or a place of the history opens the popup only when the flight really arrived.
import assert from 'node:assert/strict'
import test from 'node:test'

import { searchFlightArrived, SEARCH_FLIGHT_ZOOM } from '../src/lib/search-flight.ts'

const target = { lat: 50.0755, lon: 14.4378, zoom: SEARCH_FLIGHT_ZOOM }

test('only a moveend at the flight target counts as arrival — an interrupted flight does not', () => {
  // Landed (float error from project/unproject round trips).
  assert.equal(searchFlightArrived({ lat: 50.0755 + 1e-9, lng: 14.4378 - 1e-9, zoom: SEARCH_FLIGHT_ZOOM }, target), true)
  // Interrupted mid-flight by a hash jump: the map sits elsewhere, still zooming.
  assert.equal(searchFlightArrived({ lat: 50.02, lng: 14.40, zoom: 11.7 }, target), false)
  // Landed across the antimeridian, in the next world copy.
  assert.equal(searchFlightArrived({ lat: -16.5, lng: -179 + 1e-9, zoom: SEARCH_FLIGHT_ZOOM }, { lat: -16.5, lon: 181, zoom: SEARCH_FLIGHT_ZOOM }), true)
  // Interrupted at the target's coordinates but before the zoom settled.
  assert.equal(searchFlightArrived({ lat: 50.0755, lng: 14.4378, zoom: 13.4 }, target), false)
})
