/** Load and match Peru's pinned MTC national and departmental road sources. */

import { buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine, type PinnedRoadLine } from './pinned-road-lines.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import type { RoadRow } from './roads-arrow.js'
import { inBbox } from './spatial.js'

const TIER1_CITIES: readonly [number, number, number, number][] = [[-12.30, -77.20, -11.80, -76.80]]
const TIER2_CITIES: readonly [number, number, number, number][] = [
  [-16.50, -71.65, -16.30, -71.45], [-8.20, -79.10, -8.00, -78.95], [-6.82, -79.92, -6.70, -79.78],
  [-5.25, -80.68, -5.15, -80.58], [-3.80, -73.30, -3.70, -73.20], [-13.60, -72.00, -13.45, -71.85],
  [-9.15, -78.60, -9.05, -78.50], [-12.10, -75.25, -12.00, -75.15], [-18.05, -70.30, -17.95, -70.20],
  [-15.55, -70.15, -15.45, -70.05], [-14.15, -75.80, -14.00, -75.65], [-7.20, -78.55, -7.10, -78.45],
  [-8.45, -74.60, -8.35, -74.50], [-4.92, -80.72, -4.85, -80.65], [-13.20, -74.27, -13.13, -74.20],
  [-13.47, -76.15, -13.40, -76.08], [-9.98, -76.27, -9.90, -76.20], [-6.52, -76.40, -6.45, -76.33],
  [-15.85, -70.05, -15.78, -69.98], [-3.60, -80.48, -3.53, -80.42], [-9.56, -77.56, -9.48, -77.50],
  [-5.73, -78.82, -5.67, -78.78], [-11.12, -77.63, -11.05, -77.57], [-13.73, -76.22, -13.68, -76.17],
]
const MINING_REGIONS: readonly [number, number, number, number][] = [
  [-18.4, -73.0, -15.5, -70.0], [-11.0, -77.8, -8.5, -76.0], [-8.0, -79.0, -6.0, -77.5],
  [-15.0, -73.5, -13.0, -71.5],
]
const FILES = {
  national: { relativePath: 'pe/roads-nacional-mtc.geojson', sha256: '2e03a336396602262cc879cf335ddf43f5efbbfeacb4c5d7ccde3627fb93427e' },
  departmental: { relativePath: 'pe/roads-departamental.geojson', sha256: '86ae74ddbfd5d6b00dd477b0d297158ee42ccb8af41725ac7aaa491c93b480a7' },
} as const

export interface PeruRoadSource {
  roads: ReturnType<typeof buildRoadLineVertexGrid>
  sourceRows: number
  sourceLines: number
  invalidGeometrySkipped: number
  observedTrafficLines: number
}

const text = (line: PinnedRoadLine, key: string): string => String(line.properties[key] ?? '').trim()
const imd = (line: PinnedRoadLine): number => {
  const value = line.properties.dIMD
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : 0
}

export function loadPeruRoadSource(options: RoadLoaderArguments): PeruRoadSource {
  const national = loadPinnedRoadLines(options, [FILES.national])
  const departmental = loadPinnedRoadLines(options, [FILES.departmental])
  return {
    roads: buildRoadLineVertexGrid([...national.lines, ...departmental.lines]),
    sourceRows: national.sourceRows + departmental.sourceRows,
    sourceLines: national.lines.length + departmental.lines.length,
    invalidGeometrySkipped: national.invalidGeometrySkipped + departmental.invalidGeometrySkipped,
    observedTrafficLines: national.lines.filter(line => imd(line) > 0).length,
  }
}

function cityTier(latitude: number, longitude: number): 0 | 1 | 2 {
  if (TIER1_CITIES.some(bbox => inBbox(latitude, longitude, bbox))) return 1
  return TIER2_CITIES.some(bbox => inBbox(latitude, longitude, bbox)) ? 2 : 0
}
const multiplier = (tier: 0 | 1 | 2): number => tier === 1 ? 2 : tier === 2 ? 1.4 : 1

function peruRegion(latitude: number, longitude: number): 'costa' | 'sierra' | 'selva' {
  if (longitude > -73) return 'selva'
  if (longitude > -75 && latitude < -8) return 'sierra'
  if (longitude > -76 && latitude > -4) return 'selva'
  if (longitude < -77.5 && latitude < -4) return 'costa'
  return longitude >= -77.5 && longitude <= -73 && latitude < -4 ? 'sierra' : 'costa'
}

function classifiedAadt(line: PinnedRoadLine): number {
  const departmental = line.relativePath.endsWith('roads-departamental.geojson')
  const surface = departmental ? text(line, 'SUPERFIC') : text(line, 'cSuperfici')
  if (surface !== '1' && surface !== '11') return 1_200
  const classification = departmental ? 'DEPARTAMENTAL' : text(line, 'cClasifica')
  const concession = /concesi/i.test(text(line, 'cPeajes'))
  if (concession && /LONGITUDINAL DE LA COSTA/i.test(classification)) return 20_000
  if (concession) return 10_000
  if (/LONGITUDINAL DE LA COSTA/i.test(classification)) return 12_000
  if (/LONGITUDINAL DE LA (SIERRA|SELVA)/i.test(classification)) return 6_000
  if (/TRANSVERSAL/i.test(classification)) return 4_000
  return /RAMAL|VARIANTE|DEPARTAMENTAL/i.test(classification) ? 2_500 : 2_000
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, region: 'costa' | 'sierra' | 'selva', mining: boolean) {
  const fractions = tier === 1 ? [0.65, 0.06, 0.14, 0.15] : tier === 2 ? [0.68, 0.06, 0.12, 0.14]
    : mining ? [0.45, 0.08, 0.38, 0.09] : region === 'sierra' ? [0.50, 0.10, 0.30, 0.10]
      : [0.60, 0.08, 0.22, 0.10]
  return { light: Math.round(aadt * fractions[0]), medium: Math.round(aadt * fractions[1]),
    heavy: Math.round(aadt * fractions[2]), moto: Math.round(aadt * fractions[3]) }
}

export function matchPeruRoad(row: RoadRow, source: PeruRoadSource) {
  if (row.roadClass > 2) return null
  const line = nearestRoadLine(row.midLat, row.midLon, source.roads, 500)
  if (!line) return null
  const tier = cityTier(row.midLat, row.midLon)
  const observed = imd(line)
  const traffic = splitVehicles((observed || classifiedAadt(line)) * multiplier(tier), tier,
    peruRegion(row.midLat, row.midLon), tier === 0 && MINING_REGIONS.some(bbox => inBbox(row.midLat, row.midLon, bbox)))
  if (traffic.light + traffic.medium + traffic.heavy + traffic.moto === 0) return null
  return { kind: observed > 0 ? 'imd' as const : 'network' as const, ...traffic }
}

export const PERU_ROAD_BBOX: [number, number, number, number] = [-18.4, -82.0, 0, -68.5]
export const PERU_ROAD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2])
