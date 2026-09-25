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

// Dedicated power, turbine and inactive classes never receive generic industry
// priors: turbines pipe their registry parameters through the wind enricher,
// never a NACE stamp; a substation polygon IS its class — a nearby power-plant
// point stamping 3511 onto it was the 7,166-row Tata-class bug.
export const isGenericIndustrialSource = (sourceType: number | undefined): boolean => (sourceType ?? 0) < 10

// Mirror of the engine's NACE EMITTED levels (division → LwA at 1 ha =
// base_lw + the spectrum's C1 debt, from
// `engine/noise-compute/src/emission/industrial.rs::nace_profile` plus
// `industrial_lw`) — ONLY for the "loudest contained facility stamps it"
// rule. Post-norm, so close pairs order exactly as the engine emits them
// (metallurgy 106.4 over cement 104.9 — the pre-norm bases tie at 100).
// Ties fall through to edge. Update together with the engine arms.
const NACE_DIVISION_BASE_LW: Record<number, number> = {
  1: 75.6, 2: 75.6, 3: 75.6, 5: 103.9, 6: 97.6, 7: 103.9, 8: 103.9,
  10: 95.6, 11: 95.6, 13: 93.6, 14: 93.6, 15: 93.6, 16: 99.4, 17: 99.4,
  19: 101.7, 20: 99.6, 22: 95.6, 23: 104.9, 24: 106.4, 25: 99.4,
  27: 95.6, 28: 95.6, 29: 98.6, 30: 98.6, 35: 102.7, 37: 94.2, 38: 100.6,
  46: 89.4, 47: 89.4, 52: 91.4, 62: 65.4,
}

/** Comparator loudness of a facility NACE: 4-digit exceptions first (hydro
 * 3512 is quieter than the division-35 thermal fallback; synthetic solar 3599
 * compares at its 1 ha per-MW value 80.4), then the division mirror.
 * Unknown → −1, losing to any known profile. */
export function naceBaseLw(nace4: number | undefined): number {
  if (nace4 === undefined) return -1
  if (nace4 === 3599) return 80.4
  if (nace4 === 3512) return 95.6
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
  // Dedicated power, turbine and inactive classes carry their own identity;
  // a nearby registry point cannot claim it.
  if (!isGenericIndustrialSource(polygon.sourceType) ||
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
