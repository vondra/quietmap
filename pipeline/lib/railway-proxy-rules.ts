/** Shared model for operator-anchored railway proxies that have no open timetable feed. */

import { inBbox, pointToPolylineDist } from './spatial.js'
import type { PreparedBbox } from './prepared-grid.js'
import type { RailwayRow } from './railways-arrow.js'

export interface ProxyTrainCounts {
  passenger: number
  freight: number
}

export interface RailwayProxySpec {
  iso2: string
  bbox: PreparedBbox
  sourceId: number
  /** Null means dev1 proved that no source exists and this country must not stamp. */
  classify: ((row: RailwayRow) => ProxyTrainCounts | null) | null
}

export const trains = (passenger: number, freight: number): ProxyTrainCounts =>
  ({ passenger, freight })

export const nearPolyline = (
  row: RailwayRow,
  line: ReadonlyArray<readonly [number, number]>,
  thresholdMetres: number,
): boolean => pointToPolylineDist(row.midLat, row.midLon, line) <= thresholdMetres

export const nearAnyPolyline = (
  row: RailwayRow,
  lines: ReadonlyArray<ReadonlyArray<readonly [number, number]>>,
  thresholdMetres: number,
): boolean => lines.some(line => nearPolyline(row, line, thresholdMetres))

export const inCoordinateBox = (
  row: RailwayRow,
  bbox: PreparedBbox,
): boolean => inBbox(row.midLat, row.midLon, bbox)

export const inCentreBox = (
  row: RailwayRow,
  latitude: number,
  longitude: number,
  halfDegrees: number,
): boolean => inCoordinateBox(row, [
  latitude - halfDegrees,
  longitude - halfDegrees,
  latitude + halfDegrees,
  longitude + halfDegrees,
])
