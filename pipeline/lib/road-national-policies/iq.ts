/** IQ OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const IQ_SCAN_BBOX: [number, number, number, number] = [29.0, 38.7, 37.4, 48.6]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2 | 3; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Greater Baghdad (~8 M; box wide enough for the metro + Tigris
  // suburbs out to Abu Ghraib / Mada'in).
  { name: 'Baghdad', lat: 33.31, lon: 44.36, tier: 1, half: 0.25 },
  // Tier 2 ×1.6 — oil capital + KRG twin cities.
  { name: 'Basra',        lat: 30.51, lon: 47.78, tier: 2, half: 0.14 },
  { name: 'Erbil',        lat: 36.19, lon: 44.01, tier: 2, half: 0.13 },
  { name: 'Sulaymaniyah', lat: 35.56, lon: 45.43, tier: 2, half: 0.11 },
  // Tier 3 ×1.3 — 14 provincial capitals.
  { name: 'Mosul',      lat: 36.34, lon: 43.13, tier: 3, half: 0.13 },
  { name: 'Najaf',      lat: 32.00, lon: 44.33, tier: 3, half: 0.09 },
  { name: 'Karbala',    lat: 32.61, lon: 44.02, tier: 3, half: 0.09 },
  { name: 'Kirkuk',     lat: 35.47, lon: 44.39, tier: 3, half: 0.10 },
  { name: 'Nasiriyah',  lat: 31.05, lon: 46.26, tier: 3, half: 0.08 },
  { name: 'Hillah',     lat: 32.47, lon: 44.43, tier: 3, half: 0.08 },
  { name: 'Diwaniyah',  lat: 31.99, lon: 44.93, tier: 3, half: 0.08 },
  { name: 'Kut',        lat: 32.51, lon: 45.82, tier: 3, half: 0.08 },
  { name: 'Tikrit',     lat: 34.61, lon: 43.68, tier: 3, half: 0.07 },
  { name: 'Samarra',    lat: 34.20, lon: 43.87, tier: 3, half: 0.07 },
  { name: 'Fallujah',   lat: 33.35, lon: 43.79, tier: 3, half: 0.08 },
  { name: 'Ramadi',     lat: 33.42, lon: 43.31, tier: 3, half: 0.08 },
  { name: 'Amarah',     lat: 31.84, lon: 47.15, tier: 3, half: 0.08 },
  { name: 'Duhok',      lat: 36.87, lon: 42.99, tier: 3, half: 0.08 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 | 3 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Basra oil corridor [lon,lat] — ~58% heavy ──
// Crude, rig kit and reconstruction freight move between the southern super-giant
// fields (West Qurna ~31.0N, Rumaila/Zubair ~30.1-30.4N) and Basra → the Faw
// export terminals. The whole southern oil province is heavy-truck dominated.
const BASRA_OIL: [number, number][] = [
  [47.43, 31.00], // West Qurna
  [47.25, 30.20], // Rumaila
  [47.55, 30.40], // Zubair
  [47.83, 30.51], // Basra
  [48.30, 29.97], // toward Faw / Khor al-Zubair port
]

function nearBasraOil(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, BASRA_OIL) <= 25000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'rural' | 'basra'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1: { light: 0.60, medium: 0.10, heavy: 0.22, moto: 0.08 },
  tier2: { light: 0.58, medium: 0.08, heavy: 0.26, moto: 0.08 },
  rural: { light: 0.50, medium: 0.05, heavy: 0.38, moto: 0.07 },
  basra: { light: 0.35, medium: 0.03, heavy: 0.58, moto: 0.04 },
}

/** Region for the vehicle split. Cities first (the urban mix dominates even where
 *  a freight road passes through, e.g. Basra city centre), then the Basra oil
 *  haul, else rural highway. */
function region(lat: number, lon: number, tier: 0 | 1 | 2 | 3): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2 || tier === 3) return 'tier2'
  if (nearBasraOil(lat, lon)) return 'basra'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 40000, 10: 20000, // motorway + motorway_link
  1: 18000, 11: 9000,  // trunk + trunk_link
  2: 9000,  12: 4500,  // primary + primary_link
  3: 4500,             // secondary
  4: 2000,             // tertiary
  5: 800,              // residential
}
const IQ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
    return splitVehicles(base * (tier === 1 ? 2.5 : tier === 2 ? 1.6 : tier === 3 ? 1.3 : 1), SPLITS[region(row.midLat, row.midLon, tier)])
}

export const policy: NationalRoadPolicy = {
  country: 'IQ',
  bbox: IQ_SCAN_BBOX,
  coverage: IQ_COVERAGE,
  traffic,
}
