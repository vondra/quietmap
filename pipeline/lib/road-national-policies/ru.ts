/** RU OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const RU_SCAN_BBOX: [number, number, number, number] = [41.0, 19.0, 82.0, 180.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Moscow/SPb wider to
// cover the ring roads + inner oblast; millionniki/regional ~city extent.
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1
  { name: 'Moscow',          lat: 55.75, lon: 37.62, mult: 3.5, half: 0.40 },
  { name: 'Saint Petersburg', lat: 59.94, lon: 30.31, mult: 3.0, half: 0.32 },
  // Tier 2 — millionniki (×2.0)
  { name: 'Novosibirsk',     lat: 55.03, lon: 82.92, mult: 2.0, half: 0.22 },
  { name: 'Yekaterinburg',   lat: 56.84, lon: 60.61, mult: 2.0, half: 0.22 },
  { name: 'Kazan',           lat: 55.79, lon: 49.12, mult: 2.0, half: 0.22 },
  { name: 'Nizhny Novgorod', lat: 56.33, lon: 44.00, mult: 2.0, half: 0.22 },
  { name: 'Krasnoyarsk',     lat: 56.01, lon: 92.85, mult: 2.0, half: 0.22 },
  { name: 'Chelyabinsk',     lat: 55.16, lon: 61.40, mult: 2.0, half: 0.22 },
  { name: 'Samara',          lat: 53.20, lon: 50.15, mult: 2.0, half: 0.22 },
  { name: 'Ufa',             lat: 54.74, lon: 55.97, mult: 2.0, half: 0.22 },
  { name: 'Rostov-on-Don',   lat: 47.24, lon: 39.71, mult: 2.0, half: 0.22 },
  { name: 'Krasnodar',       lat: 45.04, lon: 38.98, mult: 2.0, half: 0.22 },
  { name: 'Omsk',            lat: 54.99, lon: 73.37, mult: 2.0, half: 0.22 },
  { name: 'Voronezh',        lat: 51.66, lon: 39.20, mult: 2.0, half: 0.22 },
  { name: 'Perm',            lat: 58.01, lon: 56.25, mult: 2.0, half: 0.22 },
  { name: 'Volgograd',       lat: 48.71, lon: 44.51, mult: 2.0, half: 0.30 }, // long N-S along Volga
  // Tier 3 — sub-million regional capitals (×1.5)
  { name: 'Saratov',         lat: 51.53, lon: 46.03, mult: 1.5, half: 0.18 },
  { name: 'Tyumen',          lat: 57.15, lon: 65.53, mult: 1.5, half: 0.18 },
  { name: 'Tolyatti',        lat: 53.51, lon: 49.42, mult: 1.5, half: 0.18 },
  { name: 'Barnaul',         lat: 53.35, lon: 83.78, mult: 1.5, half: 0.18 },
  { name: 'Makhachkala',     lat: 42.98, lon: 47.50, mult: 1.5, half: 0.18 },
  { name: 'Izhevsk',         lat: 56.85, lon: 53.20, mult: 1.5, half: 0.18 },
  { name: 'Khabarovsk',      lat: 48.48, lon: 135.07, mult: 1.5, half: 0.18 },
  { name: 'Ulyanovsk',       lat: 54.32, lon: 48.40, mult: 1.5, half: 0.18 },
  { name: 'Irkutsk',         lat: 52.29, lon: 104.28, mult: 1.5, half: 0.18 },
  { name: 'Vladivostok',     lat: 43.12, lon: 131.89, mult: 1.5, half: 0.18 },
  { name: 'Yaroslavl',       lat: 57.63, lon: 39.87, mult: 1.5, half: 0.18 },
  { name: 'Tomsk',           lat: 56.49, lon: 84.95, mult: 1.5, half: 0.18 },
  { name: 'Stavropol',       lat: 45.04, lon: 41.97, mult: 1.5, half: 0.18 },
  { name: 'Kemerovo',        lat: 55.35, lon: 86.09, mult: 1.5, half: 0.18 },
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
  1: 14000, 11: 7000,  // trunk + trunk_link
  2: 7000,  12: 3500,  // primary + primary_link
  3: 3000,             // secondary
  4: 1200,             // tertiary
  5: 300,              // residential
}
const RU_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Heavy-vehicle share by road class — trucks concentrate on the high-order network. */
function heavyShare(cls: number): number {
  if (cls === 0 || cls === 1 || cls === 10 || cls === 11) return 0.27 // motorway/trunk + links
  if (cls === 2 || cls === 3 || cls === 12) return 0.12               // primary/secondary + primary_link
  return 0.05                                                          // tertiary/residential
}

function splitVehicles(aadt: number, cls: number) {
  const heavy = heavyShare(cls)
  const medium = 0.10 // LCV
  const moto = 0.01
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
  country: 'RU',
  bbox: RU_SCAN_BBOX,
  coverage: RU_COVERAGE,
  traffic,
}
