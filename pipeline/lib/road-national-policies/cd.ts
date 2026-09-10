/** CD OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const CD_SCAN_BBOX: [number, number, number, number] = [-13.5, 12.0, 5.5, 31.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Kinshasa metro + immediate sprawl (E along Blvd Lumumba to N'Djili).
  { name: 'Kinshasa', lat: -4.325, lon: 15.322, tier: 1, half: 0.30 },
  // Tier 2 ×1.4 — provincial capitals + mining centres.
  { name: 'Lubumbashi', lat: -11.660, lon: 27.480, tier: 2, half: 0.12 },
  { name: 'Mbuji-Mayi', lat:  -6.150, lon: 23.600, tier: 2, half: 0.10 },
  { name: 'Kisangani',  lat:   0.520, lon: 25.200, tier: 2, half: 0.10 },
  { name: 'Kananga',    lat:  -5.900, lon: 22.420, tier: 2, half: 0.09 },
  { name: 'Bukavu',     lat:  -2.510, lon: 28.840, tier: 2, half: 0.07 },
  { name: 'Goma',       lat:  -1.660, lon: 29.220, tier: 2, half: 0.07 },
  { name: 'Likasi',     lat: -10.980, lon: 26.730, tier: 2, half: 0.07 },
  { name: 'Kolwezi',    lat: -10.720, lon: 25.470, tier: 2, half: 0.07 },
  { name: 'Matadi',     lat:  -5.820, lon: 13.450, tier: 2, half: 0.07 },
  { name: 'Kikwit',     lat:  -5.040, lon: 18.820, tier: 2, half: 0.07 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Heavy-truck freight corridors [lon,lat] — matched by min distance to line ──
// RN1 Matadi↔Kinshasa: the DRC's economic lifeline — every container for the
// ~17 M capital lands at Matadi (no rail capacity), trucked ~350 km up RN1.
const MATADI_KINSHASA_RN1: [number, number][] = [
  [13.45, -5.82], [13.92, -5.70], [14.44, -5.56], [15.10, -5.13], [15.31, -4.33],
]
// Katanga copperbelt Kolwezi→Likasi→Lubumbashi→Kasumbalesa: cobalt/copper export
// trucking to the Zambian border (DRC produces ~70 % of world cobalt).
const KATANGA_COPPERBELT: [number, number][] = [
  [25.47, -10.72], [26.73, -10.98], [27.48, -11.66], [27.79, -12.22],
]

function nearFreightCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, MATADI_KINSHASA_RN1) <= 15000 ||
         pointToPolylineDist(lat, lon, KATANGA_COPPERBELT) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.50, medium: 0.17, heavy: 0.08, moto: 0.25 },
  tier2:    { light: 0.50, medium: 0.15, heavy: 0.12, moto: 0.23 },
  corridor: { light: 0.42, medium: 0.08, heavy: 0.40, moto: 0.10 },
  rural:    { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
}

/** Region for the vehicle split: cities first (urban minibus/wewa mix dominates
 *  the metro surface streets), then the freight corridors on their long rural
 *  stretches, then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearFreightCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 15000, 10: 7500, // motorway + motorway_link (none in DRC; safety only)
  1: 5000,  11: 2500, // trunk + trunk_link (Routes Nationales)
  2: 2500,  12: 1250, // primary + primary_link
  3: 1200,            // secondary
  4: 500,             // tertiary
  5: 250,             // residential
}
const CD_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

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
    return splitVehicles(base * (tier === 1 ? 2.5 : tier === 2 ? 1.4 : 1), SPLITS[region(row.midLat, row.midLon, tier)])
}

export const policy: NationalRoadPolicy = {
  country: 'CD',
  bbox: CD_SCAN_BBOX,
  coverage: CD_COVERAGE,
  traffic,
}
