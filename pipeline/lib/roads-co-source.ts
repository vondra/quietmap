/** Load and match Colombia's pinned INVIAS traffic and road-network sources. */

import { buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine, type PinnedRoadLine } from './pinned-road-lines.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import type { RoadRow } from './roads-arrow.js'
import { inBbox } from './spatial.js'

const TIER1_CITIES: readonly [number, number, number, number][] = [
  [4.50, -74.20, 4.85, -73.95], [6.10, -75.65, 6.35, -75.50],
]
const TIER2_CITIES: readonly [number, number, number, number][] = [
  [3.30, -76.60, 3.55, -76.45], [10.90, -74.85, 11.05, -74.75], [10.35, -75.55, 10.50, -75.45],
  [7.85, -72.55, 8.00, -72.45], [7.05, -73.15, 7.20, -73.05], [4.78, -75.75, 4.88, -75.65],
  [11.20, -74.25, 11.30, -74.15], [4.40, -75.25, 4.50, -75.15], [5.03, -75.55, 5.12, -75.45],
  [1.18, -77.30, 1.25, -77.25], [4.10, -73.65, 4.20, -73.60], [2.90, -75.30, 2.97, -75.25],
  [4.50, -75.70, 4.55, -75.65], [10.88, -74.80, 10.95, -74.74], [4.55, -74.25, 4.65, -74.15],
  [10.45, -73.27, 10.50, -73.22], [8.73, -75.92, 8.80, -75.85], [9.27, -75.42, 9.32, -75.36],
  [3.85, -77.05, 3.90, -76.97], [5.52, -73.40, 5.58, -73.35], [11.50, -72.95, 11.57, -72.88],
  [5.67, -76.68, 5.72, -76.62], [1.59, -75.62, 1.64, -75.58], [2.42, -76.62, 2.47, -76.57],
]
const COAL_REGIONS: readonly [number, number, number, number][] = [
  [10.50, -73.00, 11.80, -72.00], [9.50, -73.85, 10.50, -73.20],
]
const FILES = {
  network: { relativePath: 'co/roads-network.geojson', sha256: 'c23487ab8b230f91416288db6d17e52adbbe62db30fca38b78d19feca751c82a' },
  tpda: { relativePath: 'co/tpda-2024.geojson', sha256: 'dcac31c9238b47113d8811fe333b30c0f9ec5ffb012fc7e653bef30797ca8577' },
} as const

export interface ColombiaRoadSource {
  network: ReturnType<typeof buildRoadLineVertexGrid>
  tpda: ReturnType<typeof buildRoadLineVertexGrid>
  sourceRows: number
  sourceLines: number
  invalidGeometrySkipped: number
  unavailableTrafficRows: number
}

const number = (line: PinnedRoadLine, key: string, fallback = 0): number => {
  const value = line.properties[key]
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}
const text = (line: PinnedRoadLine, key: string): string => String(line.properties[key] ?? '').trim()
const tpdaAadt = (line: PinnedRoadLine): number => number(line, 'conteo')

export function loadColombiaRoadSource(options: RoadLoaderArguments): ColombiaRoadSource {
  const network = loadPinnedRoadLines(options, [FILES.network])
  const tpda = loadPinnedRoadLines(options, [FILES.tpda])
  const usableTpda = tpda.lines.filter(line => tpdaAadt(line) >= 50)
  return {
    network: buildRoadLineVertexGrid(network.lines),
    tpda: buildRoadLineVertexGrid(usableTpda),
    sourceRows: network.sourceRows + tpda.sourceRows,
    sourceLines: network.lines.length + tpda.lines.length,
    invalidGeometrySkipped: network.invalidGeometrySkipped + tpda.invalidGeometrySkipped,
    unavailableTrafficRows: tpda.lines.length - usableTpda.length,
  }
}

function cityTier(latitude: number, longitude: number): 0 | 1 | 2 {
  if (TIER1_CITIES.some(bbox => inBbox(latitude, longitude, bbox))) return 1
  return TIER2_CITIES.some(bbox => inBbox(latitude, longitude, bbox)) ? 2 : 0
}
const multiplier = (tier: 0 | 1 | 2): number => tier === 1 ? 2 : tier === 2 ? 1.4 : 1

function networkAadt(line: PinnedRoadLine): number {
  if (text(line, 'superficie') !== '1') return 1_500
  if (text(line, 'administrador') === '2' && text(line, 'calzada') === '2') return 25_000
  if (text(line, 'administrador') === '2') return 18_000
  return text(line, 'administrador') === '1' ? 12_000 : 6_000
}

function observedSplit(aadt: number, line: PinnedRoadLine) {
  const cars = number(line, 'au_p', 65), buses = number(line, 'bu_p', 5), trucks = number(line, 'ca_p', 30)
  const scale = cars + buses + trucks > 0 ? 95 / (cars + buses + trucks) : 1
  return { light: Math.round(aadt * cars * scale / 100), medium: Math.round(aadt * buses * scale / 100),
    heavy: Math.round(aadt * trucks * scale / 100), moto: Math.round(aadt * 0.05) }
}

function defaultSplit(aadt: number, tier: 0 | 1 | 2, coal: boolean) {
  const fractions = tier === 1 ? [0.55, 0.05, 0.10, 0.30] : tier === 2 ? [0.60, 0.05, 0.10, 0.25]
    : coal ? [0.40, 0.05, 0.45, 0.10] : [0.55, 0.08, 0.22, 0.15]
  return { light: Math.round(aadt * fractions[0]), medium: Math.round(aadt * fractions[1]),
    heavy: Math.round(aadt * fractions[2]), moto: Math.round(aadt * fractions[3]) }
}

export function matchColombiaRoad(row: RoadRow, source: ColombiaRoadSource) {
  if (row.roadClass > 2) return null
  const tier = cityTier(row.midLat, row.midLon)
  const observed = nearestRoadLine(row.midLat, row.midLon, source.tpda, 500)
  if (observed) return { kind: 'tpda' as const, ...observedSplit(tpdaAadt(observed) * multiplier(tier), observed) }
  const network = nearestRoadLine(row.midLat, row.midLon, source.network, 400)
  if (!network) return null
  const coal = tier === 0 && COAL_REGIONS.some(bbox => inBbox(row.midLat, row.midLon, bbox))
  return { kind: 'network' as const, ...defaultSplit(networkAadt(network) * multiplier(tier), tier, coal) }
}

export const COLOMBIA_ROAD_BBOX: [number, number, number, number] = [-4.3, -82.0, 13.5, -66.8]
export const COLOMBIA_ROAD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2])
