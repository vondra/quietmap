/** PH road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const PH_BBOX: [number, number, number, number] = [4.0, 115.8, 22.0, 127.5]

// Exclusion zones for neighbour countries inside PH bbox
const METRO_MANILA: [number, number, number, number] = [14.3, 120.8, 14.85, 121.2]

// Regional Tier-2 cities
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Cebu City', bbox: [10.25, 123.8, 10.45, 124.0] },
  { name: 'Davao City', bbox: [7.0, 125.5, 7.15, 125.65] },
  { name: 'Quezon City', bbox: [14.6, 121.0, 14.75, 121.15] },  // Part of NCR but explicit
  { name: 'Iloilo City', bbox: [10.6, 122.5, 10.8, 122.7] },
  { name: 'Bacolod', bbox: [10.6, 122.9, 10.75, 123.05] },
  { name: 'Zamboanga City', bbox: [6.85, 122.0, 6.98, 122.15] },
  { name: 'Cagayan de Oro', bbox: [8.4, 124.58, 8.55, 124.73] },
  { name: 'Baguio', bbox: [16.38, 120.55, 16.45, 120.65] },
  { name: 'General Santos', bbox: [6.05, 125.1, 6.2, 125.22] },
  { name: 'Angeles', bbox: [15.12, 120.55, 15.2, 120.65] },
  { name: 'Bataan (Mariveles)', bbox: [14.35, 120.4, 14.5, 120.55] },
  { name: 'Cavite City', bbox: [14.45, 120.85, 14.55, 120.97] },
  { name: 'Calamba (Laguna)', bbox: [14.15, 121.1, 14.25, 121.2] },
  { name: 'Tacloban', bbox: [11.2, 124.95, 11.3, 125.05] },
  { name: 'Dagupan', bbox: [16.0, 120.32, 16.08, 120.4] },
  { name: 'Olongapo', bbox: [14.82, 120.25, 14.88, 120.32] },
  { name: 'Naga (Camarines Sur)', bbox: [13.6, 123.15, 13.7, 123.25] },
  { name: 'Butuan', bbox: [8.92, 125.52, 9.0, 125.62] },
  { name: 'Iligan', bbox: [8.2, 124.2, 8.28, 124.3] },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  if (inBbox(lat, lon, METRO_MANILA)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

const DPWH_AADT: Record<string, number> = {
  'Primary': 50000,
  'Secondary': 20000,
  'Tertiary': 8000,
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.35),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.50),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.40),
    }
  }
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.10),
    moto: Math.round(aadt * 0.25),
  }
}

// PH archipelago bbox (mirrors PH_BBOX); the prepared-square listing skips the rest of
const text = (line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string =>
  String(line.properties[name] ?? '').trim()

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
    return splitVehicles(DPWH_AADT[text(line, 'ROAD_SEC_CLASS')] * tierMultiplier(tier), tier)
}

export const policy: NationalRoadLinePolicy = {
  country: 'PH',
  bbox: PH_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 'ph/roads-classification.geojson', sha256: '8559e3dd3eec71fac3250bc822c3d82e47c7b1d6b80d58a8bf433d6064892329' },
  ],
  radiusMetres: 300,
  acceptLine: line => DPWH_AADT[text(line, 'ROAD_SEC_CLASS')] !== undefined,
  traffic,
}
