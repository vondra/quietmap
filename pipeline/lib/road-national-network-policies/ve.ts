/** VE road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const VE_BBOX: [number, number, number, number] = [0.6, -73.4, 12.5, -59.0]

const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Caracas', bbox: [10.40, -67.05, 10.55, -66.75] },
  { name: 'Maracaibo', bbox: [10.58, -71.75, 10.75, -71.55] },
  { name: 'Valencia', bbox: [10.10, -68.10, 10.25, -67.95] },
  { name: 'Barquisimeto', bbox: [9.97, -69.40, 10.10, -69.25] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Ciudad Guayana', bbox: [8.25, -62.80, 8.40, -62.55] },
  { name: 'Maracay',      bbox: [10.22, -67.65, 10.33, -67.50] },
  { name: 'Maturín',      bbox: [9.72, -63.22, 9.82, -63.12] },
  { name: 'Barcelona',    bbox: [10.10, -64.75, 10.18, -64.65] },
  { name: 'Puerto La Cruz', bbox: [10.18, -64.72, 10.25, -64.60] },
  { name: 'San Cristóbal', bbox: [7.73, -72.28, 7.82, -72.18] },
  { name: 'Cumaná',       bbox: [10.42, -64.22, 10.50, -64.13] },
  { name: 'Mérida',       bbox: [8.57, -71.18, 8.65, -71.10] },
  { name: 'Ciudad Bolívar', bbox: [8.10, -63.60, 8.20, -63.50] },
  { name: 'Cabimas',      bbox: [10.35, -71.48, 10.45, -71.40] },
  { name: 'Coro',         bbox: [11.38, -69.70, 11.45, -69.63] },
  { name: 'Los Teques',   bbox: [10.32, -67.07, 10.38, -66.98] },
  { name: 'Guarenas',     bbox: [10.45, -66.65, 10.50, -66.58] },
  { name: 'Guanare',      bbox: [9.02, -69.77, 9.07, -69.70] },
  { name: 'Valera',       bbox: [9.30, -70.63, 9.35, -70.57] },
  { name: 'Punto Fijo',   bbox: [11.67, -70.22, 11.72, -70.17] },
  { name: 'Acarigua',     bbox: [9.53, -69.22, 9.58, -69.17] },
  { name: 'Barinas',      bbox: [8.60, -70.25, 8.67, -70.18] },
  { name: 'San Felipe',   bbox: [10.32, -68.77, 10.37, -68.72] },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.60),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.25),
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
  // Rural — Venezuelan economy has collapsed, fewer trucks
  return {
    light: Math.round(aadt * 0.58),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.12),
  }
}


const text = (line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string =>
  String(line.properties[name] ?? '').trim()

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
    const type = text(line, 'Tipo_vía').toLowerCase()
    const base = /autopista/.test(type) ? 22000 : /pavimentada.*\+\s*2/.test(type) ? 12000
      : /pavimentada/.test(type) ? 8000 : /engranzonada.*\+\s*2/.test(type) ? 5000
      : /engranzonada/.test(type) ? 3000 : /camino.*sendero/.test(type) ? 1000
      : /camino/.test(type) ? 1800 : /tierra/.test(type) ? 1500 : /sendero/.test(type) ? 500 : 2000
    return splitVehicles(base * tierMultiplier(tier), tier)
}

export const policy: NationalRoadLinePolicy = {
  country: 'VE',
  bbox: VE_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 've/roads-vialidad.geojson', sha256: 'f1fd6a82cd1851f3ba61888ed51b5e6f7e6bdb3e8df8d14ee35283b001d154b8' },
  ],
  radiusMetres: 400,
  acceptLine: line => true,
  traffic,
}
