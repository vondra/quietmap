/** BO road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const BO_BBOX: [number, number, number, number] = [-22.9, -69.7, -9.5, -57.5]

const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'La Paz / El Alto', bbox: [-16.58, -68.35, -16.30, -68.00] },
  { name: 'Santa Cruz de la Sierra', bbox: [-17.90, -63.30, -17.70, -63.05] },
  { name: 'Cochabamba', bbox: [-17.45, -66.30, -17.32, -66.10] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Sucre',       bbox: [-19.08, -65.30, -19.00, -65.22] },
  { name: 'Oruro',       bbox: [-17.99, -67.17, -17.93, -67.10] },
  { name: 'Tarija',      bbox: [-21.55, -64.75, -21.50, -64.68] },
  { name: 'Potosí',      bbox: [-19.61, -65.78, -19.55, -65.72] },
  { name: 'Trinidad',    bbox: [-14.85, -64.93, -14.80, -64.87] },
  { name: 'Cobija',      bbox: [-11.03, -68.77, -11.00, -68.72] },
  { name: 'Riberalta',   bbox: [-11.02, -66.08, -10.97, -66.02] },
  { name: 'Montero',     bbox: [-17.37, -63.27, -17.32, -63.22] },
  { name: 'Quillacollo', bbox: [-17.42, -66.32, -17.37, -66.27] },
  { name: 'Sacaba',      bbox: [-17.42, -66.05, -17.37, -65.98] },
  { name: 'Warnes',      bbox: [-17.52, -63.19, -17.47, -63.13] },
  { name: 'Yacuiba',     bbox: [-22.03, -63.73, -21.98, -63.67] },
  { name: 'Camiri',      bbox: [-20.06, -63.54, -20.01, -63.48] },
  { name: 'Villazón',    bbox: [-22.10, -65.62, -22.05, -65.57] },
  { name: 'Viacha',      bbox: [-16.67, -68.32, -16.63, -68.27] },
  { name: 'Uyuni',       bbox: [-20.48, -66.85, -20.45, -66.80] },
]

// Mining corridors — extreme HGV share
const MINING_CORRIDORS: Array<[number, number, number, number]> = [
  // Oruro/Uyuni/Villazón (western mining spine to Chilean ports)
  [-22.5, -68.5, -17.0, -66.0],
  // Potosí → Uyuni → Chile border (via Ruta 5)
  [-21.5, -68.0, -19.0, -65.5],
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}
function inMiningCorridor(lat: number, lon: number): boolean {
  for (const b of MINING_CORRIDORS) if (inBbox(lat, lon, b)) return true
  return false
}

// Bolivia tri-regional classifier
function boRegion(lat: number, lon: number): 'altiplano' | 'valles' | 'llanos' {
  // Llanos: east of -65, north/east of Cochabamba valleys
  if (lon > -65.0) return 'llanos'
  // Altiplano: west of -67.5 (La Paz/Oruro/Potosí highlands)
  if (lon < -67.0) return 'altiplano'
  // Valles: in between (Cochabamba, Sucre, Tarija)
  return 'valles'
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, region: string, mining: boolean): { light: number; medium: number; heavy: number; moto: number } {
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
  if (mining) {
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.37),
      moto: Math.round(aadt * 0.10),
    }
  }
  if (region === 'altiplano') {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.30),
      moto: Math.round(aadt * 0.12),
    }
  }
  if (region === 'llanos') {
    // Santa Cruz soy freight
    return {
      light: Math.round(aadt * 0.55),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.25),
      moto: Math.round(aadt * 0.12),
    }
  }
  // Valles
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
    const surface = text(line, line.relativePath.endsWith('roads-rvf.geojson') ? 'rodadura' : 'Revestimie').toUpperCase()
    const base = /PAVIMENT|ASFAL|HORMIG/.test(surface) ? 15000 : /URBAN/.test(surface) ? 12000
      : /RIPIO|CONSTRUCCIO/.test(surface) ? 5000 : /TIERRA/.test(surface) ? 2000 : 6000
    return splitVehicles(base * tierMultiplier(tier), tier, boRegion(row.midLat, row.midLon),
      inMiningCorridor(row.midLat, row.midLon) && tier === 0)
}

export const policy: NationalRoadLinePolicy = {
  country: 'BO',
  bbox: BO_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 'bo/roads-rvf.geojson', sha256: '3af344326140323216f4e39e6426ac3e52ee22c7d74556cee59d710d80af682e' },
    { relativePath: 'bo/roads-wcs.geojson', sha256: 'a7c53ec91f9e5daa8d81391dc594df31a1ecec33eb0bcc324481469d5ee0d3f8' },
  ],
  radiusMetres: 500,
  acceptLine: line => true,
  traffic,
}
