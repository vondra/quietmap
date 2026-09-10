/** SD OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const SD_SCAN_BBOX: [number, number, number, number] = [8.7, 21.8, 22.2, 38.6]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — box wide enough to cover the whole tri-city Khartoum metro
  // (Khartoum + Omdurman + Khartoum North/Bahri around the Nile confluence).
  { name: 'Greater Khartoum', lat: 15.58, lon: 32.53, tier: 1, half: 0.20 },
  // Tier 2 ×1.4 — state capitals + the port + rail/agricultural hubs.
  { name: 'Port Sudan',  lat: 19.62, lon: 37.22, tier: 2, half: 0.10 },
  { name: 'Kassala',     lat: 15.45, lon: 36.40, tier: 2, half: 0.09 },
  { name: 'El Obeid',    lat: 13.18, lon: 30.22, tier: 2, half: 0.09 },
  { name: 'Wad Madani',  lat: 14.40, lon: 33.52, tier: 2, half: 0.09 },
  { name: 'Atbara',      lat: 17.70, lon: 33.99, tier: 2, half: 0.09 },
  { name: 'Gedaref',     lat: 14.04, lon: 35.38, tier: 2, half: 0.09 },
  { name: 'El Fasher',   lat: 13.63, lon: 25.35, tier: 2, half: 0.09 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Corridor polyline [lon,lat] — matched by min distance to the whole line ──
// Khartoum → Port Sudan: the sole heavy-freight land artery to the coast. The
// modern paved highway cuts NE across the Butana to Haiya, then north to Port
// Sudan; it carries the tanker/container truck traffic for ~90% of Sudan's trade.
const PORTSUDAN_ARTERY: [number, number][] = [
  [32.53, 15.58], [33.6, 16.1], [35.0, 17.2], [36.32, 18.33], [37.22, 19.62],
]
const nearArtery = (lat: number, lon: number) => pointToPolylineDist(lat, lon, PORTSUDAN_ARTERY) <= 25000

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.55, medium: 0.12, heavy: 0.18, moto: 0.15 },
  tier2:    { light: 0.50, medium: 0.10, heavy: 0.22, moto: 0.18 },
  corridor: { light: 0.35, medium: 0.08, heavy: 0.50, moto: 0.07 },
  rural:    { light: 0.40, medium: 0.08, heavy: 0.35, moto: 0.17 },
}

/** Region for the vehicle split: cities first (urban mix wins inside the metro
 *  even where the artery passes through), then the Port Sudan freight artery,
 *  else the rural national network. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearArtery(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link
  1: 8000,  11: 4000,  // trunk + trunk_link
  2: 3500,  12: 1750,  // primary + primary_link
  3: 1500,             // secondary
  4: 600,              // tertiary
  5: 300,              // residential
}
const SD_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
  country: 'SD',
  bbox: SD_SCAN_BBOX,
  coverage: SD_COVERAGE,
  traffic,
}
