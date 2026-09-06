/**
 * Trip-rate table contract: exact parity for the arms the 2026-07 trips
 * refactor did NOT change, explicit golden values for the arms it DID
 * (splitting the two was a /gg consensus finding — a blanket "parity with
 * estimateDwellings × 3.68" test is mathematically unsatisfiable once rates
 * were corrected).
 *
 * Run: `npx tsx --test pipeline/lib/trip-rates.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { estimateBuildingLoad } from './trip-rates.js'

/** The pre-refactor estimateDwellings, verbatim semantics (dwelling-equivalents;
 *  callers multiplied by TRIPS_PER_DWELLING = 3.68). Golden reference for the
 *  parity and asymptotic-parity assertions below. */
function legacyDwellings(buildingType: number, floors: number, areaM2: number | null): number {
  const footprint = areaM2 ?? 100
  const effectiveFloors = floors > 0 ? floors : 1
  const gfa = footprint * effectiveFloors
  switch (buildingType) {
    case 1: return Math.min(Math.ceil(gfa / 92), 400)
    case 2: return Math.min(Math.ceil(gfa / 686), 200)
    case 3: return Math.min(Math.ceil(gfa / 800), 100)
    case 4: return Math.min(Math.ceil(gfa / 11), 300)
    case 5: return 2
    case 6: return Math.min(Math.ceil(gfa / 38), 400)
    case 7: return 1
    case 8: return Math.min(Math.ceil(gfa / 200), 50)
    case 9: return Math.min(Math.ceil(gfa / 300), 100)
    default: return Math.min(Math.max(1, Math.floor(gfa / 80)), 200)
  }
}
const LEGACY_TRIPS_PER_DWELLING = 3.68

// ─── Exact parity: residential dwellings arm (0) ────────────────────────────

test('apartments (0) preserve legacy integer-dwellings semantics exactly', () => {
  for (const [floors, area] of [[1, 100], [2, 300], [0, null], [5, 4000], [1, 30]] as const) {
    const load = estimateBuildingLoad(0, floors, area)
    assert.equal(load.dwellings, legacyDwellings(0, floors, area))
    assert.equal(load.trips, 0)
  }
})

test('unknown building_type falls to the residential arm like the old default', () => {
  const load = estimateBuildingLoad(99, 1, 240)
  assert.equal(load.dwellings, legacyDwellings(99, 1, 240))
})

// ─── Exact parity: fixed fixtures (5 church, 7 garage) ──────────────────────

test('church and garage restate the legacy fixed dwellings in trips', () => {
  assert.equal(estimateBuildingLoad(5, 1, 500).trips, 2 * LEGACY_TRIPS_PER_DWELLING)
  assert.equal(estimateBuildingLoad(7, 1, 40).trips, 1 * LEGACY_TRIPS_PER_DWELLING)
})

// ─── Asymptotic parity: restated continuous arms (3, 4, 6, 8, 9) ────────────
// Continuous rates replace the old per-building ceil() staircase; at sizes an
// order above the divisor the two agree within 5 %, and the trip caps equal
// the old dwelling caps × 3.68.

test('school/hospital/hotel/farm/civic rates restate the legacy divisors', () => {
  for (const [type, gfa] of [[3, 10_000], [4, 2_000], [6, 10_000], [8, 5_000], [9, 5_000]] as const) {
    const legacy = Math.min(legacyDwellings(type, 1, gfa), Infinity) * LEGACY_TRIPS_PER_DWELLING
    const now = estimateBuildingLoad(type, 1, gfa).trips
    assert.ok(
      Math.abs(now - legacy) / legacy < 0.05,
      `type ${type} at ${gfa} m²: ${now} vs legacy ${legacy}`,
    )
  }
  // Caps: legacy dwelling cap × 3.68 (hospital 300 dw → 1104 trips).
  assert.equal(estimateBuildingLoad(4, 10, 10_000).trips, 300 * LEGACY_TRIPS_PER_DWELLING)
})

// ─── Golden deltas: intentionally changed / new arms ────────────────────────

test('GOLDEN commercial (1): 12/100 m² GFA — was ~4 (divisor 10× under ITE 820)', () => {
  assert.equal(estimateBuildingLoad(1, 1, 1000).trips, 120)
  assert.equal(estimateBuildingLoad(1, 1, 30).trips, 5) // minTrips floor
  assert.equal(estimateBuildingLoad(1, 10, 100_000).trips, 3000) // cap
})

test('GOLDEN industrial building (2): 1.6/100 m² GFA — was ~0.54', () => {
  assert.equal(estimateBuildingLoad(2, 1, 10_000).trips, 160)
})

test('GOLDEN SILENT (10): sheds/roofs generate nothing — was 1+ phantom dwellings', () => {
  assert.deepEqual(estimateBuildingLoad(10, 1, 500), { dwellings: 0, trips: 0 })
  assert.deepEqual(estimateBuildingLoad(10, 0, null), { dwellings: 0, trips: 0 })
})

test('GOLDEN HOUSE (11): 1 dw / 120 m² GFA, cap 4 — was the 0-arm (80 m², cap 200)', () => {
  assert.equal(estimateBuildingLoad(11, 1, 100).dwellings, 1)
  assert.equal(estimateBuildingLoad(11, 2, 150).dwellings, 2) // gfa 300
  assert.equal(estimateBuildingLoad(11, 2, 900).dwellings, 4) // cap: terrace row
})

test('GOLDEN FOOD_RETAIL (12): 30/100 m² footprint, floors ignored', () => {
  assert.equal(estimateBuildingLoad(12, 1, 400).trips, 120)
  assert.equal(estimateBuildingLoad(12, 5, 400).trips, 120) // ground-floor basis
  assert.equal(estimateBuildingLoad(12, 1, 50_000).trips, 15_000)
  assert.equal(estimateBuildingLoad(12, 1, 10).trips, 20) // minTrips
})

test('GOLDEN HOSPITALITY (13): 30/100 m² footprint — the Krabi guesthouse fix', () => {
  // 200 m² restaurant: was billed as 2 dwellings ≈ 7.4 trips; now 60.
  assert.equal(estimateBuildingLoad(13, 1, 200).trips, 60)
  assert.equal(estimateBuildingLoad(13, 3, 200).trips, 60) // tower POI-join case
  assert.equal(estimateBuildingLoad(13, 1, 20_000).trips, 2000) // cap
})
