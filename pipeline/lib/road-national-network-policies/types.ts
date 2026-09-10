/** Contract for national road-network sources whose traffic values are classification proxies. */

import type { PreparedBbox } from '../prepared-grid.js'
import type { PinnedRoadLine, PinnedRoadLineFile } from '../pinned-road-lines.js'
import type { PolicyRoadTraffic } from '../road-national-policies/types.js'
import type { RoadRow } from '../roads-arrow.js'

export interface NationalRoadLinePolicy {
  country: string
  bbox: PreparedBbox
  coverage: ReadonlySet<number>
  files: readonly PinnedRoadLineFile[]
  radiusMetres: number
  acceptLine: (line: PinnedRoadLine) => boolean
  traffic: (row: RoadRow, line: PinnedRoadLine) => PolicyRoadTraffic
}
