/** Shared contract for national OSM-class road policies without segment counts. */

import type { PreparedBbox } from '../prepared-grid.js'
import type { RoadRow } from '../roads-arrow.js'

export interface PolicyRoadTraffic {
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface NationalRoadPolicy {
  country: string
  bbox: PreparedBbox
  coverage: ReadonlySet<number>
  traffic: (row: RoadRow) => PolicyRoadTraffic | null
}
