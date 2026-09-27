/** Footprint ownership, registry authority and duplicate-site election. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { containsFacility, containingPolygonBeats, contestBeats, footprintAcceptsRegistryClass, lookupRadiusM, naceBaseLw,
  quietGateBlocks, overlapPairs, overlapsSameSite, type MatchFacility, type MatchPolygon, type OverlapWinner } from './facility-match.js'

const overlapLosers = (rows: OverlapWinner[]) => new Set(overlapPairs(rows).map(([, loser]) => loser))
const fac = (over: Partial<MatchFacility> = {}): MatchFacility =>
  ({ lat: 50, lon: 14, nace4: 1011, id: 310, rank: 5, year: 2022, ...over })
const mLat = (m: number) => m / 111_195
const poly = (over: Partial<MatchPolygon> = {}): MatchPolygon =>
  ({ lat: 50, lon: 14, areaM2: 10_000, subtype: 0,
    ring: [[13.999, 49.999], [14.001, 49.999], [14.001, 50.001], [13.999, 50.001]], ...over })

test('registry identity uses the actual footprint, never a neighbouring equivalent circle', () => {
  // A long narrow site has a large equivalent circle but does not own a point beside it.
  const longSite = poly({ areaM2: 500_000,
    ring: [[13.999, 49.98], [14.001, 49.98], [14.001, 50.02], [13.999, 50.02]] })
  assert.equal(containsFacility(fac({ lon: 14.002 }), longSite), false)
  assert.equal(containsFacility(fac({ lat: 50.019 }), longSite), true)
  assert.ok(lookupRadiusM(longSite) > 2000, 'an actual footprint is not clipped by a proximity horizon')
  const tenant = poly({ areaM2: 1000 })
  assert.ok(containingPolygonBeats(fac(), tenant, longSite))
  assert.ok(containingPolygonBeats(fac(), poly({ sourceType: 13 }), poly()))
  assert.equal(containingPolygonBeats(fac(), poly(), poly({ sourceType: 13 })), false)
  const seam = poly({ lon: 179.999,
    ring: [[179.998, 49.999], [180.002, 49.999], [180.002, 50.001], [179.998, 50.001]] })
  assert.ok(containsFacility(fac({ lon: -179.999 }), seam))
})

test('mapped source identity constrains registry activity even at a contained address', () => {
  for (const [subtype, allowed, blocked] of [[10, 146, 1011], [3, 810, 2410], [4, 2011, 2410],
    [5, 2351, 2410], [6, 2410, 3511], [1, 5210, 3511]]) {
    assert.ok(footprintAcceptsRegistryClass(fac({ nace4: allowed }), poly({ subtype })))
    assert.equal(footprintAcceptsRegistryClass(fac({ nace4: blocked }), poly({ subtype })), false)
  }
  assert.ok(quietGateBlocks(11, 3511))
  assert.ok(footprintAcceptsRegistryClass(fac({ nace4: 3700 }), poly({ sourceType: 4 })))
  assert.equal(footprintAcceptsRegistryClass(fac({ nace4: 3800 }), poly({ sourceType: 4 })), false)
  for (const sourceType of [10, 11, 12, 13, 14, 15]) {
    assert.equal(footprintAcceptsRegistryClass(fac(), poly({ sourceType })), false)
  }
})

test('registry contests retain authority, contained coal identity, and the loudest contained sector', () => {
  const eprtr = { rank: 5, year: 2022, id: 310, nace4: 812 }
  const gppd = { rank: 4, year: 2021, id: 300, nace4: 3511 }
  const coal = { rank: 4, year: 2025, id: 333, nace4: 510 }
  assert.ok(contestBeats(eprtr, gppd))
  assert.ok(contestBeats(coal, eprtr))
  const steel = { ...eprtr, nace4: 2410 }, chemicals = { ...eprtr, nace4: 2011 }
  assert.ok(contestBeats(steel, coal))
  assert.ok(contestBeats(steel, chemicals))
  assert.equal(contestBeats(chemicals, steel), false)
  assert.equal(naceBaseLw(2410), 106.4)
  assert.equal(naceBaseLw(3512), 95.6)
  assert.equal(naceBaseLw(3599), 80.4)
  assert.equal(naceBaseLw(9999), -1)
})

// ── I-07 dual-registry overlap dedup (Wave 2 B) ──────────────────────────────
const win = (over: Partial<OverlapWinner> = {}): OverlapWinner => {
  const row = { key: 'k', lat: 49.18, lon: 14.376, areaM2: 1_200_000, rank: 5, year: 2022, id: 310, ...over }
  const dy = mLat(Math.sqrt(row.areaM2) / 2), dx = dy / Math.cos(row.lat * Math.PI / 180)
  return { ...row, ring: over.ring ?? [[row.lon - dx, row.lat - dy], [row.lon + dx, row.lat - dy],
    [row.lon + dx, row.lat + dy], [row.lon - dx, row.lat + dy]] }
}

test('disjoint parallel industrial strips cannot suppress one another as duplicate sites', () => {
  const a = win({ key: 'A', lat: 50, lon: 14, areaM2: 150_000,
    ring: [[13.9998, 49.98], [14.0002, 49.98], [14.0002, 50.02], [13.9998, 50.02]] })
  const b = win({ key: 'B', lat: 50, lon: 14.001, areaM2: 100_000,
    ring: [[14.0008, 49.98], [14.0012, 49.98], [14.0012, 50.02], [14.0008, 50.02]] })
  assert.equal(overlapsSameSite(a, b), false)
  assert.deepEqual(overlapPairs([a, b]), [])
})

test('I-07: two coincident different-registry polygons → the lower-provenance row is suppressed', () => {
  const eprtr = win({ key: 'A', areaM2: 1_231_457, rank: 5, id: 310 })              // Temelín E-PRTR 123 ha
  const gppd = win({ key: 'B', areaM2: 1_433_333, rank: 4, id: 300, lat: 49.18 + mLat(30) }) // GPPD 143 ha, ~30 m off
  assert.deepEqual([...overlapLosers([eprtr, gppd])], ['B'], 'E-PRTR (rank 5) survives, GPPD (rank 4) suppressed')
})

test('I-07: adjacent DISTINCT plants (centroids far apart) are never merged', () => {
  const a = win({ key: 'A', areaM2: 500_000 })                                        // r≈399 m → 0.5r≈199 m threshold
  const b = win({ key: 'B', areaM2: 500_000, rank: 4, id: 300, lat: 49.18 + mLat(700) }) // 700 m apart
  assert.equal(overlapLosers([a, b]).size, 0)
})

test('I-07: small coincident sites below the 10 ha floor are left alone', () => {
  const a = win({ key: 'A', areaM2: 50_000 })
  const b = win({ key: 'B', areaM2: 55_000, rank: 4, id: 300, lat: 49.18 + mLat(10) })
  assert.equal(overlapLosers([a, b]).size, 0)
})

test('I-07: a big zone and a small nested tenant (area ratio too large) are not merged', () => {
  const zone = win({ key: 'Z', areaM2: 1_000_000 })
  const tenant = win({ key: 'T', areaM2: 100_000, rank: 4, id: 300, lat: 49.18 + mLat(20) }) // 10× ratio
  assert.equal(overlapLosers([zone, tenant]).size, 0)
})

test('I-07: only mutual-best pairs collapse; an unrelated third plant is untouched (no transitive merge)', () => {
  const a = win({ key: 'A', areaM2: 1_200_000, rank: 5, id: 310 })
  const b = win({ key: 'B', areaM2: 1_300_000, rank: 4, id: 300, lat: 49.18 + mLat(40) })
  const c = win({ key: 'C', areaM2: 1_200_000, rank: 5, id: 310, lat: 55.0, lon: 10.0 }) // far away
  assert.deepEqual([...overlapLosers([a, b, c])].sort(), ['B'], 'A/B collapse, C untouched')
})

test('overlapsSameSite: coincident + similar-size + sizable = true; far / below-floor = false', () => {
  const base = win({ areaM2: 1_200_000 })
  assert.ok(overlapsSameSite(base, win({ areaM2: 1_300_000, lat: 49.18 + mLat(30) })))
  assert.ok(!overlapsSameSite(base, win({ areaM2: 1_300_000, lat: 49.18 + mLat(2000) })), 'far centroid')
  assert.ok(!overlapsSameSite(win({ areaM2: 50_000 }), win({ areaM2: 55_000 })), 'below area floor')
})

test('I-07 metric neighborhood retains the same pair rule across high-latitude and dateline cells', () => {
  for (const [lat, firstLon, secondLon] of [[80, 10, 10.045], [50, 179.999, -179.999]]) {
    const a = win({ key: 'A', lat, lon: firstLon, areaM2: 10_000_000 })
    const b = win({ key: 'B', lat, lon: secondLon, areaM2: 10_000_000, rank: 4, id: 300 })
    assert.ok(overlapsSameSite(a, b), 'the original exact geometric predicate admits the pair')
    assert.deepEqual([...overlapLosers([a, b])], ['B'])
  }
})
