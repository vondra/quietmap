/** Load and match Saudi Arabia's pinned MoT counts, Riyadh PMS and national atlas. */

import { parse } from 'csv-parse/sync'
import { buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine, type PinnedRoadLine } from './pinned-road-lines.js'
import { readPinnedRoadSource } from './pinned-road-source.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE, type RoadRow } from './roads-arrow.js'
import { inBbox } from './spatial.js'

const MOT_FILE = { relativePath: 'sa/mot-traffic-density.csv', sha256: 'ac80f4557a5aede318230c826b809a4a77796c6b32d0ed5bc2c4ad18e7cf1339' } as const
const RIYADH_FILE = { relativePath: 'sa/riyadh-pms-lanes.geojson', sha256: '276e1c50479c305856e1fa222f31abb9205d6993069ae2e13ff1f47af747a9d7' } as const
const ATLAS_FILE = { relativePath: 'sa/sau-atlas-roads.geojson', sha256: '78a38b129038aaf607ef490fd9e9114b9e838819c52b10da148eed59baaad647' } as const
const RIYADH_BBOX: [number, number, number, number] = [24.4, 46.4, 25.1, 47.0]

export interface SaudiMotSource {
  refAadt: ReadonlyMap<string, number>
  sourceRows: number
  stations: number
  unavailableTrafficRows: number
}

export interface SaudiRoadSource extends SaudiMotSource {
  riyadh: ReturnType<typeof buildRoadLineVertexGrid>
  atlas: ReturnType<typeof buildRoadLineVertexGrid>
  sourceLines: number
  invalidGeometrySkipped: number
}

export function parseSaudiMotSource(csv: string): SaudiMotSource {
  const rows = parse(csv, { columns: true, skip_empty_lines: true, bom: true, relax_column_count: false }) as Record<string, unknown>[]
  const aggregates = new Map<string, { sum: number; count: number }>()
  let stations = 0, unavailableTrafficRows = 0
  for (const row of rows) {
    const roadRef = String(row['Road No.'] ?? '').trim()
    const latitude = Number(row.Latitude), longitude = Number(row.Longitude)
    const aadt = Number(String(row['24 Hour Total'] ?? '').replace(/[,\"]/g, ''))
    if (!roadRef || !Number.isFinite(latitude) || !Number.isFinite(longitude) ||
        !Number.isFinite(aadt) || aadt <= 0) { unavailableTrafficRows++; continue }
    const aggregate = aggregates.get(roadRef) ?? { sum: 0, count: 0 }
    aggregate.sum += aadt; aggregate.count++
    aggregates.set(roadRef, aggregate); stations++
  }
  if (rows.length === 0 || aggregates.size === 0) throw new Error('Saudi MoT source has no usable traffic counts')
  return { refAadt: new Map([...aggregates].map(([ref, value]) => [ref, Math.round(value.sum / value.count)])),
    sourceRows: rows.length, stations, unavailableTrafficRows }
}

export function loadSaudiRoadSource(options: RoadLoaderArguments): SaudiRoadSource {
  const mot = parseSaudiMotSource(readPinnedRoadSource(options, MOT_FILE.relativePath, MOT_FILE.sha256).toString('utf8'))
  const riyadh = loadPinnedRoadLines(options, [RIYADH_FILE])
  const atlas = loadPinnedRoadLines(options, [ATLAS_FILE])
  return { ...mot, riyadh: buildRoadLineVertexGrid(riyadh.lines), atlas: buildRoadLineVertexGrid(atlas.lines),
    sourceRows: mot.sourceRows + riyadh.sourceRows + atlas.sourceRows,
    sourceLines: riyadh.lines.length + atlas.lines.length,
    invalidGeometrySkipped: riyadh.invalidGeometrySkipped + atlas.invalidGeometrySkipped }
}

const text = (line: PinnedRoadLine, key: string): string => String(line.properties[key] ?? '').trim()
function riyadhRank(line: PinnedRoadLine): number {
  const classification = text(line, 'CLASS').toUpperCase()
  return classification === 'A' ? 0 : classification === 'B' ? 1 : classification === 'C' ? 2
    : classification === 'D' ? 3 : 2
}
function atlasRank(line: PinnedRoadLine): number {
  const classification = text(line, 'RTT_DESCRI').toLowerCase()
  return classification.includes('primary') ? 1 : classification.includes('secondary') ? 3 : 4
}
function riyadhAadt(line: PinnedRoadLine): number {
  const classification = text(line, 'CLASS').toUpperCase()
  const rawLanes = Number(line.properties.NO_OF_LANE)
  const lanes = Math.max(1, Number.isFinite(rawLanes) && rawLanes > 0 ? rawLanes : 2)
  if (classification === 'A') return lanes >= 5 ? 50_000 : lanes >= 4 ? 35_000 : 22_000
  if (classification === 'B') return lanes >= 4 ? 18_000 : lanes >= 3 ? 12_000 : 8_000
  if (classification === 'C') return lanes >= 3 ? 6_000 : 3_500
  if (classification === 'D') return lanes >= 2 ? 1_800 : 900
  return 4_000
}
function atlasAadt(line: PinnedRoadLine): number {
  const classification = text(line, 'RTT_DESCRI').toLowerCase()
  return classification.includes('primary') ? 6_000 : classification.includes('secondary') ? 1_500 : 800
}
function compatible(rowRank: number, rank: number): boolean {
  return Math.abs(rowRank - rank) <= ROAD_CLASS_RANK_TOLERANCE
}
function split(aadt: number) {
  return { light: Math.round(aadt * 0.78), medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.11), moto: Math.round(aadt * 0.01) }
}

export function matchSaudiRoad(row: RoadRow, source: SaudiRoadSource) {
  for (const token of String(row.ref ?? '').split(/[;,]/)) {
    const roadRef = token.trim().replace(/^[A-Za-z]+/, '')
    const aadt = source.refAadt.get(roadRef)
    if (aadt !== undefined) return { kind: 'mot' as const, ...split(aadt) }
  }
  const rank = osmRoadClassRank(row.roadClass)
  if (inBbox(row.midLat, row.midLon, RIYADH_BBOX)) {
    const line = nearestRoadLine(row.midLat, row.midLon, source.riyadh, 200,
      candidate => compatible(rank, riyadhRank(candidate)))
    if (line) return { kind: 'riyadh' as const, ...split(riyadhAadt(line)) }
  }
  const line = nearestRoadLine(row.midLat, row.midLon, source.atlas, 250,
    candidate => compatible(rank, atlasRank(candidate)))
  return line ? { kind: 'atlas' as const, ...split(atlasAadt(line)) } : null
}

export const SAUDI_ROAD_BBOX: [number, number, number, number] = [16.0, 34.5, 32.5, 51.0]
export const SAUDI_ROAD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])
