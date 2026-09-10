/** EG OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const EG_SCAN_BBOX: [number, number, number, number] = [22.0, 24.7, 31.7, 36.9]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Cairo box is wide enough to also cover Giza, New Cairo, 6th October.
  { name: 'Greater Cairo', lat: 30.05, lon: 31.24, tier: 1, half: 0.45 },
  { name: 'Alexandria',    lat: 31.20, lon: 29.92, tier: 1, half: 0.30 },
  // Tier 2 ×1.5 — governorate capitals + Suez-Canal + Red-Sea/Sinai tourism cities.
  { name: 'Port Said',        lat: 31.26, lon: 32.30, tier: 2, half: 0.12 },
  { name: 'Suez',             lat: 29.97, lon: 32.55, tier: 2, half: 0.12 },
  { name: 'Ismailia',         lat: 30.60, lon: 32.27, tier: 2, half: 0.12 },
  { name: 'Luxor',            lat: 25.70, lon: 32.64, tier: 2, half: 0.12 },
  { name: 'Aswan',            lat: 24.09, lon: 32.90, tier: 2, half: 0.12 },
  { name: 'Hurghada',         lat: 27.26, lon: 33.81, tier: 2, half: 0.12 },
  { name: 'Sharm El Sheikh',  lat: 27.91, lon: 34.33, tier: 2, half: 0.12 },
  { name: 'Tanta',            lat: 30.79, lon: 31.00, tier: 2, half: 0.12 },
  { name: 'Mansoura',         lat: 31.04, lon: 31.38, tier: 2, half: 0.12 },
  { name: 'Mahalla el-Kubra', lat: 30.97, lon: 31.17, tier: 2, half: 0.12 },
  { name: 'Asyut',            lat: 27.18, lon: 31.18, tier: 2, half: 0.12 },
  { name: 'Sohag',            lat: 26.56, lon: 31.69, tier: 2, half: 0.12 },
  { name: 'Damanhur',         lat: 31.03, lon: 30.47, tier: 2, half: 0.12 },
  { name: 'Zagazig',          lat: 30.59, lon: 31.50, tier: 2, half: 0.12 },
  { name: 'Faiyum',           lat: 29.31, lon: 30.84, tier: 2, half: 0.12 },
  { name: 'Minya',            lat: 28.10, lon: 30.75, tier: 2, half: 0.12 },
  { name: 'Beni Suef',        lat: 29.07, lon: 31.10, tier: 2, half: 0.12 },
  { name: 'Qena',             lat: 26.16, lon: 32.72, tier: 2, half: 0.12 },
  { name: 'Kafr el-Sheikh',   lat: 31.11, lon: 30.94, tier: 2, half: 0.12 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Desert freight corridors [lon,lat] — high HGV share (~30%) ──
// Cairo↔Alex Desert Road, Cairo↔Suez, Cairo↔Ain Sokhna, and the Red Sea coast road
// (Suez↔Hurghada↔Marsa Alam) carry Egypt's truck freight across empty desert.
const DESERT_ROAD: [number, number][] = [[31.05, 30.05], [30.70, 30.30], [30.30, 30.60], [29.95, 30.95]]
const SUEZ_ROAD: [number, number][] = [[31.45, 30.00], [31.90, 29.98], [32.40, 29.97]]
const SOKHNA_ROAD: [number, number][] = [[31.50, 29.95], [31.90, 29.78], [32.34, 29.60]]
const RED_SEA_ROAD: [number, number][] = [[32.50, 29.90], [33.00, 28.70], [33.50, 27.70], [33.81, 27.26], [34.40, 25.90], [35.10, 24.30]]
const FREIGHT_CORRIDORS = [DESERT_ROAD, SUEZ_ROAD, SOKHNA_ROAD, RED_SEA_ROAD]

function nearFreightCorridor(lat: number, lon: number): boolean {
  for (const line of FREIGHT_CORRIDORS) if (pointToPolylineDist(lat, lon, line) <= 15000) return true
  return false
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'desert' | 'upper' | 'delta'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:  { light: 0.55, medium: 0.08, heavy: 0.10, moto: 0.27 },
  tier2:  { light: 0.60, medium: 0.08, heavy: 0.12, moto: 0.20 },
  desert: { light: 0.52, medium: 0.08, heavy: 0.30, moto: 0.10 },
  upper:  { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
  delta:  { light: 0.58, medium: 0.08, heavy: 0.20, moto: 0.14 },
}

/** Region for the vehicle split: cities first, then freight corridors, then the
 *  Upper-Egypt (south of ~Faiyum) vs Delta (north) split at the Cairo latitude. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearFreightCorridor(lat, lon)) return 'desert'
  return lat < 29.5 ? 'upper' : 'delta'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 40000, 10: 20000, // motorway + motorway_link
  1: 15000, 11: 7500,  // trunk + trunk_link
  2: 7000,  12: 3500,  // primary + primary_link
  3: 3500,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const EG_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
    return splitVehicles(base * (tier === 1 ? 2.5 : tier === 2 ? 1.5 : 1), SPLITS[region(row.midLat, row.midLon, tier)])
}

export const policy: NationalRoadPolicy = {
  country: 'EG',
  bbox: EG_SCAN_BBOX,
  coverage: EG_COVERAGE,
  traffic,
}
