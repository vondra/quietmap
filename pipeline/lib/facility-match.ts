/** Registry coordinates classify their smallest containing footprint; mutual-best site deduplication. */

import { DataType, Table, type Vector } from 'apache-arrow'
import { gridToLonLat } from './prepared-grid.js'
import { SOURCE_ID_GLOBAL_GEM_COALMINE } from './source-ids.generated.js'
import { buildOneHundredthDegreePointGrid, flatDist, M_PER_DEG_LAT, M_PER_DEG_LON_EQ, pointGridCandidates, pointInRing, wrapLonDeltaDeg } from './spatial.js'

export interface MatchFacility {
  lat: number
  lon: number
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
  ring: readonly (readonly [number, number])[]
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
  const geometry = table.getChild('geom')
  if (!geometry || !DataType.isBinary(geometry.type)) throw new Error('industrial Arrow requires binary geom')
  const area = table.getChild('area_m2')
  if (!area || !DataType.isFloat(area.type)) throw new Error('industrial Arrow requires floating area_m2')
  return Array.from({ length: table.numRows }, (_, row) => {
    const areaM2 = (area.get(row) as number | null) ?? 0
    if (!Number.isFinite(areaM2) || areaM2 < 0) throw new Error(`invalid industrial area at row ${row}`)
    const center = gridToLonLat(gx.get(row) as number, gy.get(row) as number)
    const bytes = geometry.get(row) as Uint8Array | null
    const ring: Array<[number, number]> = []
    if (bytes !== null) {
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
      if (bytes.byteLength < 4 || bytes.byteLength !== 4 + view.getUint32(0, true) * 8) {
        throw new Error(`invalid industrial geometry at row ${row}`)
      }
      for (let offset = 4; offset < bytes.byteLength; offset += 8) {
        const { lon, lat } = gridToLonLat(view.getInt32(offset, true), view.getInt32(offset + 4, true))
        ring.push([center.lon + longitudeDelta(lon, center.lon), lat])
      }
    }
    return { ...center, areaM2, ring,
      subtype: subtype.get(row) as number, sourceType: sourceType.get(row) as number }
  })
}

// OSM subtype codes are owned by osm-extract/spill.rs; compatible NACE divisions
// retain the dev1 quiet-address and monolithic-heavy-site classification gates.
const HEAVY_SUBTYPE_NACE: Record<number, readonly number[]> = {
  3: [5, 7, 8], 4: [19, 20], 5: [23], 6: [24],
}
const SOURCE_TYPE_NACE: Record<number, readonly number[]> = { 4: [37] }

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
// Equal levels retain observation order. Update together with the engine arms.
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

const longitudeDelta = (a: number, b: number) => wrapLonDeltaDeg(a - b)

/** Every containing footprint vertex lies within this conservative metric box. */
export function lookupRadiusM(polygon: Pick<MatchPolygon, 'lat' | 'lon' | 'ring'>): number {
  return polygon.ring.reduce((radius, [lon, lat]) => Math.max(radius,
    Math.hypot((lat - polygon.lat) * M_PER_DEG_LAT, longitudeDelta(lon, polygon.lon) * M_PER_DEG_LON_EQ)), 0)
}

export function containsFacility(facility: MatchFacility, polygon: MatchPolygon): boolean {
  return pointInRing(polygon.lon + longitudeDelta(facility.lon, polygon.lon), facility.lat, polygon.ring)
}

/** The owning footprint keeps a dedicated or incompatible mapped identity; the point never falls through to an enclosing zone. */
export function footprintAcceptsRegistryClass(
  facility: Pick<MatchFacility, 'nace4'>, polygon: Pick<MatchPolygon, 'sourceType' | 'subtype'>,
): boolean {
  const activity = SOURCE_TYPE_NACE[polygon.sourceType ?? 0]
  return isGenericIndustrialSource(polygon.sourceType) && !quietGateBlocks(polygon.subtype, facility.nace4)
    && (activity === undefined || activity.includes(Math.floor(facility.nace4 / 100)))
}

/** Source authority selects the registry; its loudest activity represents the footprint. */
export function contestBeats(
  a: { rank: number; year: number; id: number; nace4?: number },
  b: { rank: number; year: number; id: number; nace4?: number },
): boolean {
  // Commodity-specific GEM coal evidence is more informative than a broad
  // E-PRTR mining activity for the same footprint.
  const coal = (f: typeof a) => f.id === SOURCE_ID_GLOBAL_GEM_COALMINE && Math.floor((f.nace4 ?? 0) / 100) === 5
  const broadMining = (f: typeof a) => f.id !== SOURCE_ID_GLOBAL_GEM_COALMINE &&
    [7, 8].includes(Math.floor((f.nace4 ?? -100) / 100))
  if (coal(a) && broadMining(b)) return true
  if (coal(b) && broadMining(a)) return false
  if (a.rank !== b.rank) return a.rank > b.rank
  if (a.year !== b.year) return a.year > b.year
  if (a.id !== b.id) return a.id > b.id
  return a.nace4 !== undefined && b.nace4 !== undefined && naceBaseLw(a.nace4) > naceBaseLw(b.nace4)
}

/** Nested footprints resolve to the tenant; equal-area outlines preserve dedicated identity. */
export function containingPolygonBeats(facility: MatchFacility, candidate: MatchPolygon, previous: MatchPolygon): boolean {
  if (candidate.areaM2 !== previous.areaM2) return candidate.areaM2 < previous.areaM2
  const generic = isGenericIndustrialSource(candidate.sourceType)
  if (generic !== isGenericIndustrialSource(previous.sourceType)) return !generic
  return flatDist(facility.lat, facility.lon, candidate.lat, candidate.lon)
    < flatDist(facility.lat, facility.lon, previous.lat, previous.lon)
}

export interface OverlapWinner extends Pick<MatchPolygon, 'lat' | 'lon' | 'areaM2' | 'ring'> {
  key: string
  rank: number
  year: number
  id: number
  nace4?: number
}

// Similar whole-site outlines must contain each other's centres. Nearby disjoint
// footprints cannot suppress one another, however large their equivalent circles.
export const OVERLAP_MIN_AREA_M2 = 100_000
export const OVERLAP_AREA_RATIO_MAX = 2.5

export function overlapsSameSite(a: OverlapWinner, b: OverlapWinner): boolean {
  const minimumArea = Math.min(a.areaM2, b.areaM2)
  return minimumArea >= OVERLAP_MIN_AREA_M2 &&
    Math.max(a.areaM2, b.areaM2) / minimumArea <= OVERLAP_AREA_RATIO_MAX &&
    pointInRing(a.lon + longitudeDelta(b.lon, a.lon), b.lat, a.ring) &&
    pointInRing(b.lon + longitudeDelta(a.lon, b.lon), a.lat, b.ring)
}

export function overlapPairs(winners: OverlapWinner[]): Array<[string, string]> {
  const grid = buildOneHundredthDegreePointGrid(winners.map((winner, index) =>
    ({ latitude: winner.lat, longitude: winner.lon, index })))
  const bestPartner = new Int32Array(winners.length).fill(-1)
  for (const [i, winner] of winners.entries()) {
    if (winner.areaM2 < OVERLAP_MIN_AREA_M2) continue
    let nearest = Infinity
    // A partner's centre must lie in this footprint.
    const reach = lookupRadiusM(winner)
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
