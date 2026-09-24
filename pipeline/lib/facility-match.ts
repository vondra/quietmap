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
  3: [5, 7, 8], 4: [19, 20], 5: [23], 6: [24],
}

// OSM industrial source_type classes the extractor owns (consumer contract v1):
// turbines pipe their registry parameters through the wind enricher, never a
// NACE stamp; a substation polygon IS its class — a nearby power-plant point
// stamping 3511 onto it was the 7,166-row Tata-class bug.
const TURBINE_SOURCE_TYPE = 10
const SUBSTATION_SOURCE_TYPE = 12

// Mirror of the engine's NACE base levels (division → base_lw at 1 ha, from
// `engine/noise-compute/src/emission/industrial.rs::nace_profile`) — ONLY for
// the "loudest contained facility stamps it" rule. The C1 spectral debt is
// ignored (it never flips a >2 dB base gap); ties fall through to edge.
// Update together with the engine arms.
const NACE_DIVISION_BASE_LW: Record<number, number> = {
  1: 70, 2: 70, 3: 70, 5: 99, 6: 92, 7: 99, 8: 99, 10: 90, 11: 90,
  13: 88, 14: 88, 15: 88, 16: 93, 17: 93, 19: 96, 20: 94, 22: 90,
  23: 100, 24: 100, 25: 93, 27: 90, 28: 90, 29: 93, 30: 93,
  35: 97, 37: 89, 38: 95, 46: 84, 47: 84, 52: 86, 62: 60,
}

/** Comparator loudness of a facility NACE: 4-digit exceptions first (hydro
 * 3512 is quieter than the division-35 thermal fallback; synthetic solar 3599
 * compares at its legacy area level), then the division mirror. Unknown → −1,
 * losing to any known profile. */
export function naceBaseLw(nace4: number | undefined): number {
  if (nace4 === undefined) return -1
  if (nace4 === 3599) return 55
  if (nace4 === 3512) return 90
  return NACE_DIVISION_BASE_LW[Math.floor(nace4 / 100)] ?? -1
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

/** Grid lookup horizon for one polygon: the registry search radius, widened
 * to the polygon's own equivalent radius so a facility it CONTAINS is found
 * even past the 2 km proximity horizon (containment bypasses the radius). */
export function lookupRadiusM(polygon: MatchPolygon, radiusM: number): number {
  return Math.max(radiusM, equivalentCircleRadiusM(polygon.areaM2))
}

export function edgeDistM(facility: { lat: number; lon: number }, polygon: MatchPolygon): number {
  return flatDist(facility.lat, facility.lon, polygon.lat, polygon.lon) - equivalentCircleRadiusM(polygon.areaM2)
}

export function contestBeats(
  a: { rank: number; year: number; id: number; edge?: number; contained?: boolean; nace4?: number },
  b: { rank: number; year: number; id: number; edge?: number; contained?: boolean; nace4?: number },
): boolean {
  if (a.rank !== b.rank) return a.rank > b.rank
  if (a.year !== b.year) return a.year > b.year
  if (a.id !== b.id) return a.id > b.id
  // Same registry: a contained point beats a merely near one (it is inside
  // the plant), and among contained points the loudest NACE stamps the site
  // (Tata: steel 2410 over chemicals 2011) — but only when both sides carry a
  // NACE, so legacy duplicate elections without one keep their exact order.
  if ((a.contained ?? false) !== (b.contained ?? false)) return a.contained ?? false
  if (a.nace4 !== undefined && b.nace4 !== undefined) {
    const loud = naceBaseLw(a.nace4) - naceBaseLw(b.nace4)
    if (loud !== 0) return loud > 0
  }
  return a.edge !== undefined && b.edge !== undefined && a.edge < b.edge
}

export interface MatchCandidate {
  edge: number
  contained: boolean
}

export function candidateEdgeM(facility: MatchFacility, polygon: MatchPolygon, radiusM: number): MatchCandidate | null {
  // Turbines are native point sources and substations carry their own class;
  // a nearby registry point cannot claim either identity.
  if (polygon.sourceType === TURBINE_SOURCE_TYPE || polygon.sourceType === SUBSTATION_SOURCE_TYPE ||
      quietGateBlocks(polygon.subtype, facility.nace4)) return null
  const edge = edgeDistM(facility, polygon)
  // The 2 km radius gates proximity only: a contained facility stamps from
  // inside no matter how far the centroid sits.
  if (edge >= 0 && flatDist(facility.lat, facility.lon, polygon.lat, polygon.lon) >= radiusM) return null
  return { edge, contained: edge < 0 }
}

export interface CandidatePick { row: number; edge: number; contained: boolean; areaM2: number }

/** Facility → polygon preference, shared by `bestCandidate` and the global
 * sweep: the smallest CONTAINING polygon wins; among uncontained polygons
 * the nearest edge wins (the original rule). */
export function candidateBeats(a: CandidatePick, b: CandidatePick): boolean {
  if (a.contained !== b.contained) return a.contained
  if (a.contained && a.areaM2 !== b.areaM2) return a.areaM2 < b.areaM2
  return a.edge < b.edge
}

export function bestCandidate(facility: MatchFacility, polygons: MatchPolygon[], radiusM: number) {
  let best: (CandidatePick) | null = null
  for (const [row, polygon] of polygons.entries()) {
    const candidate = candidateEdgeM(facility, polygon, radiusM)
    if (candidate !== null) {
      const pick = { row, areaM2: polygon.areaM2, ...candidate }
      if (!best || candidateBeats(pick, best)) best = pick
    }
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
  contained?: boolean
  nace4?: number
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
