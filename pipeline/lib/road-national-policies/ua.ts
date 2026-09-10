/** UA OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const UA_SCAN_BBOX: [number, number, number, number] = [44.0, 22.0, 52.5, 40.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Kyiv wider to cover the
// ring + suburbs; regional/oblast ~city extent.
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1 — capital (×2.0)
  { name: 'Kyiv',            lat: 50.45, lon: 30.52, mult: 2.0, half: 0.30 },
  // Tier 2 — regional capitals (×1.6)
  { name: 'Kharkiv',         lat: 49.99, lon: 36.23, mult: 1.6, half: 0.18 },
  { name: 'Odesa',           lat: 46.48, lon: 30.72, mult: 1.6, half: 0.18 },
  { name: 'Dnipro',          lat: 48.46, lon: 35.04, mult: 1.6, half: 0.18 },
  { name: 'Lviv',            lat: 49.84, lon: 24.03, mult: 1.6, half: 0.18 },
  { name: 'Zaporizhzhia',    lat: 47.84, lon: 35.14, mult: 1.6, half: 0.18 },
  { name: 'Donetsk',         lat: 48.02, lon: 37.80, mult: 1.6, half: 0.18 },
  // Tier 3 — oblast centres (×1.3)
  { name: 'Mykolaiv',        lat: 46.97, lon: 31.99, mult: 1.3, half: 0.12 },
  { name: 'Vinnytsia',       lat: 49.23, lon: 28.47, mult: 1.3, half: 0.12 },
  { name: 'Poltava',         lat: 49.59, lon: 34.55, mult: 1.3, half: 0.12 },
  { name: 'Chernihiv',       lat: 51.49, lon: 31.29, mult: 1.3, half: 0.12 },
  { name: 'Zhytomyr',        lat: 50.25, lon: 28.66, mult: 1.3, half: 0.12 },
  { name: 'Sumy',            lat: 50.91, lon: 34.80, mult: 1.3, half: 0.12 },
  { name: 'Khmelnytskyi',    lat: 49.42, lon: 26.99, mult: 1.3, half: 0.12 },
  { name: 'Cherkasy',        lat: 49.44, lon: 32.06, mult: 1.3, half: 0.12 },
  { name: 'Ivano-Frankivsk', lat: 48.92, lon: 24.71, mult: 1.3, half: 0.12 },
  { name: 'Ternopil',        lat: 49.55, lon: 25.59, mult: 1.3, half: 0.12 },
  { name: 'Rivne',           lat: 50.62, lon: 26.25, mult: 1.3, half: 0.12 },
  { name: 'Uzhhorod',        lat: 48.62, lon: 22.30, mult: 1.3, half: 0.12 },
  { name: 'Lutsk',           lat: 50.75, lon: 25.34, mult: 1.3, half: 0.12 },
  { name: 'Chernivtsi',      lat: 48.29, lon: 25.94, mult: 1.3, half: 0.12 },
  { name: 'Kropyvnytskyi',   lat: 48.51, lon: 32.26, mult: 1.3, half: 0.12 },
  { name: 'Kremenchuk',      lat: 49.07, lon: 33.42, mult: 1.3, half: 0.12 },
  { name: 'Mariupol',        lat: 47.10, lon: 37.55, mult: 1.3, half: 0.12 },
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
  0: 28000, 10: 14000, // motorway + motorway_link
  1: 14000, 11: 7000,  // trunk + trunk_link
  2: 7000,  12: 3500,  // primary + primary_link
  3: 3500,             // secondary
  4: 1500,             // tertiary
  5: 600,              // residential
}
const UA_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Heavy-vehicle share by road class — grain/steel freight concentrates on trunks. */
function heavyShare(cls: number): number {
  if (cls === 0 || cls === 1 || cls === 10 || cls === 11) return 0.18 // motorway/trunk + links
  if (cls === 2 || cls === 3 || cls === 12) return 0.10               // primary/secondary + primary_link
  return 0.04                                                          // tertiary/residential
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
  country: 'UA',
  bbox: UA_SCAN_BBOX,
  coverage: UA_COVERAGE,
  traffic,
}
