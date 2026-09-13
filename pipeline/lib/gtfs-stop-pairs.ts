/** Parse active GTFS rail trips into station pairs for graph matching. */

import { existsSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { readGtfsPairCache, writeGtfsPairCache } from './gtfs-pair-cache.js'
import { readGtfsStopTimes, readCsvRows } from './gtfs-csv.js'
import { RailPairSearches } from './rail-pair-searches.js'
import {
  RAIL_TYPES, readGtfsTripDepartureMultipliers,
  computeActiveTripFamiliesForFeed, loadStopsWithCoords, resolveStopViaParent,
  type GtfsStop,
} from './gtfs-enrich-core.js'
import type { RailStationPairCount } from './rail-graph.js'
export type { RailStationPairCount }

export interface StopPairFrequenciesOptions {
  /** Bounding box for stops.txt out-of-bounds pruning (`GTFS_BORDER_MARGIN_DEG`
   *  margin), same convention as `loadStopsWithCoords`/
   *  `computeStopFrequenciesForFeed`. Omit to keep every stop. */
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
  /** Distinct trips actually found in stop_times.txt (can be < activeTripCount for a
   *  malformed feed with trips that have no stop pattern). */
  tripsWithStopTimes: number
  stopTimesLines: number
  /** Consecutive-pair occurrences emitted across all trips, before identical-search summing. */
  pairEventsBeforeDedup: number
  /** Distinct directed coordinate/shape searches, with counts summed across identical inputs. */
  pairsAfterDedup: number
  droppedUnresolvedStops: number
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
    droppedUnresolvedStops: 0,
    collapsedAdjacentDuplicates: 0,
    frequenciesExpanded: false,
    tripsWithShape: 0,
    fromCache: false,
    ...overrides,
  }
}

/** The GTFS inputs whose content this parser's result depends on. A change in
 *  ANY of them (a `--force-download` unzipping a fresh feed over the old
 *  extractDir is the reference case) must invalidate the pair cache — the
 *  optionsKey alone cannot see it (2026-07-16 review fix, item 2: the cache
 *  used to keep serving the OLD timetable's pairs, target date and
 *  frequencies.txt expansion after a feed refresh, and could even vouch
 *  "non-empty" retract evidence for a broken fresh feed). */
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

  const stops = await loadStopsWithCoords(extractDir, opts.bbox)

  // ── Per trip: sort, resolve+bridge, collapse adjacent dups, emit + accumulate pairs ──
  const searches = new RailPairSearches()
  let droppedUnresolvedStops = 0
  let collapsedAdjacentDuplicates = 0
  let pairEventsBeforeDedup = 0
  let tripsWithShape = 0

  for (const [tripId, rows] of perTripRows) {
    rows.sort((a, b) => a.seq - b.seq) // numeric stop_sequence sort — file order is not trustworthy
    const boost = boostFor(tripId)
    const shapeId = tripShapeId.get(tripId)
    const shape = shapeId ? shapesById.get(shapeId) : undefined
    if (shape) tripsWithShape++

    // Resolve WITH parent-station fallback BEFORE dropping coordless stops.
    const resolved: GtfsStop[] = []
    for (const { stopId } of rows) {
      const { stop } = resolveStopViaParent(stops, stopId)
      if (!stop) { droppedUnresolvedStops++; continue }
      resolved.push(stop)
    }

    // Collapse adjacent duplicates (same resolved station) — dropped/bridged stops
    // above already let pairs span the gap; this handles e.g. two child platforms of
    // one parent visited back-to-back.
    const sequence: GtfsStop[] = []
    for (const stop of resolved) {
      const prev = sequence[sequence.length - 1]
      if (prev && prev.stop_id === stop.stop_id) { collapsedAdjacentDuplicates++; continue }
      sequence.push(stop)
    }

    for (let i = 0; i < sequence.length - 1; i++) {
      const from = sequence[i], to = sequence[i + 1]
      searches.add({ fromLat: from.lat, fromLon: from.lon, toLat: to.lat, toLon: to.lon,
        pax: boost, frt: 0, ...(shape ? { shapePolyline: shape } : {}) })
      pairEventsBeforeDedup++
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
    droppedUnresolvedStops,
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
