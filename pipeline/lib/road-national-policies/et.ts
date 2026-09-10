/** ET OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const ET_SCAN_BBOX: [number, number, number, number] = [3.4, 32.9, 14.9, 48.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — Addis metro + Ring Road + satellite sprawl (Burayu/Sebeta/Legetafo).
  { name: 'Addis Ababa', lat: 9.030, lon: 38.740, tier: 1, half: 0.25 },
  // Tier 2 ×1.4 — regional capitals + major towns.
  { name: 'Dire Dawa',     lat: 9.593,  lon: 41.866, tier: 2, half: 0.10 },
  { name: 'Adama',         lat: 8.540,  lon: 39.270, tier: 2, half: 0.10 },
  { name: 'Mekelle',       lat: 13.497, lon: 39.477, tier: 2, half: 0.09 },
  { name: 'Gondar',        lat: 12.603, lon: 37.452, tier: 2, half: 0.08 },
  { name: 'Bahir Dar',     lat: 11.594, lon: 37.391, tier: 2, half: 0.08 },
  { name: 'Hawassa',       lat: 7.062,  lon: 38.478, tier: 2, half: 0.08 },
  { name: 'Dessie',        lat: 11.133, lon: 39.633, tier: 2, half: 0.07 },
  { name: 'Jimma',         lat: 7.667,  lon: 36.833, tier: 2, half: 0.07 },
  { name: 'Shashamane',    lat: 7.200,  lon: 38.600, tier: 2, half: 0.07 },
  { name: 'Dilla',         lat: 6.410,  lon: 38.310, tier: 2, half: 0.06 },
  { name: 'Debre Markos',  lat: 10.333, lon: 37.717, tier: 2, half: 0.06 },
  { name: 'Debre Birhan',  lat: 9.680,  lon: 39.533, tier: 2, half: 0.06 },
  { name: 'Assela',        lat: 7.950,  lon: 39.133, tier: 2, half: 0.06 },
  { name: 'Nekemte',       lat: 9.083,  lon: 36.550, tier: 2, half: 0.06 },
  { name: 'Kombolcha',     lat: 11.083, lon: 39.733, tier: 2, half: 0.06 },
  { name: 'Harar',         lat: 9.311,  lon: 42.128, tier: 2, half: 0.06 },
  { name: 'Arba Minch',    lat: 6.033,  lon: 37.550, tier: 2, half: 0.06 },
  { name: 'Wukro',         lat: 13.789, lon: 39.600, tier: 2, half: 0.05 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Addis↔Djibouti freight corridors [lon,lat] — high HGV share (~45%) ──
// Landlocked Ethiopia routes ~95 % of its trade through Djibouti's Doraleh
// Container Terminal. Two truck routes carry it: the Afar/Galafi route (the
// dominant container-truck path through the lowlands) and the Dire Dawa route
// (parallel to the electrified EDR railway). A receiver near either runs heavy.
// Vertices trace the real A1 closely (≤~80 km spacing) so the long highway
// segments' midpoints stay inside the 15 km buffer — a coarse chord across the
// Afar valley left the main trunk mis-classified as rural (24% vs 45% HGV).
const CORRIDOR_GALAFI: [number, number][] = [
  [38.740, 9.030], [39.110, 8.590], [39.270, 8.540], [39.920, 8.900],
  [40.170, 8.990], [40.550, 9.600], [40.650, 10.170], [40.720, 10.800],
  [40.770, 11.410], [40.990, 11.790], [41.400, 11.650], [41.750, 11.580],
]
const CORRIDOR_DIRE_DAWA: [number, number][] = [
  [40.170, 8.990], [40.450, 9.100], [40.750, 9.240], [41.100, 9.220],
  [41.450, 9.410], [41.866, 9.593], [42.130, 9.750], [42.400, 9.900],
]

function nearCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, CORRIDOR_GALAFI) <= 15000 ||
         pointToPolylineDist(lat, lon, CORRIDOR_DIRE_DAWA) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.60, medium: 0.15, heavy: 0.12, moto: 0.13 },
  tier2:    { light: 0.62, medium: 0.12, heavy: 0.14, moto: 0.12 },
  corridor: { light: 0.40, medium: 0.08, heavy: 0.45, moto: 0.07 },
  rural:    { light: 0.58, medium: 0.10, heavy: 0.24, moto: 0.08 },
}

/** Region for the vehicle split: cities first (urban minibus/Bajaj mix dominates
 *  metro surface streets), then the Djibouti freight corridor on rural stretches,
 *  then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link (Addis–Adama Expressway, Ring Road)
  1: 10000, 11: 5000,  // trunk + trunk_link (A-routes A1..A6)
  2: 5000,  12: 2500,  // primary + primary_link
  3: 2500,             // secondary
  4: 1200,             // tertiary
  5: 600,              // residential
}
const ET_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
  country: 'ET',
  bbox: ET_SCAN_BBOX,
  coverage: ET_COVERAGE,
  traffic,
}
