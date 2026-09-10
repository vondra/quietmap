/** MA OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const MA_SCAN_BBOX: [number, number, number, number] = [20.7, -17.3, 36.1, -0.9]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — the 5 metros. Boxes wide enough to cover the conurbations
  // (Casablanca + Mohammedia, Rabat + Salé + Témara).
  { name: 'Casablanca', lat: 33.57, lon: -7.59, tier: 1, half: 0.22 },
  { name: 'Rabat-Salé',  lat: 34.02, lon: -6.82, tier: 1, half: 0.18 },
  { name: 'Marrakech',   lat: 31.63, lon: -8.01, tier: 1, half: 0.16 },
  { name: 'Fez',         lat: 34.04, lon: -5.00, tier: 1, half: 0.14 },
  { name: 'Tangier',     lat: 35.77, lon: -5.80, tier: 1, half: 0.16 },
  // Tier 2 ×1.4 — provincial capitals + ports (20 cities).
  { name: 'Meknes',      lat: 33.90, lon: -5.55, tier: 2, half: 0.11 },
  { name: 'Oujda',       lat: 34.68, lon: -1.91, tier: 2, half: 0.11 },
  { name: 'Kenitra',     lat: 34.26, lon: -6.58, tier: 2, half: 0.10 },
  { name: 'Tetouan',     lat: 35.58, lon: -5.37, tier: 2, half: 0.10 },
  { name: 'Agadir',      lat: 30.42, lon: -9.60, tier: 2, half: 0.12 },
  { name: 'Nador',       lat: 35.17, lon: -2.93, tier: 2, half: 0.10 },
  { name: 'Safi',        lat: 32.30, lon: -9.24, tier: 2, half: 0.10 },
  { name: 'El Jadida',   lat: 33.23, lon: -8.50, tier: 2, half: 0.10 },
  { name: 'Khouribga',   lat: 32.88, lon: -6.91, tier: 2, half: 0.10 },
  { name: 'Beni Mellal', lat: 32.34, lon: -6.36, tier: 2, half: 0.10 },
  { name: 'Taza',        lat: 34.21, lon: -4.01, tier: 2, half: 0.10 },
  { name: 'Khemisset',   lat: 33.82, lon: -6.07, tier: 2, half: 0.10 },
  { name: 'Laayoune',    lat: 27.15, lon: -13.20, tier: 2, half: 0.12 },
  { name: 'Mohammedia',  lat: 33.69, lon: -7.38, tier: 2, half: 0.08 },
  { name: 'Settat',      lat: 33.00, lon: -7.62, tier: 2, half: 0.10 },
  { name: 'Larache',     lat: 35.19, lon: -6.16, tier: 2, half: 0.10 },
  { name: 'Ouarzazate',  lat: 30.92, lon: -6.89, tier: 2, half: 0.10 },
  { name: 'Taourirt',    lat: 34.41, lon: -2.89, tier: 2, half: 0.10 },
  { name: 'Essaouira',   lat: 31.51, lon: -9.77, tier: 2, half: 0.10 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── OCP phosphate haul corridors [lon,lat] — matched by min distance to the line ──
// Khouribga (world's largest phosphate mine) feeds two export complexes by a
// slurry pipeline + rail + truck haulage. The connecting RN/RP roads carry a
// heavy-truck share found nowhere else in Morocco's interior.
//   north: Khouribga → Jorf Lasfar fertilizer port (via El Jadida)
const PHOSPHATE_JORF: [number, number][] = [
  [-6.91, 32.88], [-7.60, 33.00], [-8.50, 33.23], [-8.63, 33.13],
]
//   south: Khouribga → Benguerir → Youssoufia → Safi phosphate port
const PHOSPHATE_SAFI: [number, number][] = [
  [-6.91, 32.88], [-7.95, 32.24], [-8.53, 32.25], [-9.24, 32.30],
]
const nearPhosphate = (lat: number, lon: number) =>
  pointToPolylineDist(lat, lon, PHOSPHATE_JORF) <= 15000 ||
  pointToPolylineDist(lat, lon, PHOSPHATE_SAFI) <= 15000

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'phosphate' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:     { light: 0.65, medium: 0.06, heavy: 0.12, moto: 0.17 },
  tier2:     { light: 0.65, medium: 0.08, heavy: 0.12, moto: 0.15 },
  phosphate: { light: 0.50, medium: 0.08, heavy: 0.35, moto: 0.07 },
  rural:     { light: 0.62, medium: 0.08, heavy: 0.20, moto: 0.10 },
}

/** Region for the vehicle split: cities win first (so urban streets aren't tagged
 *  35% truck), then the rural phosphate haul roads, else the rural RN network. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearPhosphate(lat, lon)) return 'phosphate'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const MA_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
  country: 'MA',
  bbox: MA_SCAN_BBOX,
  coverage: MA_COVERAGE,
  traffic,
}
