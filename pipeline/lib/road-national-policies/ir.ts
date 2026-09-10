/** IR OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const IR_SCAN_BBOX: [number, number, number, number] = [25.0, 44.0, 39.8, 63.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2 | 3; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Greater Tehran (box wide enough for the metro + near suburbs;
  // Karaj keeps its own Tier-2 box just to the west).
  { name: 'Tehran', lat: 35.70, lon: 51.42, tier: 1, half: 0.35 },
  // Tier 2 ×1.8 — top regional cities.
  { name: 'Mashhad', lat: 36.30, lon: 59.57, tier: 2, half: 0.18 },
  { name: 'Isfahan', lat: 32.65, lon: 51.67, tier: 2, half: 0.18 },
  { name: 'Karaj',   lat: 35.84, lon: 50.99, tier: 2, half: 0.13 },
  { name: 'Shiraz',  lat: 29.59, lon: 52.58, tier: 2, half: 0.15 },
  { name: 'Tabriz',  lat: 38.08, lon: 46.29, tier: 2, half: 0.15 },
  // Tier 3 ×1.4 — 20 provincial capitals.
  { name: 'Ahvaz',        lat: 31.32, lon: 48.67, tier: 3, half: 0.10 },
  { name: 'Qom',          lat: 34.64, lon: 50.88, tier: 3, half: 0.10 },
  { name: 'Kermanshah',   lat: 34.31, lon: 47.07, tier: 3, half: 0.10 },
  { name: 'Urmia',        lat: 37.55, lon: 45.07, tier: 3, half: 0.09 },
  { name: 'Rasht',        lat: 37.28, lon: 49.58, tier: 3, half: 0.09 },
  { name: 'Kerman',       lat: 30.28, lon: 57.08, tier: 3, half: 0.09 },
  { name: 'Hamadan',      lat: 34.80, lon: 48.51, tier: 3, half: 0.09 },
  { name: 'Arak',         lat: 34.09, lon: 49.69, tier: 3, half: 0.09 },
  { name: 'Yazd',         lat: 31.90, lon: 54.37, tier: 3, half: 0.09 },
  { name: 'Ardabil',      lat: 38.25, lon: 48.29, tier: 3, half: 0.09 },
  { name: 'Bandar Abbas', lat: 27.18, lon: 56.28, tier: 3, half: 0.09 },
  { name: 'Zahedan',      lat: 29.50, lon: 60.86, tier: 3, half: 0.09 },
  { name: 'Sanandaj',     lat: 35.31, lon: 46.99, tier: 3, half: 0.09 },
  { name: 'Zanjan',       lat: 36.68, lon: 48.49, tier: 3, half: 0.09 },
  { name: 'Gorgan',       lat: 36.84, lon: 54.44, tier: 3, half: 0.09 },
  { name: 'Bushehr',      lat: 28.92, lon: 50.84, tier: 3, half: 0.09 },
  { name: 'Khorramabad',  lat: 33.49, lon: 48.36, tier: 3, half: 0.09 },
  { name: 'Sari',         lat: 36.57, lon: 53.06, tier: 3, half: 0.09 },
  { name: 'Birjand',      lat: 32.87, lon: 59.22, tier: 3, half: 0.09 },
  { name: 'Bojnurd',      lat: 37.47, lon: 57.33, tier: 3, half: 0.09 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 | 3 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── South Pars freight corridor [lon,lat] — ~52% heavy ──
// Petrochemicals leave the Asaluyeh / Pars Special Economic Zone two ways: inland
// to Shiraz (then the national network) and along the Gulf coast toward Bushehr.
const SOUTH_PARS_INLAND: [number, number][] = [[52.61, 27.48], [52.06, 27.83], [51.55, 28.40], [51.21, 29.27], [52.53, 29.59]]
const SOUTH_PARS_COAST: [number, number][] = [[52.61, 27.48], [51.55, 28.30], [50.84, 28.92]]
const SOUTH_PARS = [SOUTH_PARS_INLAND, SOUTH_PARS_COAST]

function nearSouthPars(lat: number, lon: number): boolean {
  for (const line of SOUTH_PARS) if (pointToPolylineDist(lat, lon, line) <= 20000) return true
  return false
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'rural' | 'freeway' | 'southpars'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:     { light: 0.65, medium: 0.10, heavy: 0.15, moto: 0.10 },
  tier2:     { light: 0.65, medium: 0.08, heavy: 0.19, moto: 0.08 },
  rural:     { light: 0.55, medium: 0.05, heavy: 0.33, moto: 0.07 },
  freeway:   { light: 0.74, medium: 0.03, heavy: 0.21, moto: 0.02 },
  southpars: { light: 0.40, medium: 0.04, heavy: 0.52, moto: 0.04 },
}

/** Region for the vehicle split. Cities first (urban mix dominates even where a
 *  freight road passes through, e.g. Shiraz), then the South Pars freight haul,
 *  then bare motorways (آزادراه), else rural two-lane highway. */
function region(lat: number, lon: number, roadClass: number, tier: 0 | 1 | 2 | 3): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2 || tier === 3) return 'tier2'
  if (nearSouthPars(lat, lon)) return 'southpars'
  if (roadClass === 0 || roadClass === 10) return 'freeway' // motorway + link = آزادراه
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 50000, 10: 25000, // motorway + motorway_link
  1: 20000, 11: 10000, // trunk + trunk_link
  2: 11000, 12: 5500,  // primary + primary_link
  3: 5500,             // secondary
  4: 2500,             // tertiary
  5: 900,              // residential
}
const IR_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
    return splitVehicles(base * (tier === 1 ? 2.5 : tier === 2 ? 1.8 : tier === 3 ? 1.4 : 1), SPLITS[region(row.midLat, row.midLon, row.roadClass, tier)])
}

export const policy: NationalRoadPolicy = {
  country: 'IR',
  bbox: IR_SCAN_BBOX,
  coverage: IR_COVERAGE,
  traffic,
}
