/** Project complete GTFS services into the current station-pair railway consumer. */

import { openGtfsServices, type GtfsServiceOptions, type GtfsServiceStop } from './gtfs-service-store.js'
import { RailPairSearches } from './rail-pair-searches.js'
import { gtfsStopWithinBounds } from './gtfs-enrich-core.js'
import type { RailStationPairCount } from './rail-graph.js'
export type { RailStationPairCount }

export interface StopPairFrequenciesOptions extends GtfsServiceOptions {
  /** Clipping breaks adjacency; unresolved source stops fail before clipping. */
  bbox?: readonly [number, number, number, number]
}

export interface StopPairFrequenciesProvenance {
  targetDate: string
  calendarPresent: boolean
  activeTripCount: number
  tripsWithStopTimes: number
  stopTimesLines: number
  pairEventsBeforeDedup: number
  pairsAfterDedup: number
  clippedStopTimes: number
  collapsedAdjacentDuplicates: number
  frequenciesExpanded: boolean
  tripsWithShape: number
  fromCache: boolean
}

export interface StopPairFrequenciesResult {
  pairs: RailStationPairCount[]
  provenance: StopPairFrequenciesProvenance
}

export async function computeStopPairFrequenciesForFeed(
  extractDir: string,
  opts: StopPairFrequenciesOptions = {},
): Promise<StopPairFrequenciesResult> {
  using store = await openGtfsServices(extractDir, opts)
  const { frequenciesPresent, ...source } = store.provenance
  const frequenciesExpanded = source.activeTripCount > 0 && frequenciesPresent
  const searches = new RailPairSearches()
  let clippedStopTimes = 0
  let collapsedAdjacentDuplicates = 0
  let pairEventsBeforeDedup = 0
  let previousShape: unknown
  let shapePolyline: Array<[number, number]> | undefined

  for (const service of store.services()) {
    if (service.shape !== previousShape) {
      previousShape = service.shape
      shapePolyline = service.shape.length ? service.shape.map(point => [point.lat, point.lon]) : undefined
    }
    const boost = frequenciesExpanded ? service.departureMultiplier : 1
    let previous: GtfsServiceStop | undefined
    for (const stop of service.stops) {
      if (!gtfsStopWithinBounds(stop, opts.bbox)) {
        clippedStopTimes++
        previous = undefined
        continue
      }
      if (previous?.stopId === stop.stopId) { collapsedAdjacentDuplicates++; continue }
      if (previous) {
        searches.add({ fromLat: previous.lat, fromLon: previous.lon, toLat: stop.lat, toLon: stop.lon,
          pax: boost, frt: 0, ...(shapePolyline ? { shapePolyline } : {}) })
        pairEventsBeforeDedup++
      }
      previous = stop
    }
  }

  const pairs = [...searches.values()]
  return { pairs, provenance: { ...source, pairEventsBeforeDedup, pairsAfterDedup: pairs.length,
    clippedStopTimes, collapsedAdjacentDuplicates, frequenciesExpanded } }
}
