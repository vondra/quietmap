/** One facility, one industrial polygon; dev1 edge scoring and mutual-best site deduplication. */

import { DataType, Table, type Vector } from 'apache-arrow'
import { gridToLonLat } from './prepared-grid.js'
import { buildOneHundredthDegreePointGrid, flatDist, pointGridCandidates } from './spatial.js'

export interface MatchFacility {
  lat: number
  lon: number
  searchRadiusM?: number
  nace4: number
  id: number
  rank: number
  year: number
}

export interface MatchPolygon {
  lat: number
  lon: number
  areaM2: number
  subtype: number
  sourceType?: number
}

function integerColumn(table: Table, name: string, bits: number, signed = false): Vector {
  const column = table.getChild(name)
  if (!column || !DataType.isInt(column.type) || column.type.bitWidth !== bits ||
      column.type.isSigned !== signed || column.nullCount !== 0) {
    throw new Error(`industrial Arrow '${name}' must be non-null ${signed ? 'Int' : 'Uint'}${bits}`)
  }
  return column
}

export function readPolygons(table: Table): MatchPolygon[] {
  if (table.schema.metadata.get('grid') !== 'z30') throw new Error('industrial Arrow requires grid=z30')
  const gx = integerColumn(table, 'centroid_gx', 32, true)
  const gy = integerColumn(table, 'centroid_gy', 32, true)
  const subtype = integerColumn(table, 'site_subtype', 8)
  const sourceType = integerColumn(table, 'source_type', 8)
  const area = table.getChild('area_m2')
  if (!area || !DataType.isFloat(area.type)) throw new Error('industrial Arrow requires floating area_m2')
  return Array.from({ length: table.numRows }, (_, row) => {
    const areaM2 = (area.get(row) as number | null) ?? 0
    if (!Number.isFinite(areaM2) || areaM2 < 0) throw new Error(`invalid industrial area at row ${row}`)
    return { ...gridToLonLat(gx.get(row) as number, gy.get(row) as number), areaM2,
      subtype: subtype.get(row) as number, sourceType: sourceType.get(row) as number }
  })
}

// OSM subtype codes are owned by osm-extract/spill.rs; compatible NACE divisions
// retain the dev1 quiet-address and monolithic-heavy-site classification gates.
const HEAVY_SUBTYPE_NACE: Record<number, readonly number[]> = {
  3: [5, 8], 4: [19, 20], 5: [23], 6: [24],
}

export function quietGateBlocks(subtype: number, nace4: number): boolean {
  const division = Math.floor(nace4 / 100)
  if (subtype === 10) return !(division >= 1 && division <= 3)
  if (subtype === 1) return ![46, 47, 52].includes(division)
  if (subtype === 11) return true
  const heavy = HEAVY_SUBTYPE_NACE[subtype]
  return heavy !== undefined && !heavy.includes(division)
}

const equivalentCircleRadiusM = (areaM2: number) => Math.sqrt(Math.max(areaM2, 0) / Math.PI)

export function edgeDistM(facility: { lat: number; lon: number }, polygon: MatchPolygon): number {
  return flatDist(facility.lat, facility.lon, polygon.lat, polygon.lon) - equivalentCircleRadiusM(polygon.areaM2)
}

export function contestBeats(
  a: { rank: number; year: number; id: number; edge?: number },
  b: { rank: number; year: number; id: number; edge?: number },
): boolean {
  if (a.rank !== b.rank) return a.rank > b.rank
  if (a.year !== b.year) return a.year > b.year
  if (a.id !== b.id) return a.id > b.id
  return a.edge !== undefined && b.edge !== undefined && a.edge < b.edge
}

export function candidateEdgeM(facility: MatchFacility, polygon: MatchPolygon, radiusM: number): number | null {
  // Turbines are native point sources; a nearby registry cannot claim their identity.
  if (polygon.sourceType === 10 || quietGateBlocks(polygon.subtype, facility.nace4) ||
      flatDist(facility.lat, facility.lon, polygon.lat, polygon.lon) >= radiusM) return null
  return edgeDistM(facility, polygon)
}

export function bestCandidate(facility: MatchFacility, polygons: MatchPolygon[], radiusM: number) {
  let best: { row: number; edge: number } | null = null
  for (const [row, polygon] of polygons.entries()) {
    const edge = candidateEdgeM(facility, polygon, radiusM)
    if (edge !== null && (!best || edge < best.edge)) best = { row, edge }
  }
  return best
}

export interface OverlapWinner {
  key: string
  lat: number
  lon: number
  areaM2: number
  rank: number
  year: number
  id: number
  edge?: number
}

// Accepted dev1 I-07 whole-site duplicate rule: >=10 ha, similar areas, coincident
// centroids. Equivalent circles deliberately do not claim exact polygon intersection.
export const OVERLAP_MIN_AREA_M2 = 100_000
export const OVERLAP_AREA_RATIO_MAX = 2.5
export const OVERLAP_CENTROID_RADIUS_FACTOR = 0.5

export function overlapsSameSite(a: OverlapWinner, b: OverlapWinner): boolean {
  const minimumArea = Math.min(a.areaM2, b.areaM2)
  return minimumArea >= OVERLAP_MIN_AREA_M2 &&
    Math.max(a.areaM2, b.areaM2) / minimumArea <= OVERLAP_AREA_RATIO_MAX &&
    flatDist(a.lat, a.lon, b.lat, b.lon) <= OVERLAP_CENTROID_RADIUS_FACTOR * equivalentCircleRadiusM(minimumArea)
}

export function overlapPairs(winners: OverlapWinner[]): Array<[string, string]> {
  const grid = buildOneHundredthDegreePointGrid(winners.map((winner, index) =>
    ({ latitude: winner.lat, longitude: winner.lon, index })))
  const bestPartner = new Int32Array(winners.length).fill(-1)
  for (const [i, winner] of winners.entries()) {
    if (winner.areaM2 < OVERLAP_MIN_AREA_M2) continue
    let nearest = Infinity
    // The smaller radius controls each pair, so this winner's radius is a safe
    // bound. Metric lookup also covers high latitudes, seams and very large sites.
    const reach = OVERLAP_CENTROID_RADIUS_FACTOR * equivalentCircleRadiusM(winner.areaM2)
    for (const { index: j } of pointGridCandidates(winner.lat, winner.lon, reach, grid)) {
      if (i === j || !overlapsSameSite(winner, winners[j])) continue
      const distance = flatDist(winner.lat, winner.lon, winners[j].lat, winners[j].lon)
      if (distance < nearest || (distance === nearest && j < bestPartner[i])) {
        nearest = distance
        bestPartner[i] = j
      }
    }
  }
  const pairs: Array<[string, string]> = []
  for (let i = 0; i < winners.length; i++) {
    const j = bestPartner[i]
    if (j < 0 || bestPartner[j] !== i || i > j) continue
    pairs.push(contestBeats(winners[i], winners[j])
      ? [winners[i].key, winners[j].key] : [winners[j].key, winners[i].key])
  }
  return pairs
}
