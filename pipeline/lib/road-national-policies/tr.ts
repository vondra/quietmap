/** TR OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const TR_SCAN_BBOX: [number, number, number, number] = [35.8, 25.6, 42.2, 44.8]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2 | 3; half: number }

const CITIES: City[] = [
  // Tier-1 ×2.5 — Istanbul box spans both banks of the Bosphorus.
  { name: 'Istanbul', lat: 41.05, lon: 28.95, tier: 1, half: 0.45 },
  // Tier-2 ×1.8 — capital + Aegean metropolis.
  { name: 'Ankara', lat: 39.93, lon: 32.86, tier: 2, half: 0.32 },
  { name: 'İzmir',  lat: 38.42, lon: 27.14, tier: 2, half: 0.30 },
  // Tier-3 ×1.4 — 19 cities.
  { name: 'Bursa',     lat: 40.19, lon: 29.06, tier: 3, half: 0.20 },
  { name: 'Antalya',   lat: 36.90, lon: 30.70, tier: 3, half: 0.18 },
  { name: 'Adana',     lat: 37.00, lon: 35.32, tier: 3, half: 0.18 },
  { name: 'Gaziantep', lat: 37.07, lon: 37.38, tier: 3, half: 0.18 },
  { name: 'Konya',     lat: 37.87, lon: 32.48, tier: 3, half: 0.18 },
  { name: 'Kayseri',   lat: 38.73, lon: 35.48, tier: 3, half: 0.15 },
  { name: 'Mersin',    lat: 36.81, lon: 34.63, tier: 3, half: 0.15 },
  { name: 'Eskişehir', lat: 39.78, lon: 30.52, tier: 3, half: 0.15 },
  { name: 'Diyarbakır',lat: 37.91, lon: 40.24, tier: 3, half: 0.15 },
  { name: 'Samsun',    lat: 41.29, lon: 36.33, tier: 3, half: 0.15 },
  { name: 'Trabzon',   lat: 41.00, lon: 39.72, tier: 3, half: 0.13 },
  { name: 'Şanlıurfa', lat: 37.16, lon: 38.80, tier: 3, half: 0.15 },
  { name: 'Malatya',   lat: 38.36, lon: 38.31, tier: 3, half: 0.13 },
  { name: 'Erzurum',   lat: 39.90, lon: 41.27, tier: 3, half: 0.13 },
  { name: 'Van',       lat: 38.49, lon: 43.38, tier: 3, half: 0.13 },
  { name: 'Denizli',   lat: 37.78, lon: 29.09, tier: 3, half: 0.15 },
  { name: 'Manisa',    lat: 38.61, lon: 27.43, tier: 3, half: 0.13 },
  { name: 'Kocaeli',   lat: 40.77, lon: 29.92, tier: 3, half: 0.18 }, // Ford Otosan / petrochemical
  { name: 'Sakarya',   lat: 40.78, lon: 30.40, tier: 3, half: 0.15 }, // Toyota Turkey
]

/** Highest city tier whose box contains the point (Tier-1 wins over overlaps). */
function cityTier(lat: number, lon: number): 0 | 1 | 2 | 3 {
  let best: 0 | 1 | 2 | 3 = 0
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) {
      if (best === 0 || c.tier < best) best = c.tier
    }
  }
  return best
}

// ── D-400 Mediterranean transit corridor [lon,lat] — İzmir↔Antalya↔Mersin↔Adana↔
//    Gaziantep. Carries Turkey's TIR freight to the Syria/Iraq border (~35% heavy). ──
const D400_WEST: [number, number][] = [
  [27.14, 38.42], [27.84, 37.85], [28.36, 37.21], [29.10, 36.80], [30.13, 36.88], [30.70, 36.89],
]
const D400_EAST: [number, number][] = [
  [30.70, 36.89], [31.99, 36.55], [32.83, 36.07], [33.94, 36.38], [34.64, 36.81],
  [35.33, 37.00], [36.17, 37.07], [37.06, 37.06], [37.38, 37.07],
]
function nearD400(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, D400_WEST) <= 15000 ||
         pointToPolylineDist(lat, lon, D400_EAST) <= 15000
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes); links get ~half ──
const BASE_AADT: Record<number, number> = {
  0: 55000, 10: 27500, // motorway + motorway_link (O-roads)
  1: 22000, 11: 11000, // trunk + trunk_link (D-routes)
  2: 12000, 12: 6000,  // primary + primary_link
  3: 6000,             // secondary
  4: 3000,             // tertiary
  5: 1000,             // residential
}
const TR_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

// ── Vehicle splits (fractions sum to 1.0) ──
interface Split { light: number; medium: number; heavy: number; moto: number }
const MOTORWAY: Split  = { light: 0.72, medium: 0.04, heavy: 0.22, moto: 0.02 }
const RURAL_MAJOR: Split = { light: 0.55, medium: 0.06, heavy: 0.32, moto: 0.07 }
const D400: Split      = { light: 0.55, medium: 0.05, heavy: 0.35, moto: 0.05 }
const ISTANBUL: Split  = { light: 0.62, medium: 0.12, heavy: 0.18, moto: 0.08 }
const URBAN: Split     = { light: 0.64, medium: 0.10, heavy: 0.19, moto: 0.07 } // Ankara/İzmir + Tier-3
const RURAL_MINOR: Split = { light: 0.70, medium: 0.07, heavy: 0.13, moto: 0.10 }

/** Vehicle split by road class × urbanity. Heavy share is the dominant lever, so
 *  motorways (O-roads) and inter-urban D-roads (trunk/primary) keep the TIR-heavy
 *  profiles even inside a city box; minor rural roads stay local/low-heavy. */
function splitFor(roadClass: number, tier: 0 | 1 | 2 | 3, lat: number, lon: number): Split {
  if (roadClass === 0 || roadClass === 10) return MOTORWAY
  const isMajor = roadClass === 1 || roadClass === 11 || roadClass === 2 || roadClass === 12
  if (tier === 0 && isMajor) return nearD400(lat, lon) ? D400 : RURAL_MAJOR
  if (tier === 1) return ISTANBUL
  if (tier === 2 || tier === 3) return URBAN
  return RURAL_MINOR
}

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
    return splitVehicles(base * (tier === 1 ? 2.5 : tier === 2 ? 1.8 : tier === 3 ? 1.4 : 1), splitFor(row.roadClass, tier, row.midLat, row.midLon))
}

export const policy: NationalRoadPolicy = {
  country: 'TR',
  bbox: TR_SCAN_BBOX,
  coverage: TR_COVERAGE,
  traffic,
}
