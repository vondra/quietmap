/** KZ OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const KZ_SCAN_BBOX: [number, number, number, number] = [40.0, 46.0, 56.0, 88.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Tier-1 wider (0.30) to cover
// the ring roads + inner suburbs; tier-2 ~city extent (0.18).
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1 (×2.0)
  { name: 'Almaty', lat: 43.24, lon: 76.92, mult: 2.0, half: 0.30 },
  { name: 'Astana', lat: 51.13, lon: 71.43, mult: 2.0, half: 0.30 },
  // Tier 2 (×1.4)
  { name: 'Shymkent',   lat: 42.32, lon: 69.59, mult: 1.4, half: 0.18 },
  { name: 'Karaganda',  lat: 49.80, lon: 73.10, mult: 1.4, half: 0.18 },
  { name: 'Aktobe',     lat: 50.28, lon: 57.21, mult: 1.4, half: 0.18 },
  { name: 'Atyrau',     lat: 47.12, lon: 51.88, mult: 1.4, half: 0.18 },
  { name: 'Aktau',      lat: 43.65, lon: 51.16, mult: 1.4, half: 0.18 },
  { name: 'Pavlodar',   lat: 52.29, lon: 76.97, mult: 1.4, half: 0.18 },
  { name: 'Kostanay',   lat: 53.21, lon: 63.62, mult: 1.4, half: 0.18 },
  { name: 'Oskemen',    lat: 49.95, lon: 82.61, mult: 1.4, half: 0.18 }, // Ust-Kamenogorsk
  { name: 'Semey',      lat: 50.41, lon: 80.23, mult: 1.4, half: 0.18 },
  { name: 'Taraz',      lat: 42.90, lon: 71.39, mult: 1.4, half: 0.18 },
  { name: 'Petropavl',  lat: 54.87, lon: 69.15, mult: 1.4, half: 0.18 },
  { name: 'Turkestan',  lat: 43.30, lon: 68.25, mult: 1.4, half: 0.18 },
  { name: 'Kyzylorda',  lat: 44.85, lon: 65.51, mult: 1.4, half: 0.18 },
  { name: 'Uralsk',     lat: 51.23, lon: 51.37, mult: 1.4, half: 0.18 },
]

/** City tier multiplier (1.0 = rural) for the road midpoint. */
function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── Freight corridors (elevated-HGV vehicle split, 62% heavy) ──
// Ekibastuz/Pavlodar coal box + Caspian Atyrau↔Tengiz↔Aktau oil zone, as small bboxes.
// Membership is tested on the road midpoint; the 62%-heavy split is applied only to the
// high-order trunk network (classes 0-2 + their links) inside these boxes — that long-
// haul coal/oil flow, not the in-town streets.
const FREIGHT_ZONES: ReadonlyArray<[number, number, number, number]> = [
  [51.3, 74.8, 52.6, 77.3], // Ekibastuz / Pavlodar coal box  [minLat,minLon,maxLat,maxLon]
  [43.4, 51.0, 47.6, 54.2], // Caspian oil zone Atyrau↔Tengiz↔Aktau
]
const FREIGHT_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 10, 11, 12])

function inFreightCorridor(lat: number, lon: number, cls: number): boolean {
  if (!FREIGHT_CLASSES.has(cls)) return false
  return FREIGHT_ZONES.some(z => inBbox(lat, lon, z))
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes), rural baseline ──
const BASE_AADT: Record<number, number> = {
  0: 20000, 10: 10000, // motorway + motorway_link
  1: 8000,  11: 4000,  // trunk + trunk_link
  2: 4000,  12: 2000,  // primary + primary_link
  3: 2000,             // secondary
  4: 800,              // tertiary
  5: 350,              // residential
}
const KZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Vehicle-split fractions keyed by tier (docs: split depends on tier, not class). */
type Split = { light: number; medium: number; heavy: number; moto: number }
const TIER1_SPLIT:   Split = { light: 0.72, medium: 0.06, heavy: 0.18, moto: 0.04 }
const TIER2_SPLIT:   Split = { light: 0.68, medium: 0.05, heavy: 0.24, moto: 0.03 }
const RURAL_SPLIT:   Split = { light: 0.55, medium: 0.03, heavy: 0.40, moto: 0.02 }
const FREIGHT_SPLIT: Split = { light: 0.35, medium: 0.02, heavy: 0.62, moto: 0.01 }

/** Pick the tier split. Freight corridor wins on the high-order network; otherwise
 *  metro tier (by multiplier) wins; else rural. Returns the split + a tier label. */
function tierSplit(lat: number, lon: number, cls: number, mult: number): { split: Split; tier: string } {
  if (inFreightCorridor(lat, lon, cls)) return { split: FREIGHT_SPLIT, tier: 'freight ×0.62hgv' }
  if (mult === 2.0) return { split: TIER1_SPLIT, tier: 'tier-1 ×2.0' }
  if (mult === 1.4) return { split: TIER2_SPLIT, tier: 'tier-2 ×1.4' }
  return { split: RURAL_SPLIT, tier: 'rural ×1.0' }
}

function splitVehicles(aadt: number, split: Split) {
  return {
    light: Math.round(aadt * split.light),
    medium: Math.round(aadt * split.medium),
    heavy: Math.round(aadt * split.heavy),
    moto: Math.round(aadt * split.moto),
  }
}

function traffic(row: Parameters<NationalRoadPolicy['traffic']>[0]): PolicyRoadTraffic | null {
  const base = BASE_AADT[row.roadClass]
  if (base === undefined) return null
  const mult = cityMultiplier(row.midLat, row.midLon)
    return splitVehicles(base * mult, tierSplit(row.midLat, row.midLon, row.roadClass, mult).split)
}

export const policy: NationalRoadPolicy = {
  country: 'KZ',
  bbox: KZ_SCAN_BBOX,
  coverage: KZ_COVERAGE,
  traffic,
}
