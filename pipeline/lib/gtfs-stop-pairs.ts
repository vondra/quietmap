/** Parse active GTFS rail trips into station pairs for graph matching. */

import { existsSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { readGtfsPairCache, writeGtfsPairCache } from './gtfs-pair-cache.js'
import { readGtfsStopTimes, readCsvRows } from './gtfs-csv.js'
import { RailPairSearches } from './rail-pair-searches.js'
import {
  RAIL_TYPES, readGtfsTripDepartureMultipliers,
  computeActiveTripFamiliesForFeed, gtfsStopWithinBounds, loadStopsWithCoords, resolveStopViaParent,
  type GtfsStop,
} from './gtfs-enrich-core.js'
import type { RailStationPairCount } from './rail-graph.js'
export type { RailStationPairCount }

export interface StopPairFrequenciesOptions {
  /** Geographic extent for resolved observations, padded by GTFS_BORDER_MARGIN_DEG.
   *  Clipping breaks adjacency; unresolved source stops fail independently of this extent. */
  bbox?: readonly [number, number, number, number]
  /** Route-type -> family classifier. Defaults to RAIL_TYPES-only (tram/metro excluded).
   *  Pass a metroAsRail-style override (e.g. europe's `railFamilyFor` narrowed to 'rail')
   *  to widen it per feed — the hook IS the override mechanism, no built-in flag needed. */
  familyOf?: (routeType: number) => 'rail' | null
  /** Calendar service-day picker, shared with `computeActiveTripFamiliesForFeed`.
   *  Defaults to the midpoint-Wednesday policy; pass europe's busiest-Wednesday sampler
   *  to match its per-stop counter's target date. */
  dateSelection?: (calendarRows: Record<string, string>[]) => string
  /** Expand frequencies.txt headway blocks into a per-trip repeat count (TH-multiplier
   *  style, `enrich-railway-th.ts`). Defaults to true iff frequencies.txt exists in
   *  extractDir; pass false to force-ignore it even when present. */
  expandFrequencies?: boolean
  /** Optional derived-cache path. Omit it to keep the source tree immutable. */
  cachePath?: string
  /** Caller identity for family/date callbacks, whose captured values cannot be derived.
   *  Geographic bounds and frequency expansion are included automatically. */
  optionsKey?: string
}

export interface StopPairFrequenciesProvenance {
  /** Resolved service day (YYYYMMDD), '' when unresolved (no rail trips at all). */
  targetDate: string
  calendarPresent: boolean
  /** Active rail trips selected for `targetDate` — before stop_times is even read. */
  activeTripCount: number
  /** Distinct active trips found in stop_times.txt; missing active trips fail the parse. */
  tripsWithStopTimes: number
  stopTimesLines: number
  /** Consecutive-pair occurrences emitted across all trips, before identical-search summing. */
  pairEventsBeforeDedup: number
  /** Distinct directed coordinate/shape searches, with counts summed across identical inputs. */
  pairsAfterDedup: number
  /** Resolved stop-time observations outside the requested geographic extent. */
  clippedStopTimes: number
  collapsedAdjacentDuplicates: number
  frequenciesExpanded: boolean
  tripsWithShape: number
  /** True on a cache hit; all source accounting is retained. */
  fromCache: boolean
}

export interface StopPairFrequenciesResult {
  pairs: RailStationPairCount[]
  provenance: StopPairFrequenciesProvenance
}

function defaultFamilyOf(routeType: number): 'rail' | null {
  return RAIL_TYPES.has(routeType) ? 'rail' : null
}

function emptyProvenance(overrides: Partial<StopPairFrequenciesProvenance> = {}): StopPairFrequenciesProvenance {
  return {
    targetDate: '',
    calendarPresent: false,
    activeTripCount: 0,
    tripsWithStopTimes: 0,
    stopTimesLines: 0,
    pairEventsBeforeDedup: 0,
    pairsAfterDedup: 0,
    clippedStopTimes: 0,
    collapsedAdjacentDuplicates: 0,
    frequenciesExpanded: false,
    tripsWithShape: 0,
    fromCache: false,
    ...overrides,
  }
}

/** Every source file used by this parser participates in cache invalidation. */
const FINGERPRINT_INPUT_FILES = [
  'routes.txt', 'trips.txt', 'stop_times.txt', 'stops.txt', 'shapes.txt',
  'calendar.txt', 'calendar_dates.txt', 'frequencies.txt', 'feed_info.txt',
] as const

/** Source size and modification time, including explicit absent-file identities. */
export function gtfsPairInputsFingerprint(extractDir: string): string {
  return FINGERPRINT_INPUT_FILES.map((f) => {
    const p = resolve(extractDir, f)
    if (!existsSync(p)) return `${f}:absent`
    const st = statSync(p)
    return `${f}:${st.size}:${st.mtimeMs}`
  }).join(';')
}

export async function computeStopPairFrequenciesForFeed(
  extractDir: string,
  opts: StopPairFrequenciesOptions = {},
): Promise<StopPairFrequenciesResult> {
  const identity = {
    options: JSON.stringify([opts.optionsKey ?? 'default', opts.bbox ?? null, opts.expandFrequencies ?? null]),
    inputs: gtfsPairInputsFingerprint(extractDir),
  }
  const cachePath = opts.cachePath
  if (cachePath) {
    const cached = readGtfsPairCache(cachePath, identity)
    if (cached) return cached
  }

  const familyOf = opts.familyOf ?? defaultFamilyOf

  const { tripFam, targetDate, calendarPresent } =
    await computeActiveTripFamiliesForFeed(extractDir, familyOf, opts.dateSelection)

  if (tripFam.size === 0) {
    return { pairs: [], provenance: emptyProvenance({ targetDate, calendarPresent }) }
  }

  // ── trips.txt (2nd pass): shape_id per active trip ──
  const tripShapeId = new Map<string, string>()
  await readCsvRows(resolve(extractDir, 'trips.txt'), r => {
    if (!tripFam.has(r['trip_id'])) return
    const shapeId = (r['shape_id'] || '').trim()
    if (shapeId) tripShapeId.set(r['trip_id'], shapeId)
  })

  // ── shapes.txt (only if any active trip references one) ──
  const shapesById = new Map<string, Array<[number, number]>>()
  const shapesPath = resolve(extractDir, 'shapes.txt')
  if (tripShapeId.size > 0 && existsSync(shapesPath)) {
    const requiredShapeIds = new Set(tripShapeId.values())
    const bySeq = new Map<string, Array<{ seq: number; lat: number; lon: number }>>()
    await readCsvRows(shapesPath, r => {
      const id = r['shape_id']
      if (!requiredShapeIds.has(id)) return
      const lat = parseFloat(r['shape_pt_lat'] || '')
      const lon = parseFloat(r['shape_pt_lon'] || '')
      if (isNaN(lat) || isNaN(lon)) return
      const seq = parseInt(r['shape_pt_sequence'] || '0', 10)
      let arr = bySeq.get(id)
      if (!arr) { arr = []; bySeq.set(id, arr) }
      arr.push({ seq: Number.isFinite(seq) ? seq : arr.length, lat, lon })
    })
    for (const [id, pts] of bySeq) {
      pts.sort((a, b) => a.seq - b.seq)
      shapesById.set(id, pts.map(p => [p.lat, p.lon]))
    }
  }

  // ── frequencies.txt expansion (opt-in by default when the file exists) ──
  const freqPath = resolve(extractDir, 'frequencies.txt')
  const frequenciesExpanded = opts.expandFrequencies ?? existsSync(freqPath)
  const tripBoosts = frequenciesExpanded
    ? await readGtfsTripDepartureMultipliers(extractDir, new Set(tripFam.keys()))
    : new Map<string, number>()
  const boostFor = (tripId: string): number => tripBoosts.get(tripId) ?? 1

  // ── stop_times.txt (streamed), grouped per active trip ──
  const perTripRows = new Map<string, Array<{ seq: number; stopId: string }>>()
  const stopTimesLines = await readGtfsStopTimes(extractDir, tripFam, (headers, tripIdIdx) => {
    const stopIdIdx = headers.indexOf('stop_id')
    const seqIdx = headers.indexOf('stop_sequence')
    if (stopIdIdx < 0 || seqIdx < 0) {
      throw new Error('stop_times.txt missing stop_id/stop_sequence')
    }
    return fields => {
      const tripId = fields[tripIdIdx]
      let rows = perTripRows.get(tripId)
      if (!rows) { rows = []; perTripRows.set(tripId, rows) }
      const seq = parseInt(fields[seqIdx] || '', 10)
      rows.push({ seq: Number.isFinite(seq) ? seq : rows.length, stopId: fields[stopIdIdx] })
    }
  })

  for (const tripId of tripFam.keys()) {
    if (!perTripRows.has(tripId)) {
      throw new Error(`${extractDir}: active rail trip '${tripId}' has no stop_times`)
    }
  }
  const stops = await loadStopsWithCoords(extractDir)

  const searches = new RailPairSearches()
  let clippedStopTimes = 0
  let collapsedAdjacentDuplicates = 0
  let pairEventsBeforeDedup = 0
  let tripsWithShape = 0

  for (const [tripId, rows] of perTripRows) {
    rows.sort((a, b) => a.seq - b.seq) // numeric stop_sequence sort — file order is not trustworthy
    const boost = boostFor(tripId)
    const shapeId = tripShapeId.get(tripId)
    const shape = shapeId ? shapesById.get(shapeId) : undefined
    if (shape) tripsWithShape++

    let previous: GtfsStop | undefined
    for (const { stopId } of rows) {
      const { stop } = resolveStopViaParent(stops, stopId)
      if (!stop) {
        throw new Error(`${extractDir}: active rail trip '${tripId}' has unresolved stop '${stopId}'`)
      }
      if (!gtfsStopWithinBounds(stop, opts.bbox)) {
        clippedStopTimes++
        previous = undefined
        continue
      }
      if (previous?.stop_id === stop.stop_id) { collapsedAdjacentDuplicates++; continue }
      if (previous) {
        searches.add({ fromLat: previous.lat, fromLon: previous.lon, toLat: stop.lat, toLon: stop.lon,
          pax: boost, frt: 0, ...(shape ? { shapePolyline: shape } : {}) })
        pairEventsBeforeDedup++
      }
      previous = stop
    }
  }

  const pairs = [...searches.values()]
  const provenance = emptyProvenance({
    targetDate,
    calendarPresent,
    activeTripCount: tripFam.size,
    tripsWithStopTimes: perTripRows.size,
    stopTimesLines,
    pairEventsBeforeDedup,
    pairsAfterDedup: pairs.length,
    clippedStopTimes,
    collapsedAdjacentDuplicates,
    frequenciesExpanded,
    tripsWithShape,
  })

  // Empty parses must not certify a later source-admission check.
  if (pairs.length > 0 && cachePath) {
    writeGtfsPairCache(cachePath, identity, { pairs, provenance })
  }

  return { pairs, provenance }
}
