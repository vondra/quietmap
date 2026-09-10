/** Load and match Chile's pinned TMDA stations and Red Vial classifications. */

import { buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine, type PinnedRoadLine } from './pinned-road-lines.js'
import { readPinnedRoadSource } from './pinned-road-source.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import type { RoadRow } from './roads-arrow.js'
import { buildOneHundredthDegreePointGrid, flatDist, inBbox, pointGridCandidates, type PointCoordinates } from './spatial.js'

const CL_BBOX: [number, number, number, number] = [-56.0, -76.0, -17.5, -66.4]

// Chile is very narrow (~180 km wide max). Border with Argentina is along
// the Andes crest, roughly at lon -67.5 in northern Chile and bending west
// toward -71 in southern Patagonia.
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Santiago (Gran Santiago)', bbox: [-33.65, -70.85, -33.30, -70.45] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Valparaíso',     bbox: [-33.10, -71.65, -32.95, -71.45] },
  { name: 'Viña del Mar',   bbox: [-33.05, -71.60, -32.95, -71.45] },
  { name: 'Concepción',     bbox: [-37.00, -73.10, -36.65, -72.85] },
  { name: 'Talcahuano',     bbox: [-36.78, -73.15, -36.65, -73.00] },
  { name: 'La Serena',      bbox: [-30.05, -71.30, -29.85, -71.20] },
  { name: 'Coquimbo',       bbox: [-29.98, -71.40, -29.88, -71.30] },
  { name: 'Antofagasta',    bbox: [-23.75, -70.50, -23.55, -70.35] },
  { name: 'Iquique',        bbox: [-20.30, -70.20, -20.15, -70.10] },
  { name: 'Arica',          bbox: [-18.55, -70.35, -18.40, -70.20] },
  { name: 'Temuco',         bbox: [-38.78, -72.65, -38.65, -72.55] },
  { name: 'Rancagua',       bbox: [-34.25, -70.80, -34.10, -70.65] },
  { name: 'Talca',          bbox: [-35.50, -71.70, -35.35, -71.60] },
  { name: 'Chillán',        bbox: [-36.65, -72.15, -36.55, -72.05] },
  { name: 'Puerto Montt',   bbox: [-41.55, -73.05, -41.40, -72.90] },
  { name: 'Osorno',         bbox: [-40.65, -73.20, -40.55, -73.10] },
  { name: 'Valdivia',       bbox: [-39.90, -73.30, -39.75, -73.20] },
  { name: 'Calama',         bbox: [-22.55, -69.00, -22.40, -68.85] },
  { name: 'Copiapó',        bbox: [-27.45, -70.40, -27.30, -70.25] },
  { name: 'Punta Arenas',   bbox: [-53.20, -70.95, -53.05, -70.85] },
  { name: 'Curicó',         bbox: [-35.00, -71.30, -34.93, -71.20] },
  { name: 'Los Ángeles',    bbox: [-37.50, -72.40, -37.40, -72.30] },
  { name: 'San Antonio',    bbox: [-33.65, -71.65, -33.55, -71.55] },
  { name: 'Quillota',       bbox: [-32.93, -71.30, -32.85, -71.20] },
  { name: 'Tomé',           bbox: [-36.65, -72.97, -36.58, -72.90] },
]

// Mining region (Antofagasta/Tarapacá/Atacama Norte) — very high HGV share
const MINING_REGION_BBOX: [number, number, number, number] = [-30.0, -71.5, -17.5, -66.4]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}
function inMiningRegion(lat: number, lon: number): boolean {
  return inBbox(lat, lon, MINING_REGION_BBOX)
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, mining: boolean): { light: number; medium: number; heavy: number; moto: number } {
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
  if (mining) {
    // Mining region (Antofagasta/Tarapacá/Atacama) — extreme HGV share
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.38),
      moto: Math.round(aadt * 0.02),
    }
  }
  // Rural — high heavy share for grain/forestry/freight
  return {
    light: Math.round(aadt * 0.60),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.27),
    moto: Math.round(aadt * 0.03),
  }
}

interface TmdaPoint extends PointCoordinates { aadt: number }
export interface ChileRoadSource {
  network: ReturnType<typeof buildRoadLineVertexGrid>
  tmda: ReturnType<typeof buildOneHundredthDegreePointGrid<TmdaPoint>>
  sourceRows: number
  sourceLines: number
  tmdaPoints: number
  invalidGeometrySkipped: number
  unavailableTrafficSkipped: number
}

function tmdaPoint(value: unknown): TmdaPoint | null {
  if (!value || typeof value !== 'object') return null
  const feature = value as { geometry?: { type?: unknown; coordinates?: unknown }; properties?: unknown }
  const geometry = feature.geometry
  let point: unknown
  if (geometry?.type === 'Point') point = geometry.coordinates
  else if ((geometry?.type === 'MultiPoint' || geometry?.type === 'LineString') && Array.isArray(geometry.coordinates)) point = geometry.coordinates[0]
  else if (geometry?.type === 'MultiLineString' && Array.isArray(geometry.coordinates) && Array.isArray(geometry.coordinates[0])) point = geometry.coordinates[0][0]
  if (!Array.isArray(point) || !Number.isFinite(Number(point[0])) || !Number.isFinite(Number(point[1]))) return null
  const properties = feature.properties && typeof feature.properties === 'object' && !Array.isArray(feature.properties)
    ? feature.properties as Record<string, unknown> : {}
  const aadt = Math.max(...[1, 2, 3, 4].map(index => Number(properties[`TMDA_RAMA_${index}`]) || 0))
  return aadt >= 50 ? { longitude: Number(point[0]), latitude: Number(point[1]), aadt } : null
}

export function loadChileRoadSource(options: RoadLoaderArguments): ChileRoadSource {
  const network = loadPinnedRoadLines(options, [{
    relativePath: 'cl/roads-network.geojson', sha256: '9a97f12bccbf9f790ece976be9f3ead13b19bc6bc3ba0de1d8ea7cd901effc17',
  }])
  const raw = JSON.parse(readPinnedRoadSource(options, 'cl/tmda-2024.geojson',
    '5d094578be96bc174a0cf4efd5c3c739b4c0292d9aac53feb4820ac0f08ab832').toString('utf8')) as { features?: unknown[] }
  if (!Array.isArray(raw.features) || raw.features.length === 0) throw new Error('Chile TMDA source has no features')
  const points = raw.features.map(tmdaPoint).filter((point): point is TmdaPoint => point !== null)
  if (points.length === 0) throw new Error('Chile TMDA source has no usable observations')
  return { network: buildRoadLineVertexGrid(network.lines), tmda: buildOneHundredthDegreePointGrid(points),
    sourceRows: network.sourceRows + raw.features.length, sourceLines: network.lines.length,
    tmdaPoints: points.length, invalidGeometrySkipped: network.invalidGeometrySkipped,
    unavailableTrafficSkipped: raw.features.length - points.length }
}

const text = (line: PinnedRoadLine, key: string): string => String(line.properties[key] ?? '').trim()
function networkAadt(line: PinnedRoadLine): number {
  if (!/asfalto|hormigon|pavi|pav/i.test(text(line, 'CARPETA'))) return 1_500
  if (/si|true|1/i.test(text(line, 'CONCESIONADO'))) return 35_000
  const roadClass = text(line, 'CLASIFICACION')
  return /longitudinal/i.test(roadClass) ? 22_000 : /nacional/i.test(roadClass) ? 14_000
    : /principal/i.test(roadClass) ? 7_000 : /provincial/i.test(roadClass) ? 3_500 : 2_000
}
function nearestTmda(latitude: number, longitude: number, source: ChileRoadSource): TmdaPoint | null {
  let nearest: TmdaPoint | null = null, distance = 600
  for (const point of pointGridCandidates(latitude, longitude, distance, source.tmda)) {
    const candidate = flatDist(latitude, longitude, point.latitude, point.longitude)
    if (candidate < distance) { nearest = point; distance = candidate }
  }
  return nearest
}

export function matchChileRoad(row: RoadRow, source: ChileRoadSource) {
  if (row.roadClass > 2) return null
  const tier = cityTier(row.midLat, row.midLon), multiplier = tierMultiplier(tier)
  const station = nearestTmda(row.midLat, row.midLon, source)
  let total: number, kind: 'tmda' | 'network'
  if (station) { total = station.aadt * multiplier; kind = 'tmda' }
  else {
    const line = nearestRoadLine(row.midLat, row.midLon, source.network, 400)
    if (!line) return null
    total = networkAadt(line) * multiplier; kind = 'network'
  }
  return { kind, ...splitVehicles(total, tier, inMiningRegion(row.midLat, row.midLon) && tier === 0) }
}

export const CHILE_ROAD_BBOX = CL_BBOX
export const CHILE_ROAD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2])
