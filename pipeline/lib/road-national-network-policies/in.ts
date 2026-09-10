/** IN road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const IN_BBOX: [number, number, number, number] = [6.5, 68.0, 37.0, 98.0]

// Exclusion zones for neighbour countries inside IN bbox.
// Tight bboxes to avoid clipping Delhi (28.47°N, Pakistan border at 74°E is 280 km west)
// and Kolkata (22.83°N 88.49°E, Bangladesh border at 88.85°E is 40 km east).
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Delhi NCR', bbox: [28.3, 76.8, 29.2, 77.6] },
  { name: 'Mumbai', bbox: [18.9, 72.7, 19.3, 73.0] },
  { name: 'Bangalore', bbox: [12.8, 77.4, 13.2, 77.8] },
  { name: 'Hyderabad', bbox: [17.2, 78.2, 17.6, 78.7] },
  { name: 'Chennai', bbox: [12.8, 80.1, 13.2, 80.4] },
  { name: 'Kolkata', bbox: [22.3, 88.2, 22.8, 88.6] },
  { name: 'Ahmedabad', bbox: [22.9, 72.4, 23.2, 72.8] },
  { name: 'Pune', bbox: [18.4, 73.7, 18.7, 74.0] },
]

// Tier-2 Indian cities
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Jaipur', bbox: [26.8, 75.7, 27.0, 76.0] },
  { name: 'Lucknow', bbox: [26.75, 80.8, 27.0, 81.1] },
  { name: 'Kanpur', bbox: [26.35, 80.25, 26.55, 80.45] },
  { name: 'Nagpur', bbox: [21.05, 79.0, 21.25, 79.2] },
  { name: 'Indore', bbox: [22.6, 75.7, 22.8, 75.95] },
  { name: 'Thane', bbox: [19.1, 72.9, 19.3, 73.1] },
  { name: 'Bhopal', bbox: [23.1, 77.3, 23.3, 77.55] },
  { name: 'Visakhapatnam', bbox: [17.6, 83.1, 17.8, 83.4] },
  { name: 'Patna', bbox: [25.55, 85.0, 25.7, 85.2] },
  { name: 'Vadodara', bbox: [22.25, 73.1, 22.4, 73.3] },
  { name: 'Ghaziabad', bbox: [28.6, 77.35, 28.75, 77.55] },
  { name: 'Ludhiana', bbox: [30.85, 75.7, 31.0, 75.95] },
  { name: 'Agra', bbox: [27.1, 77.95, 27.25, 78.15] },
  { name: 'Nashik', bbox: [19.95, 73.7, 20.1, 73.9] },
  { name: 'Faridabad', bbox: [28.35, 77.25, 28.5, 77.45] },
  { name: 'Meerut', bbox: [28.95, 77.65, 29.1, 77.85] },
  { name: 'Rajkot', bbox: [22.25, 70.7, 22.4, 70.9] },
  { name: 'Kalyan', bbox: [19.2, 73.1, 19.35, 73.25] },
  { name: 'Varanasi', bbox: [25.25, 82.95, 25.4, 83.1] },
  { name: 'Srinagar', bbox: [34.05, 74.75, 34.2, 74.9] },
  { name: 'Amritsar', bbox: [31.6, 74.8, 31.75, 74.95] },
  { name: 'Allahabad/Prayagraj', bbox: [25.4, 81.8, 25.55, 81.95] },
  { name: 'Ranchi', bbox: [23.3, 85.25, 23.45, 85.4] },
  { name: 'Howrah', bbox: [22.55, 88.25, 22.7, 88.4] },
  { name: 'Coimbatore', bbox: [10.95, 76.9, 11.1, 77.1] },
  { name: 'Jabalpur', bbox: [23.1, 79.9, 23.25, 80.05] },
  { name: 'Gwalior', bbox: [26.15, 78.15, 26.3, 78.3] },
  { name: 'Vijayawada', bbox: [16.45, 80.55, 16.6, 80.75] },
  { name: 'Jodhpur', bbox: [26.2, 72.9, 26.35, 73.1] },
  { name: 'Madurai', bbox: [9.85, 78.1, 10.0, 78.25] },
  { name: 'Kochi', bbox: [9.9, 76.2, 10.05, 76.35] },
  { name: 'Thiruvananthapuram', bbox: [8.45, 76.85, 8.6, 77.0] },
  { name: 'Surat', bbox: [21.1, 72.75, 21.3, 72.95] },
]

function cityTier(lat: number, lon: number): 1 | 2 | 0 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

// ── Load Bharatmala + build spatial grid ──

const BHARATMALA_AADT: Record<string, number> = {
  'Expressway': 80000,
  'National Highway': 35000,
  'State Highway': 15000,
  'Ring Road': 50000,
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.3 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    // Tier-1 Indian metro: motorcycle-dominated
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.40),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.09),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.31),
    }
  }
  // Rural
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.15),
    moto: Math.round(aadt * 0.20),
  }
}

const text = (line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string =>
  String(line.properties[name] ?? '').trim()

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
    return splitVehicles(BHARATMALA_AADT[text(line, 'rdtype')] * tierMultiplier(tier), tier)
}

export const policy: NationalRoadLinePolicy = {
  country: 'IN',
  bbox: IN_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 'in/bharatmala-roads.geojson', sha256: '6e7bbdc82c80ca272161e8173328ea5d3fab507a230f192b2a60f47a7918ad3b' },
  ],
  radiusMetres: 300,
  acceptLine: line => BHARATMALA_AADT[text(line, 'rdtype')] !== undefined,
  traffic,
}
