/** NG OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const NG_SCAN_BBOX: [number, number, number, number] = [4.0, 2.7, 13.9, 14.7]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Greater Lagos / FCT
// wider to cover the conurbation + ring; others ~city extent.
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1
  { name: 'Lagos',  lat: 6.50, lon: 3.38, mult: 2.5, half: 0.30 }, // Greater Lagos, ~22M
  { name: 'Kano',   lat: 12.00, lon: 8.52, mult: 2.0, half: 0.18 },
  { name: 'Abuja',  lat: 9.06, lon: 7.49, mult: 2.0, half: 0.22 }, // FCT
  { name: 'Ibadan', lat: 7.38, lon: 3.90, mult: 2.0, half: 0.16 },
  // Tier 2 — 20 regional cities (×1.4)
  { name: 'Port Harcourt', lat: 4.82, lon: 7.04, mult: 1.4, half: 0.12 },
  { name: 'Benin City',    lat: 6.34, lon: 5.62, mult: 1.4, half: 0.12 },
  { name: 'Kaduna',        lat: 10.52, lon: 7.44, mult: 1.4, half: 0.12 },
  { name: 'Jos',           lat: 9.90, lon: 8.89, mult: 1.4, half: 0.12 },
  { name: 'Maiduguri',     lat: 11.83, lon: 13.15, mult: 1.4, half: 0.12 },
  { name: 'Enugu',         lat: 6.45, lon: 7.50, mult: 1.4, half: 0.12 },
  { name: 'Onitsha',       lat: 6.15, lon: 6.79, mult: 1.4, half: 0.12 },
  { name: 'Aba',           lat: 5.11, lon: 7.37, mult: 1.4, half: 0.12 },
  { name: 'Ilorin',        lat: 8.50, lon: 4.55, mult: 1.4, half: 0.12 },
  { name: 'Abeokuta',      lat: 7.16, lon: 3.35, mult: 1.4, half: 0.12 },
  { name: 'Zaria',         lat: 11.07, lon: 7.71, mult: 1.4, half: 0.12 },
  { name: 'Warri',         lat: 5.52, lon: 5.75, mult: 1.4, half: 0.12 },
  { name: 'Sokoto',        lat: 13.06, lon: 5.24, mult: 1.4, half: 0.12 },
  { name: 'Oyo',           lat: 7.85, lon: 3.93, mult: 1.4, half: 0.12 },
  { name: 'Akure',         lat: 7.25, lon: 5.20, mult: 1.4, half: 0.12 },
  { name: 'Bauchi',        lat: 10.31, lon: 9.84, mult: 1.4, half: 0.12 },
  { name: 'Calabar',       lat: 4.98, lon: 8.34, mult: 1.4, half: 0.12 },
  { name: 'Ogbomosho',     lat: 8.13, lon: 4.25, mult: 1.4, half: 0.12 },
  { name: 'Osogbo',        lat: 7.77, lon: 4.56, mult: 1.4, half: 0.12 },
  { name: 'Lokoja',        lat: 7.80, lon: 6.74, mult: 1.4, half: 0.12 },
]

// Tier-1 listed first, so an overlap resolves to the higher multiplier.
function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── Lagos-Ibadan / Apapa container-freight corridor ──
// ~40% of national container traffic queues through Apapa/Tin Can ports and the
// Lagos-Ibadan Expressway. The truck-heavy split is applied only to motorway/
// trunk-class roads inside these bands (ordinary Lagos arterials keep the
// moto-heavy metro split).
const FREIGHT_ZONES: [number, number, number, number][] = [
  [6.40, 3.28, 6.60, 3.45], // Apapa / Tin Can / Oshodi port-industrial Lagos
  [6.58, 3.34, 7.45, 3.98], // Lagos-Ibadan Expressway band (Lagos → Sagamu → Ibadan)
]
function inFreightCorridor(lat: number, lon: number): boolean {
  for (const b of FREIGHT_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}
const FREIGHT_CLASSES: ReadonlySet<number> = new Set([0, 1, 10, 11]) // motorway/trunk + links

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 35000, 10: 17500, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link (Federal A-routes)
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const NG_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

interface Split { light: number; medium: number; heavy: number; moto: number }
const TIER1: Split   = { light: 0.45, medium: 0.05, heavy: 0.15, moto: 0.35 }
const TIER2: Split   = { light: 0.45, medium: 0.06, heavy: 0.14, moto: 0.35 }
const RURAL: Split   = { light: 0.50, medium: 0.08, heavy: 0.22, moto: 0.20 }
const FREIGHT: Split = { light: 0.35, medium: 0.05, heavy: 0.45, moto: 0.15 }

function splitFor(mult: number, cls: number, freight: boolean): Split {
  if (freight && FREIGHT_CLASSES.has(cls)) return FREIGHT
  if (mult >= 2.0) return TIER1
  if (mult >= 1.4) return TIER2
  return RURAL
}

function applySplit(aadt: number, s: Split) {
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
  const mult = cityMultiplier(row.midLat, row.midLon)
    return applySplit(base * mult, splitFor(mult, row.roadClass, inFreightCorridor(row.midLat, row.midLon)))
}

export const policy: NationalRoadPolicy = {
  country: 'NG',
  bbox: NG_SCAN_BBOX,
  coverage: NG_COVERAGE,
  traffic,
}
