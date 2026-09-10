/** EC road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const EC_BBOX: [number, number, number, number] = [-5.0, -81.1, 1.5, -75.0]

// Prepared-square scan bbox = the same Ecuador mainland box, so the prepared-square listing skips the
// rest of the planet instead of reading every roads.arrow on Earth. [minLat,minLon,maxLat,maxLon]
const EC_SCAN_BBOX: [number, number, number, number] = [-5.0, -81.1, 1.5, -75.0]

const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Quito D.M.', bbox: [-0.40, -78.60, -0.05, -78.40] },
  { name: 'Guayaquil', bbox: [-2.35, -80.00, -2.05, -79.75] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Cuenca',          bbox: [-2.95, -79.05, -2.83, -78.93] },
  { name: 'Santo Domingo',   bbox: [-0.30, -79.22, -0.20, -79.12] },
  { name: 'Machala',         bbox: [-3.30, -79.98, -3.22, -79.88] },
  { name: 'Durán',           bbox: [-2.22, -79.90, -2.12, -79.80] },
  { name: 'Manta',           bbox: [-0.98, -80.76, -0.90, -80.67] },
  { name: 'Portoviejo',      bbox: [-1.08, -80.48, -1.00, -80.40] },
  { name: 'Ambato',          bbox: [-1.30, -78.64, -1.22, -78.58] },
  { name: 'Loja',            bbox: [-4.05, -79.22, -3.97, -79.17] },
  { name: 'Riobamba',        bbox: [-1.70, -78.68, -1.63, -78.62] },
  { name: 'Esmeraldas',      bbox: [0.94, -79.70, 1.00, -79.63] },
  { name: 'Ibarra',          bbox: [0.32, -78.14, 0.38, -78.08] },
  { name: 'Latacunga',       bbox: [-0.97, -78.63, -0.91, -78.58] },
  { name: 'Milagro',         bbox: [-2.15, -79.63, -2.08, -79.56] },
  { name: 'Babahoyo',        bbox: [-1.83, -79.55, -1.77, -79.48] },
  { name: 'Quevedo',         bbox: [-1.04, -79.50, -0.97, -79.44] },
  { name: 'Lago Agrio',      bbox: [0.07, -76.92, 0.13, -76.86] },
  { name: 'Tulcán',          bbox: [0.79, -77.74, 0.84, -77.70] },
]

// Oil freight corridor (Amazon → SOTE → OCP → Esmeraldas/Balao)
const OIL_ROUTE_BBOXES: Array<[number, number, number, number]> = [
  [0.0, -77.5, 1.0, -76.0],   // Sucumbíos oil fields
  [-0.5, -78.0, 0.5, -77.5],  // SOTE corridor
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}
function inOilRoute(lat: number, lon: number): boolean {
  for (const b of OIL_ROUTE_BBOXES) if (inBbox(lat, lon, b)) return true
  return false
}

// EC tri-regional classifier
function ecRegion(lat: number, lon: number): 'costa' | 'sierra' | 'oriente' {
  // Oriente: east of ~-78 (Amazon basin)
  if (lon > -78.0) return 'oriente'
  // Sierra: -78.5 to -79 longitude in the Andes spine, roughly
  if (lon > -79.5 && lon < -77.8) return 'sierra'
  // Costa: west of -79.5
  return 'costa'
}

// Local to EC: census `feat.coords` always has ≥2 vertices (loadRoads drops shorter),
// so this matches the shared spatial.pointToPolylineDist on every call site here.

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, region: string, oilRoute: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.67),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.15),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.68),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.14),
    }
  }
  if (oilRoute) {
    // Oil freight route — extreme heavy share
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.37),
      moto: Math.round(aadt * 0.10),
    }
  }
  if (region === 'sierra') {
    return {
      light: Math.round(aadt * 0.55),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.27),
      moto: Math.round(aadt * 0.10),
    }
  }
  if (region === 'oriente') {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.32),
      moto: Math.round(aadt * 0.10),
    }
  }
  // Costa rural
  return {
    light: Math.round(aadt * 0.60),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.10),
  }
}

const text = (line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string =>
  String(line.properties[name] ?? '').trim()

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
    const isState = !line.relativePath.endsWith('roads-all.geojson')
    const clasif = text(line, line.relativePath.endsWith('roads-estatal.geojson') ? 'CLASIFICAC'
      : line.relativePath.endsWith('roads-ant2030.geojson') ? 'CLASE_VIA' : 'TIPO_VIA').toUpperCase()
    const tipo = text(line, line.relativePath.endsWith('roads-ant2030.geojson') ? 'TIPO_DE_SU'
      : line.relativePath.endsWith('roads-all.geojson') ? 'TIPO_CALZA' : '').toUpperCase()
    const unpaved = /LASTRE|TIERRA|EMPEDR|LASTRA/.test(tipo)
    const base = unpaved ? (isState ? 2000 : 1200) : isState
      ? (/ARTERIAL/.test(clasif) ? 15000 : /COLECTORA/.test(clasif) ? 6000 : 10000)
      : 4000
    return splitVehicles(base * tierMultiplier(tier), tier, ecRegion(row.midLat, row.midLon),
      inOilRoute(row.midLat, row.midLon))
}

export const policy: NationalRoadLinePolicy = {
  country: 'EC',
  bbox: EC_SCAN_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 'ec/roads-estatal.geojson', sha256: '8b6d56ed27cd8e47bbba2565b9ff62a37cfeb27a782a200fb364461940df52e5' },
    { relativePath: 'ec/roads-ant2030.geojson', sha256: 'd9de26e2b91053bb74dae71bc69e2833af500eb12f7d155c700a2b554ca28c85' },
    { relativePath: 'ec/roads-all.geojson', sha256: '4a2cff30e2a0619615690be6e2acf9eac963dad20a9e02c177fdff0df0e34cab' },
  ],
  radiusMetres: 400,
  acceptLine: line => true,
  traffic,
}
