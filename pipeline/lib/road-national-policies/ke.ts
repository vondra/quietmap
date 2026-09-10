/** KE OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const KE_SCAN_BBOX: [number, number, number, number] = [-4.7, 33.9, 5.5, 41.9]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — wide boxes cover the metro + immediate satellite sprawl.
  { name: 'Nairobi', lat: -1.286, lon: 36.817, tier: 1, half: 0.30 },
  { name: 'Mombasa', lat: -4.043, lon: 39.668, tier: 1, half: 0.18 },
  // Tier 2 ×1.4 — county capitals + regional centres.
  { name: 'Nakuru',   lat: -0.303, lon: 36.080, tier: 2, half: 0.10 },
  { name: 'Eldoret',  lat:  0.514, lon: 35.270, tier: 2, half: 0.10 },
  { name: 'Kisumu',   lat: -0.092, lon: 34.768, tier: 2, half: 0.10 },
  { name: 'Thika',    lat: -1.033, lon: 37.069, tier: 2, half: 0.08 },
  { name: 'Ruiru',    lat: -1.146, lon: 36.961, tier: 2, half: 0.06 },
  { name: 'Kiambu',   lat: -1.171, lon: 36.835, tier: 2, half: 0.06 },
  { name: 'Machakos', lat: -1.519, lon: 37.265, tier: 2, half: 0.08 },
  { name: 'Naivasha', lat: -0.717, lon: 36.431, tier: 2, half: 0.08 },
  { name: 'Kitale',   lat:  1.015, lon: 35.006, tier: 2, half: 0.08 },
  { name: 'Malindi',  lat: -3.219, lon: 40.117, tier: 2, half: 0.08 },
  { name: 'Kakamega', lat:  0.282, lon: 34.752, tier: 2, half: 0.08 },
  { name: 'Kisii',    lat: -0.681, lon: 34.767, tier: 2, half: 0.08 },
  { name: 'Embu',     lat: -0.539, lon: 37.457, tier: 2, half: 0.08 },
  { name: 'Meru',     lat:  0.047, lon: 37.649, tier: 2, half: 0.08 },
  { name: 'Nyeri',    lat: -0.420, lon: 36.947, tier: 2, half: 0.08 },
  { name: 'Garissa',  lat: -0.453, lon: 39.646, tier: 2, half: 0.08 },
  { name: 'Lamu',     lat: -2.272, lon: 40.902, tier: 2, half: 0.06 },
  { name: 'Kilifi',   lat: -3.630, lon: 39.849, tier: 2, half: 0.08 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Mombasa↔Nairobi container corridor [lon,lat] — high HGV share (~35%) ──
// A10 / Mombasa Road, parallel to SGR Phase 1: port freight from Mombasa to the
// Embakasi ICD and on toward Uganda (Northern Corridor). ~480 km of empty Tsavo
// between the two metros runs heavy with container trucks.
const CONTAINER_CORRIDOR: [number, number][] = [
  [39.668, -4.043], [38.556, -3.396], [38.169, -2.690], [37.825, -2.281],
  [37.461, -2.103], [36.980, -1.450], [36.900, -1.340], [36.817, -1.286],
]

function nearContainerCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, CONTAINER_CORRIDOR) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.50, medium: 0.15, heavy: 0.10, moto: 0.25 },
  tier2:    { light: 0.52, medium: 0.15, heavy: 0.10, moto: 0.23 },
  corridor: { light: 0.45, medium: 0.08, heavy: 0.35, moto: 0.12 },
  rural:    { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
}

/** Region for the vehicle split: cities first (urban matatu/boda mix dominates the
 *  metro surface streets), then the container corridor on its long rural stretches,
 *  then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearContainerCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link (Nairobi Expressway only)
  1: 10000, 11: 5000,  // trunk + trunk_link (Class A/B)
  2: 5000,  12: 2500,  // primary + primary_link
  3: 2500,             // secondary
  4: 1200,             // tertiary
  5: 600,              // residential
}
const KE_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

function traffic(row: Parameters<NationalRoadPolicy['traffic']>[0]): PolicyRoadTraffic | null {
  const base = BASE_AADT[row.roadClass]
  if (base === undefined) return null
  const tier = cityTier(row.midLat, row.midLon)
    return splitVehicles(base * (tier === 1 ? 2 : tier === 2 ? 1.4 : 1), SPLITS[region(row.midLat, row.midLon, tier)])
}

export const policy: NationalRoadPolicy = {
  country: 'KE',
  bbox: KE_SCAN_BBOX,
  coverage: KE_COVERAGE,
  traffic,
}
