/** UZ OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const UZ_SCAN_BBOX: [number, number, number, number] = [37.2, 55.9, 45.6, 73.2]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid (~city + ring extent).
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1 — Tashkent (~2.9 M)
  { name: 'Tashkent',  lat: 41.31, lon: 69.24, mult: 2.0, half: 0.30 },
  // Tier 2 — regional capitals (×1.4)
  { name: 'Samarkand', lat: 39.65, lon: 66.96, mult: 1.4, half: 0.16 },
  { name: 'Namangan',  lat: 40.99, lon: 71.67, mult: 1.4, half: 0.16 },
  { name: 'Andijan',   lat: 40.78, lon: 72.34, mult: 1.4, half: 0.16 },
  { name: 'Bukhara',   lat: 39.77, lon: 64.42, mult: 1.4, half: 0.16 },
  { name: 'Fergana',   lat: 40.39, lon: 71.78, mult: 1.4, half: 0.16 },
  { name: 'Nukus',     lat: 42.46, lon: 59.61, mult: 1.4, half: 0.16 },
  { name: 'Qarshi',    lat: 38.86, lon: 65.79, mult: 1.4, half: 0.16 },
  { name: 'Jizzakh',   lat: 40.12, lon: 67.84, mult: 1.4, half: 0.16 },
  { name: 'Navoiy',    lat: 40.10, lon: 65.38, mult: 1.4, half: 0.16 },
  { name: 'Urgench',   lat: 41.55, lon: 60.63, mult: 1.4, half: 0.16 },
]

function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 25000, 10: 12500, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1400,             // tertiary
  5: 600,              // residential
}
const UZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Heavy-vehicle share by road class — trucks concentrate on the high-order
 *  network; UZ's freight reliance + M-39 transit lift the top tier above RU's. */
function heavyShare(cls: number): number {
  if (cls === 0 || cls === 1 || cls === 10 || cls === 11) return 0.30 // motorway/trunk + links
  if (cls === 2 || cls === 3 || cls === 12) return 0.16               // primary/secondary + primary_link
  if (cls === 4) return 0.08                                          // tertiary
  return 0.05                                                         // residential
}

function splitVehicles(aadt: number, cls: number) {
  const heavy = heavyShare(cls)
  const medium = 0.06 // LCV
  const moto = 0.04
  const light = 1 - heavy - medium - moto
  return {
    light: Math.round(aadt * light),
    medium: Math.round(aadt * medium),
    heavy: Math.round(aadt * heavy),
    moto: Math.round(aadt * moto),
  }
}

function traffic(row: Parameters<NationalRoadPolicy['traffic']>[0]): PolicyRoadTraffic | null {
  const base = BASE_AADT[row.roadClass]
  if (base === undefined) return null
  const mult = cityMultiplier(row.midLat, row.midLon)
    return splitVehicles(base * mult, row.roadClass)
}

export const policy: NationalRoadPolicy = {
  country: 'UZ',
  bbox: UZ_SCAN_BBOX,
  coverage: UZ_COVERAGE,
  traffic,
}
