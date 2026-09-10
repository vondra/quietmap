/** PY road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const PY_BBOX: [number, number, number, number] = [-27.7, -62.7, -19.3, -54.2]

const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Gran Asunción', bbox: [-25.45, -57.72, -25.20, -57.35] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Ciudad del Este', bbox: [-25.55, -54.72, -25.45, -54.58] },
  { name: 'Encarnación',    bbox: [-27.35, -55.90, -27.28, -55.78] },
  { name: 'Luque',          bbox: [-25.30, -57.55, -25.22, -57.45] },
  { name: 'San Lorenzo',    bbox: [-25.37, -57.55, -25.30, -57.48] },
  { name: 'Capiatá',        bbox: [-25.40, -57.48, -25.33, -57.40] },
  { name: 'Lambaré',        bbox: [-25.36, -57.65, -25.30, -57.58] },
  { name: 'Fernando de la Mora', bbox: [-25.35, -57.57, -25.30, -57.50] },
  { name: 'Limpio',         bbox: [-25.17, -57.50, -25.12, -57.43] },
  { name: 'Ñemby',          bbox: [-25.42, -57.55, -25.36, -57.50] },
  { name: 'Pedro Juan Caballero', bbox: [-22.58, -55.77, -22.52, -55.70] },
  { name: 'Concepción',     bbox: [-23.43, -57.47, -23.38, -57.39] },
  { name: 'Coronel Oviedo', bbox: [-25.45, -56.48, -25.40, -56.42] },
  { name: 'Villarrica',     bbox: [-25.80, -56.48, -25.73, -56.40] },
  { name: 'Pilar',          bbox: [-26.88, -58.32, -26.82, -58.25] },
  { name: 'Caaguazú',       bbox: [-25.50, -56.05, -25.42, -55.97] },
  { name: 'San Lorenzo de Chaco (Filadelfia)', bbox: [-22.36, -60.06, -22.32, -60.00] },
  { name: 'Salto del Guairá', bbox: [-24.09, -54.35, -24.00, -54.28] },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}
// Chaco region (Occidental) — sparse, different vehicle split
function inChaco(lat: number, lon: number): boolean {
  // West of Río Paraguay (~-57.3) and north of Asunción
  return lon < -57.3 && lat > -26.0
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, chaco: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.65),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.20),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.62),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.20),
    }
  }
  if (chaco) {
    return {
      light: Math.round(aadt * 0.60),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.20),
      moto: Math.round(aadt * 0.10),
    }
  }
  // Rural Oriental (soy freight corridors)
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.25),
    moto: Math.round(aadt * 0.12),
  }
}

// Paraguay bbox (matches PY_BBOX); [minLat,minLon,maxLat,maxLon]. the prepared-square listing
const text = (line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string =>
  String(line.properties[name] ?? '').trim()

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
    const surface = text(line, 'TIPO_SUP').toUpperCase()
    const base = /^PCA$/.test(surface) ? 12000 : /PCA.*T|T.*PCA/.test(surface) ? 8000
      : /^T$/.test(surface) ? 3000 : 6000
    return splitVehicles(base * tierMultiplier(tier), tier, inChaco(row.midLat, row.midLon))
}

export const policy: NationalRoadLinePolicy = {
  country: 'PY',
  bbox: PY_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 'py/roads-mopc.geojson', sha256: 'a40c46183c87b3f7136e8c0e297b846fb85392d718136e977cb3832db8a170aa' },
  ],
  radiusMetres: 500,
  acceptLine: line => true,
  traffic,
}
