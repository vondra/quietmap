/** Load and match Argentina's pinned TMDA observations and DNV road classifications. */

import { buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine, type PinnedRoadLine } from './pinned-road-lines.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import type { RoadRow } from './roads-arrow.js'
import { inBbox } from './spatial.js'

const AR_BBOX: [number, number, number, number] = [-55.5, -73.6, -21.7, -53.6]

// Exclusion zones for neighbours
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Buenos Aires (Gran BA)', bbox: [-35.05, -58.95, -34.30, -58.20] },
  { name: 'Córdoba', bbox: [-31.55, -64.32, -31.28, -64.05] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Rosario',         bbox: [-33.05, -60.78, -32.85, -60.55] },
  { name: 'Mendoza',         bbox: [-33.00, -68.92, -32.80, -68.72] },
  { name: 'San Miguel de Tucumán', bbox: [-26.92, -65.30, -26.75, -65.10] },
  { name: 'La Plata',        bbox: [-35.00, -58.05, -34.85, -57.85] },
  { name: 'Mar del Plata',   bbox: [-38.10, -57.65, -37.90, -57.45] },
  { name: 'Salta',           bbox: [-24.85, -65.50, -24.70, -65.30] },
  { name: 'Santa Fe',        bbox: [-31.70, -60.78, -31.55, -60.60] },
  { name: 'San Juan',        bbox: [-31.60, -68.60, -31.45, -68.45] },
  { name: 'Resistencia',     bbox: [-27.55, -59.10, -27.40, -58.95] },
  { name: 'Neuquén',         bbox: [-39.00, -68.20, -38.90, -68.00] },
  { name: 'Bahía Blanca',    bbox: [-38.78, -62.35, -38.65, -62.18] },
  { name: 'Posadas',         bbox: [-27.42, -56.00, -27.32, -55.85] },
  { name: 'Corrientes',      bbox: [-27.55, -58.90, -27.40, -58.75] },
  { name: 'Paraná',          bbox: [-31.78, -60.60, -31.65, -60.45] },
  { name: 'Santiago del Estero', bbox: [-27.85, -64.32, -27.72, -64.18] },
  { name: 'San Salvador de Jujuy', bbox: [-24.25, -65.35, -24.15, -65.20] },
  { name: 'Río Cuarto',      bbox: [-33.18, -64.40, -33.05, -64.25] },
  { name: 'Comodoro Rivadavia', bbox: [-45.92, -67.55, -45.78, -67.40] },
  { name: 'San Luis',        bbox: [-33.35, -66.40, -33.22, -66.25] },
  { name: 'La Rioja',        bbox: [-29.48, -66.92, -29.35, -66.78] },
  { name: 'Catamarca',       bbox: [-28.55, -65.85, -28.40, -65.70] },
  { name: 'Formosa',         bbox: [-26.25, -58.25, -26.12, -58.10] },
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
      light: Math.round(aadt * 0.75),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.05),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.73),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.05),
    }
  }
  // Rural — high heavy share for grain/freight/oil corridors
  return {
    light: Math.round(aadt * 0.60),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.27),
    moto: Math.round(aadt * 0.03),
  }
}

const FILES = {
  national: { relativePath: 'ar/roads-national.geojson', sha256: '520cb6d43b81974786eebb765e00a3868cadaaeb12768bbe69b1776ae88dd658' },
  provincial: { relativePath: 'ar/roads-provincial.geojson', sha256: 'd6aec55afc604086f9578d265464d519a891b1339ee23e1c2476441247240a6c' },
  tmda: { relativePath: 'ar/tmda-2017-18.geojson', sha256: '7e1cda4adb6a92d45baa0308424f0fd55006adcf680950614441d676404850d8' },
} as const

export interface ArgentinaRoadSource {
  dnv: ReturnType<typeof buildRoadLineVertexGrid>
  tmda: ReturnType<typeof buildRoadLineVertexGrid>
  sourceRows: number
  sourceLines: number
  invalidGeometrySkipped: number
  unavailableTrafficRows: number
}

export function loadArgentinaRoadSource(options: RoadLoaderArguments): ArgentinaRoadSource {
  const national = loadPinnedRoadLines(options, [FILES.national])
  const provincial = loadPinnedRoadLines(options, [FILES.provincial])
  const tmda = loadPinnedRoadLines(options, [FILES.tmda])
  const usableTmda = tmda.lines.filter(line => tmdaAadt(line) >= 50)
  return {
    dnv: buildRoadLineVertexGrid([...national.lines, ...provincial.lines]),
    tmda: buildRoadLineVertexGrid(usableTmda),
    sourceRows: national.sourceRows + provincial.sourceRows + tmda.sourceRows,
    sourceLines: national.lines.length + provincial.lines.length + tmda.lines.length,
    invalidGeometrySkipped: national.invalidGeometrySkipped + provincial.invalidGeometrySkipped + tmda.invalidGeometrySkipped,
    unavailableTrafficRows: tmda.lines.length - usableTmda.length,
  }
}

const text = (line: PinnedRoadLine, key: string): string => String(line.properties[key] ?? '').trim()
function tmdaAadt(line: PinnedRoadLine): number {
  const value = Number(line.properties.valor ?? line.properties.tmda17 ?? 0)
  return Number.isFinite(value) ? value : 0
}
function dnvAadt(line: PinnedRoadLine, national: boolean): number {
  if (!/pavi/i.test(text(line, 'tipo_de_superficie_de_via'))) return 3_000
  return /autopista|autov/i.test(text(line, 'tipo_de_via_de_transporte')) ? 30_000 : national ? 18_000 : 12_000
}

export function matchArgentinaRoad(row: RoadRow, source: ArgentinaRoadSource) {
  if (row.roadClass > 2) return null
  const tier = cityTier(row.midLat, row.midLon)
  const multiplier = tierMultiplier(tier)
  const observed = nearestRoadLine(row.midLat, row.midLon, source.tmda, 300)
  let total: number, kind: 'tmda' | 'dnv-national' | 'dnv-provincial'
  if (observed) { total = tmdaAadt(observed) * multiplier; kind = 'tmda' }
  else {
    const classified = nearestRoadLine(row.midLat, row.midLon, source.dnv, 400)
    if (!classified) return null
    const national = classified.relativePath.endsWith('roads-national.geojson')
    total = dnvAadt(classified, national) * multiplier
    kind = national ? 'dnv-national' : 'dnv-provincial'
  }
  return { kind, ...splitVehicles(total, tier) }
}

export const ARGENTINA_ROAD_BBOX = AR_BBOX
export const ARGENTINA_ROAD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2])
