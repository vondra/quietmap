/** TZ OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const TZ_SCAN_BBOX: [number, number, number, number] = [-11.8, 29.3, -0.9, 40.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — wide box covers the Dar metro + immediate satellite sprawl.
  { name: 'Dar es Salaam', lat: -6.82, lon: 39.28, tier: 1, half: 0.30 },
  // Tier 2 ×1.4 — regional capitals + major centres (NBS 2022 census).
  { name: 'Dodoma',        lat: -6.173, lon: 35.742, tier: 2, half: 0.12 },
  { name: 'Mwanza',        lat: -2.517, lon: 32.900, tier: 2, half: 0.10 },
  { name: 'Arusha',        lat: -3.387, lon: 36.683, tier: 2, half: 0.10 },
  { name: 'Mbeya',         lat: -8.909, lon: 33.460, tier: 2, half: 0.10 },
  { name: 'Morogoro',      lat: -6.821, lon: 37.661, tier: 2, half: 0.08 },
  { name: 'Tanga',         lat: -5.069, lon: 39.099, tier: 2, half: 0.08 },
  { name: 'Kahama',        lat: -3.838, lon: 32.600, tier: 2, half: 0.07 },
  { name: 'Tabora',        lat: -5.017, lon: 32.800, tier: 2, half: 0.08 },
  { name: 'Zanzibar City', lat: -6.165, lon: 39.199, tier: 2, half: 0.07 },
  { name: 'Kigoma',        lat: -4.877, lon: 29.626, tier: 2, half: 0.07 },
  { name: 'Sumbawanga',    lat: -7.967, lon: 31.617, tier: 2, half: 0.07 },
  { name: 'Kasulu',        lat: -4.578, lon: 30.103, tier: 2, half: 0.06 },
  { name: 'Songea',        lat: -10.683, lon: 35.650, tier: 2, half: 0.07 },
  { name: 'Musoma',        lat: -1.500, lon: 33.800, tier: 2, half: 0.07 },
  { name: 'Iringa',        lat: -7.770, lon: 35.691, tier: 2, half: 0.07 },
  { name: 'Singida',       lat: -4.817, lon: 34.745, tier: 2, half: 0.07 },
  { name: 'Shinyanga',     lat: -3.661, lon: 33.421, tier: 2, half: 0.07 },
  { name: 'Moshi',         lat: -3.350, lon: 37.333, tier: 2, half: 0.07 },
  { name: 'Bukoba',        lat: -1.332, lon: 31.812, tier: 2, half: 0.07 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── TANZAM Highway corridor (T5) [lon,lat] — high HGV share (~40%) ──
// Dar es Salaam ↔ Chalinze ↔ Morogoro ↔ Mikumi ↔ Iringa ↔ Mbeya ↔ Tunduma (Zambia
// border), parallel to the TAZARA railway. Carries Zambian/DRC Copperbelt copper &
// cobalt to Dar es Salaam port and import freight inland — East Africa's main
// southern trucking spine. ~900 km, much of it empty highland between towns.
const TANZAM_CORRIDOR: [number, number][] = [
  [39.28, -6.82],  // Dar es Salaam
  [38.35, -6.64],  // Chalinze
  [37.661, -6.821], // Morogoro
  [37.00, -7.40],  // Mikumi
  [35.691, -7.770], // Iringa
  [34.55, -8.30],  // Makambako
  [33.460, -8.909], // Mbeya
  [32.77, -9.30],  // Tunduma (Zambia border)
]

function nearTanzamCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, TANZAM_CORRIDOR) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.50, medium: 0.15, heavy: 0.10, moto: 0.25 },
  tier2:    { light: 0.52, medium: 0.13, heavy: 0.12, moto: 0.23 },
  corridor: { light: 0.42, medium: 0.08, heavy: 0.40, moto: 0.10 },
  rural:    { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
}

/** Region for the vehicle split: cities first (urban daladala/boda mix dominates
 *  the metro surface streets), then the TANZAM corridor on its long rural stretches,
 *  then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearTanzamCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 25000, 10: 12500, // motorway + motorway_link (rare: Dar expressways)
  1: 10000, 11: 5000,  // trunk + trunk_link (T-routes)
  2: 5000,  12: 2500,  // primary + primary_link
  3: 2500,             // secondary
  4: 1200,             // tertiary
  5: 600,              // residential
}
const TZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
  country: 'TZ',
  bbox: TZ_SCAN_BBOX,
  coverage: TZ_COVERAGE,
  traffic,
}
