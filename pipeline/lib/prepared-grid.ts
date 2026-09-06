/** Strict z9 prepared-data paths and z30 geometry decoding for enrichers. */

import { existsSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { DataType, Table, type Vector } from 'apache-arrow'

// Faithful JS mirror of engine/grid, which owns these constants and formulas.
const WEB_MERCATOR_RADIUS_M = 6_378_137
const EARTH_CIRCUMFERENCE_M = 40_075_016.685_578_49
const GRID_QUANTUM_M = 0.037_322_767_717_044_72
const GRID_ORIGIN = 2 ** 29
const Z9_AXIS = 512
const Z9_SPAN_DEG = 360 / Z9_AXIS

export const GRID_CONTRACT = 'z30'
export const COUNTRY_BAKED_CONTRACT = 'country_baked_v1'

export interface SegmentGeometry {
  startLat: number
  startLon: number
  endLat: number
  endLon: number
  midLat: number
  midLon: number
}

export interface SegmentEndpointKeys {
  startKey: string
  endKey: string
}

export interface SegmentGeometryReader {
  row(index: number): SegmentGeometry
  endpointKeys(index: number): SegmentEndpointKeys
}

function requiredVector(table: Table, name: string): Vector {
  const vector = table.getChild(name)
  if (!vector) throw new Error(`prepared Arrow missing '${name}'`)
  return vector
}

function requiredInt32(table: Table, name: string): Vector {
  const vector = requiredVector(table, name)
  if (!DataType.isInt(vector.type) || !vector.type.isSigned || vector.type.bitWidth !== 32 || vector.nullCount !== 0) {
    throw new Error(`prepared Arrow '${name}' must be non-null Int32`)
  }
  return vector
}

function assertRow(index: number, rows: number): void {
  if (!Number.isInteger(index) || index < 0 || index >= rows) {
    throw new RangeError(`prepared Arrow row ${index} outside 0..${rows - 1}`)
  }
}

export function normalizeLongitude(longitude: number): number {
  return ((longitude + 180) % 360 + 360) % 360 - 180
}

export function wrappedLongitudeMidpoint(start: number, end: number): number {
  const delta = normalizeLongitude(end - start)
  return normalizeLongitude(start + delta / 2)
}

/** Decode one global z30 cell corner to latitude/longitude. */
export function gridToLonLat(gx: number, gy: number): { lat: number; lon: number } {
  if (!Number.isInteger(gx) || !Number.isInteger(gy)) throw new TypeError('grid coordinates must be integers')
  const x = (gx - GRID_ORIGIN) * GRID_QUANTUM_M
  const y = (gy - GRID_ORIGIN) * GRID_QUANTUM_M
  return {
    lon: (x / WEB_MERCATOR_RADIUS_M) * 180 / Math.PI,
    lat: (2 * Math.atan(Math.exp(y / WEB_MERCATOR_RADIUS_M)) - Math.PI / 2) * 180 / Math.PI,
  }
}

/** Validate the on-disk geometry contract once, then decode rows cheaply. */
export function segmentGeometryReader(table: Table): SegmentGeometryReader {
  if (table.schema.metadata.get('grid') !== GRID_CONTRACT) {
    throw new Error(`prepared Arrow grid contract must be '${GRID_CONTRACT}'`)
  }
  const startGx = requiredInt32(table, 'start_gx')
  const startGy = requiredInt32(table, 'start_gy')
  const endGx = requiredInt32(table, 'end_gx')
  const endGy = requiredInt32(table, 'end_gy')
  return {
    endpointKeys(index): SegmentEndpointKeys {
      assertRow(index, table.numRows)
      return { startKey: `${startGx.get(index)}_${startGy.get(index)}`,
        endKey: `${endGx.get(index)}_${endGy.get(index)}` }
    },
    row(index): SegmentGeometry {
      assertRow(index, table.numRows)
      const start = gridToLonLat(startGx.get(index) as number, startGy.get(index) as number)
      const end = gridToLonLat(endGx.get(index) as number, endGy.get(index) as number)
      return {
        startLat: start.lat,
        startLon: start.lon,
        endLat: end.lat,
        endLon: end.lon,
        midLat: (start.lat + end.lat) / 2,
        midLon: wrappedLongitudeMidpoint(start.lon, end.lon),
      }
    },
  }
}

/** Numeric little-endian ISO2 form used by the committed admin bake. */
export function iso2Code(iso: string): number {
  if (!/^[A-Z]{2}$/.test(iso)) throw new Error(`invalid ISO2 '${iso}'`)
  return iso.charCodeAt(0) | (iso.charCodeAt(1) << 8)
}

function bakedCountryReader(
  table: Table,
  layer: 'roads' | 'railways' | 'industrial',
): { codeAt(index: number): number } {
  const contract = layer === 'industrial' ? 'country_land_baked_v1' : COUNTRY_BAKED_CONTRACT
  if (table.schema.metadata.get(`${layer}_contract`) !== contract) {
    throw new Error(`${layer} Arrow contract must be '${contract}' before national enrichment`)
  }
  const vector = requiredVector(table, 'country_iso')
  if (!DataType.isInt(vector.type) || vector.type.isSigned || vector.type.bitWidth !== 16 || vector.nullCount !== 0) {
    throw new Error(`${layer} Arrow 'country_iso' must be non-null Uint16`)
  }
  return {
    codeAt(index: number): number {
      assertRow(index, table.numRows)
      return vector.get(index) as number
    },
  }
}

/** Fail-closed access to road ownership baked by scripts/admin/build_admin.py. */
export function bakedRoadCountryReader(table: Table): { codeAt(index: number): number } {
  return bakedCountryReader(table, 'roads')
}

/** Fail-closed access to railway ownership baked by scripts/admin/build_admin.py. */
export function bakedRailwayCountryReader(table: Table): { codeAt(index: number): number } {
  return bakedCountryReader(table, 'railways')
}

/** Strict land ownership for industrial centroids; coastal road attribution is not admissible. */
export function bakedIndustrialCountryReader(table: Table): { codeAt(index: number): number } {
  return bakedCountryReader(table, 'industrial')
}

export type PreparedBbox = readonly [south: number, west: number, north: number, east: number]

function canonicalAxisDirectory(name: string): number | null {
  if (!/^(0|[1-9]\d*)$/.test(name)) return null
  const value = Number(name)
  return value < Z9_AXIS ? value : null
}

function tileLatitudeSpan(y: number): [south: number, north: number] {
  const half = EARTH_CIRCUMFERENCE_M / 2
  const northM = half - y * EARTH_CIRCUMFERENCE_M / Z9_AXIS
  const southM = half - (y + 1) * EARTH_CIRCUMFERENCE_M / Z9_AXIS
  const latitude = (meters: number) =>
    (2 * Math.atan(Math.exp(meters / WEB_MERCATOR_RADIUS_M)) - Math.PI / 2) * 180 / Math.PI
  return [latitude(southM), latitude(northM)]
}

function intersectsBbox(x: number, y: number, bbox: PreparedBbox): boolean {
  const [south, west, north, east] = bbox
  if (![south, west, north, east].every(Number.isFinite) || south > north ||
      south < -90 || north > 90 || west < -180 || west > 180 || east < -180 || east > 180) {
    throw new Error(`invalid prepared bbox '${bbox.join(',')}'`)
  }
  const [tileSouth, tileNorth] = tileLatitudeSpan(y)
  if (tileNorth < south || tileSouth > north) return false
  const tileWest = x * Z9_SPAN_DEG - 180
  const tileEast = tileWest + Z9_SPAN_DEG
  const overlaps = (rangeWest: number, rangeEast: number) => tileEast >= rangeWest && tileWest <= rangeEast
  return west <= east ? overlaps(west, east) : overlaps(west, 180) || overlaps(-180, east)
}

/** Canonical `z9/x/y` units intersecting a mandatory bounded scope, numerically sorted. */
export function listPreparedSquares(
  preparedYearDirectory: string,
  bbox: PreparedBbox,
  layerFile = 'roads.arrow',
): string[] {
  const z9 = resolve(preparedYearDirectory, 'z9')
  if (!existsSync(z9)) return []
  const found: Array<{ x: number; y: number; name: string }> = []
  for (const xEntry of readdirSync(z9, { withFileTypes: true })) {
    const x = xEntry.isDirectory() ? canonicalAxisDirectory(xEntry.name) : null
    if (x === null) continue
    for (const yEntry of readdirSync(resolve(z9, xEntry.name), { withFileTypes: true })) {
      const y = yEntry.isDirectory() ? canonicalAxisDirectory(yEntry.name) : null
      if (y === null || !intersectsBbox(x, y, bbox)) continue
      const name = `z9/${x}/${y}`
      if (existsSync(resolve(preparedYearDirectory, name, layerFile))) found.push({ x, y, name })
    }
  }
  found.sort((a, b) => a.x - b.x || a.y - b.y)
  return found.map(({ name }) => name)
}
