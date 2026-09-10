/** Shared GTFS timetable selection, parsing and stop-to-track matching. */

import { createReadStream, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { pointToSegmentDist } from './spatial.js'
import type { RailwayRow, RailwayTraffic } from './railways-arrow.js'

// ── GTFS route_type families ──

// GTFS route_type: 2=Rail, 100-109=Railway subtypes, 0=Tram, 900-906=Tram subtypes,
// 1=Subway/Metro, 400-405=Urban Railway/Monorail subtypes
export const RAIL_TYPES = new Set([2, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109])
export const TRAM_TYPES = new Set([0, 900, 901, 902, 903, 904, 905, 906])
export const METRO_TYPES = new Set([1, 400, 401, 402, 403, 404, 405])

// GTFS route family → OSM rail_type family (rail_type 0=rail, 1=tram, 2=light_rail).
// Metro/light-metro is grouped with tram: OSM tags light-metro as light_rail (rail_type 2)
// while GTFS tags it route_type 1 (Porto etc.), and true underground subways have no OSM
// segment so never match. Conscious Occam trade-off — a subway STOP within 500 m of a
// surface tram/light_rail segment can match it; accepted over a 3-family scheme that would
// miss GTFS-tram-tagged light rails. Bus/ferry/etc. → null (skipped).
export function routeFamily(routeType: number): 'rail' | 'tram' | null {
  if (RAIL_TYPES.has(routeType)) return 'rail'
  if (TRAM_TYPES.has(routeType) || METRO_TYPES.has(routeType)) return 'tram'
  return null
}

// ── Types ──

export interface GtfsStop {
  stop_id: string
  lat: number
  lon: number
  name: string
}

export interface StopTrainCount {
  stop_id: string
  lat: number
  lon: number
  name: string
  family: 'rail' | 'tram'
  trains_passenger: number
  trains_freight: number
}

/** Sum passenger departures of stops at the same rounded location (~11 m) + family
 *  into one row — merges platforms of one station AND the same physical stop
 *  appearing in several feeds/subfeeds of a nested feed (au-vic's Southern Cross
 *  carries V/Line + the interstate service under one stop_id in feeds 1 and 10 —
 *  /gg Codex confirmed they would otherwise not sum, since the downstream picks
 *  the single nearest stop per segment, not their total). Distinct platforms keep
 *  distinct coords, so cross-track counts are NOT over-summed. Freight is left at
 *  the first entry's value, unmerged — GTFS rarely carries freight service, so
 *  this is negligible in practice. Generic over `S` so callers whose
 *  `StopTrainCount`-shaped type adds an extra family (ae's 'metro') still get
 *  the ONE shared implementation. */
export function dedupeStopsByLocation<S extends { lat: number; lon: number; family: string; trains_passenger: number }>(
  stops: readonly S[],
): S[] {
  const byLoc = new Map<string, S>()
  for (const sc of stops) {
    const key = `${sc.lat.toFixed(4)}_${sc.lon.toFixed(4)}_${sc.family}`
    const existing = byLoc.get(key)
    if (existing) existing.trains_passenger += sc.trains_passenger
    else byLoc.set(key, { ...sc })
  }
  return [...byLoc.values()]
}

// ── Stop↔segment proximity join ──

/** Nearest GTFS stop within `radiusM` of a railway row's segment, looked up in the
 *  0.01°-cell stop grid every per-square enricher builds. The longitude cell span is
 *  latitude-aware: 0.01° lon shrinks below 500 m past ~63°N (Scandinavian feeds),
 *  where a fixed 3×3 window silently under-searched (/gg Codex W4 — codified from
 *  the old inline joins, now fixed once here). Latitude cells are ~1.11 km, so ±1
 *  always covers a 500 m radius. Known, accepted limit: the search anchors on the
 *  row MIDPOINT — a stop near the far END of a very long segment can sit outside
 *  the window (preserved pre-purge behavior; segments are 6-120 m, negligible).
 *  ONE source of truth for this join: the per-country `match` closures AND the
 *  OLD_FALLBACK retract corroborations must use the same join, otherwise a row
 *  could be disowned that `match` would still claim (or vice versa). */
export function nearestGridStop<S extends { lat: number; lon: number }>(
  grid: ReadonlyMap<string, S[]>,
  row: { startLat: number; startLon: number; endLat: number; endLon: number; midLat: number; midLon: number },
  radiusM = 500,
): S | null {
  let bestDist = radiusM
  let best: S | null = null
  const gy = Math.floor(row.midLat * 100), gx = Math.floor(row.midLon * 100)
  // Cells needed so dx spans radiusM of longitude at this latitude.
  const lonCellM = 1111.949 * Math.max(0.05, Math.cos((row.midLat * Math.PI) / 180))
  const dxMax = Math.max(1, Math.ceil(radiusM / lonCellM))
  for (let dy = -1; dy <= 1; dy++) for (let dx = -dxMax; dx <= dxMax; dx++) {
    const cell = grid.get(`${gy + dy}_${gx + dx}`)
    if (!cell) continue
    for (const sc of cell) {
      const d = pointToSegmentDist(sc.lat, sc.lon, row.startLat, row.startLon, row.endLat, row.endLon)
      if (d < bestDist) { bestDist = d; best = sc }
    }
  }
  return best
}

/** Build the one global 0.01-degree stop index used by z9 tram rows.
 * Exact point-to-segment distance keeps square boundaries irrelevant. */
export function buildTramExtraMatch<S extends {
  lat: number
  lon: number
  family: string
  trains_passenger: number
  trains_freight: number
}>(
  tramStops: readonly S[],
  sourceId: number,
): (row: RailwayRow, index: number, square: string) => RailwayTraffic | null {
  const grid = new Map<string, S[]>()
  for (const stop of tramStops) {
    if (stop.family !== 'tram') continue
    const key = `${Math.floor(stop.lat * 100)}_${Math.floor(stop.lon * 100)}`
    const cell = grid.get(key)
    if (cell) cell.push(stop)
    else grid.set(key, [stop])
  }
  return (row) => {
    if (row.railType !== 1 && row.railType !== 2) return null
    const stop = nearestGridStop(grid, row)
    return stop ? {
      passenger: stop.trains_passenger,
      freight: stop.trains_freight,
      sourceId,
      divisor: 1,
    } : null
  }
}

// ── CSV parsing ──

/** Parse a single CSV line, handling quoted fields with commas. */
export function parseCsvLine(line: string): string[] {
  const fields: string[] = []
  let current = ''
  let inQuotes = false
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]
    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < line.length && line[i + 1] === '"') {
          current += '"'
          i++
        } else {
          inQuotes = false
        }
      } else {
        current += ch
      }
    } else {
      if (ch === '"') {
        inQuotes = true
      } else if (ch === ',') {
        fields.push(current.trim())
        current = ''
      } else {
        current += ch
      }
    }
  }
  fields.push(current.trim())
  return fields
}

/** Stream-parse a large CSV file line by line. */
export async function parseCsvStream(filePath: string): Promise<Record<string, string>[]> {
  const results: Record<string, string>[] = []
  const stream = createReadStream(filePath, { encoding: 'utf-8' })
  const rl = createInterface({ input: stream, crlfDelay: Infinity })

  let headers: string[] | null = null
  for await (const rawLine of rl) {
    const line = headers === null ? rawLine.replace(/^\uFEFF/, '') : rawLine
    if (line.trim() === '') continue

    if (!headers) {
      headers = parseCsvLine(line)
      continue
    }
    const values = parseCsvLine(line)
    const row: Record<string, string> = {}
    for (let i = 0; i < headers.length; i++) {
      row[headers[i]] = values[i] || ''
    }
    results.push(row)
  }
  return results
}

/** Feed-declared dates bound timetable sampling even when recurring calendars span years. */
export async function readGtfsFeedWindow(extractDir: string): Promise<{ firstServiceDate: string; lastServiceDate: string }> {
  const path = resolve(extractDir, 'feed_info.txt')
  const rows = existsSync(path) ? await parseCsvStream(path) : []
  if (rows.length > 1) throw new Error(`${path}: expected at most one feed_info row`)
  const firstServiceDate = rows[0]?.['feed_start_date'] ?? ''
  const lastServiceDate = rows[0]?.['feed_end_date'] ?? ''
  for (const date of [firstServiceDate, lastServiceDate]) {
    if (date && !/^\d{8}$/.test(date)) throw new Error(`${path}: invalid service date '${date}'`)
  }
  if (firstServiceDate && lastServiceDate && firstServiceDate > lastServiceDate) {
    throw new Error(`${path}: reversed service window`)
  }
  return { firstServiceDate, lastServiceDate }
}

// ── Date helpers ──

export function parseGtfsDate(yyyymmdd: string): number {
  const y = parseInt(yyyymmdd.substring(0, 4))
  const m = parseInt(yyyymmdd.substring(4, 6)) - 1
  const d = parseInt(yyyymmdd.substring(6, 8))
  return Date.UTC(y, m, d)
}

function formatUtcGtfsDate(date: Date): string {
  const year = String(date.getUTCFullYear()).padStart(4, '0')
  const month = String(date.getUTCMonth() + 1).padStart(2, '0')
  const day = String(date.getUTCDate()).padStart(2, '0')
  return `${year}${month}${day}`
}

export function formatDate(yyyymmdd: string): string {
  return `${yyyymmdd.substring(0, 4)}-${yyyymmdd.substring(4, 6)}-${yyyymmdd.substring(6, 8)}`
}

/** GTFS `HH:MM:SS` (hour may exceed 23 for past-midnight trips) -> seconds since
 *  midnight, or -1 if unparseable. Shared by `gtfs-stop-pairs.ts` and
 *  `enrich-railway-th.ts`'s `frequencies.txt` headway expansion. */
export function parseTime(s: string): number {
  const m = /^(\d+):(\d+):(\d+)$/.exec(s.trim())
  if (!m) return -1
  return parseInt(m[1]) * 3600 + parseInt(m[2]) * 60 + parseInt(m[3])
}

/** Daily multipliers for GTFS headway templates; absent trips run once. */
export async function readGtfsTripDepartureMultipliers(
  extractDir: string,
  activeTripIds: ReadonlySet<string>,
): Promise<Map<string, number>> {
  const path = resolve(extractDir, 'frequencies.txt')
  const multipliers = new Map<string, number>()
  if (!existsSync(path)) return multipliers
  for (const row of await parseCsvStream(path)) {
    const tripId = row['trip_id']
    if (!activeTripIds.has(tripId)) continue
    const startSeconds = parseTime(row['start_time'] || '')
    const endSeconds = parseTime(row['end_time'] || '')
    const headwaySeconds = parseInt(row['headway_secs'] || '0', 10)
    if (startSeconds < 0 || endSeconds < 0 || headwaySeconds <= 0) continue
    if (endSeconds <= startSeconds) {
      throw new Error(`frequencies.txt has non-positive interval for trip '${tripId}'`)
    }
    const departures = Math.max(1, Math.floor((endSeconds - startSeconds) / headwaySeconds))
    multipliers.set(tripId, (multipliers.get(tripId) ?? 0) + departures)
  }
  return multipliers
}

/**
 * Pick a representative Wednesday via the calendar-midpoint heuristic: take the
 * midpoint of the overall calendar validity span (earliest start_date .. latest
 * end_date across all rows) and snap forward to the nearest Wednesday. Returns
 * an empty date when the feed supplies no usable calendar span.
 *
 * Note: this is the cheap midpoint variant. `findBusiestWednesday` below
 * preserves the continental producer's denser service-day sampling.
 */
export function findTargetWednesday(calendarRows: Record<string, string>[]): string {
  let minDate = '99999999'
  let maxDate = '00000000'
  for (const row of calendarRows) {
    const start = row['start_date'] || ''
    const end = row['end_date'] || ''
    if (start && start < minDate) minDate = start
    if (end && end > maxDate) maxDate = end
  }

  if (minDate === '99999999') return ''

  const startMs = parseGtfsDate(minDate)
  const endMs = parseGtfsDate(maxDate)
  const midMs = startMs + (endMs - startMs) / 2
  const mid = new Date(midMs)
  const day = mid.getUTCDay()
  const offset = (3 - day + 7) % 7
  mid.setUTCDate(mid.getUTCDate() + offset)
  return formatUtcGtfsDate(mid)
}

const WEEKDAY_COLUMNS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'] as const
const DAY_MS = 86_400_000

function busiestCalendarDateForWeekday(
  calendarRows: readonly Record<string, string>[],
  weekday: number,
): { date: string; count: number } {
  const events = new Map<number, number>()
  for (const row of calendarRows) {
    if (row[WEEKDAY_COLUMNS[weekday]] !== '1') continue
    const startMs = parseGtfsDate(row['start_date'] || '')
    const endMs = parseGtfsDate(row['end_date'] || '')
    if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || startMs > endMs) continue
    const startDay = new Date(startMs).getUTCDay()
    const firstMs = startMs + ((weekday - startDay + 7) % 7) * DAY_MS
    if (firstMs > endMs) continue
    const lastMs = firstMs + Math.floor((endMs - firstMs) / (7 * DAY_MS)) * 7 * DAY_MS
    events.set(firstMs, (events.get(firstMs) ?? 0) + 1)
    events.set(lastMs + 7 * DAY_MS, (events.get(lastMs + 7 * DAY_MS) ?? 0) - 1)
  }
  let active = 0
  let bestCount = 0
  let bestMs = 0
  for (const [dateMs, change] of [...events].sort((a, b) => a[0] - b[0])) {
    active += change
    if (active > bestCount) {
      bestCount = active
      bestMs = dateMs
    }
  }
  return { date: bestCount > 0 ? formatUtcGtfsDate(new Date(bestMs)) : '', count: bestCount }
}

/** Prefer the busiest Wednesday; if none runs, use the busiest real service day. */
export function findBusiestWednesday(calendarRows: Record<string, string>[]): string {
  if (calendarRows.length === 0) return findTargetWednesday(calendarRows)
  const wednesday = busiestCalendarDateForWeekday(calendarRows, 3)
  if (wednesday.count > 0) return wednesday.date
  let best = { date: '', count: 0 }
  for (let weekday = 0; weekday < 7; weekday++) {
    const candidate = busiestCalendarDateForWeekday(calendarRows, weekday)
    if (candidate.count > best.count ||
        (candidate.count === best.count && candidate.date && candidate.date < best.date)) best = candidate
  }
  return best.date || findTargetWednesday(calendarRows)
}

// ── Active trip families (routes.txt + calendar + trips.txt) ──

export interface ActiveTripFamiliesResult<F extends string> {
  /** trip_id -> its GTFS-route family, already filtered to trips active on `targetDate`. */
  tripFam: Map<string, F>
  /** Resolved service day (YYYYMMDD), or '' when no rail/tram routes or no calendar data exist. */
  targetDate: string
  /** True when calendar.txt OR calendar_dates.txt exists — i.e. there is a basis to
   *  determine which service ids ran on `targetDate`, even if that set turns out empty.
   *  Distinguishing this from `activeServiceIds.size > 0` is the 2026-07-15 fix below. */
  calendarPresent: boolean
  /** The resolved active service_id set for `targetDate`. Can be legitimately empty
   *  when `calendarPresent` is true (a broken/expired feed serves zero trips that day). */
  activeServiceIds: Set<string>
}

/**
 * routes.txt + calendar(_dates).txt + trips.txt → the trip_id -> family map every GTFS
 * rail matcher needs, shared between the per-stop frequency counter below
 * (`computeStopFrequenciesForFeed`) and the station-pair parser (`gtfs-stop-pairs.ts`).
 * `familyOf` lets callers plug in their own route_type -> family classification (europe's
 * per-feed allow-list + metroAsRail override, the pair parser's rail-only filter);
 * `dateSelection` lets callers plug in their own service-day picker (europe's
 * busiest-Wednesday sampler) in place of the default midpoint-Wednesday heuristic
 * (`findTargetWednesday`).
 */
/** routes.txt rows, failing LOUD on a malformed file (2026-07-16 review fix,
 *  item 3): an unreadable file (propagated stream error), a header-only/empty
 *  file, or a header without `route_type` is a PARSE FAILURE and must never
 *  yield the same empty result a genuine tram-only/bus-only feed yields —
 *  callers treat a throw as feed-not-loaded (retract-unsafe), never as
 *  "this feed legitimately has no rail". Shared by
 *  `computeActiveTripFamiliesForFeed` and `declaredRouteFamiliesForFeed` so
 *  the two can never disagree on what counts as malformed. Exported for
 *  callers that need the RAW rows once for several derived views (DE's
 *  route_type-histogram + declares-rail preflight builds both from one
 *  parse, enrich-railway-de.ts) — reuse this, never a bare parseCsvStream,
 *  so the malformed-file contract stays in one place. */
export async function parseRoutesTxtOrThrow(extractDir: string): Promise<Record<string, string>[]> {
  const path = resolve(extractDir, 'routes.txt')
  const rows = await parseCsvStream(path)
  if (rows.length === 0) {
    throw new Error(`routes.txt parse failure (${path}): header-only or empty — malformed feed, not a rail-less one`)
  }
  if (!('route_type' in rows[0])) {
    throw new Error(`routes.txt parse failure (${path}): no route_type column — malformed feed, not a rail-less one`)
  }
  return rows
}

/** The set of families this feed's routes.txt DECLARES under `familyOf` —
 *  the per-feed evidence base for BIDIRECTIONAL retract completeness
 *  (2026-07-16 review fix, item 4): a feed is complete only when EVERY family
 *  it declares parsed non-empty (declares 'rail' → station pairs > 0;
 *  declares 'tram' → tram stops > 0; declares neither, e.g. a bus-only feed
 *  like PT's Carris or MX's Toluca, → exempt from both, so a legitimately
 *  empty feed can no longer pin retractSafe false forever). Data-driven from
 *  the extract itself — replaces every hand-maintained per-country
 *  "which feeds carry heavy rail" set, which is exactly where CA's West Coast
 *  Express (TransLink route_type 2) got missed. Throws on malformed routes.txt
 *  (see `parseRoutesTxtOrThrow`).
 *
 *  `familyOf` must be the SAME classification the caller parses with — a feed
 *  whose pair contribution is deliberately narrowed (PL's warsaw-ztm) passes
 *  its narrowed classifier here too, so the narrowed-away family is never
 *  demanded back as completeness evidence. */
export async function declaredRouteFamiliesForFeed<F extends string>(
  extractDir: string,
  familyOf: (routeType: number) => F | null,
): Promise<Set<F>> {
  const routesRaw = await parseRoutesTxtOrThrow(extractDir)
  const declared = new Set<F>()
  for (const r of routesRaw) {
    const fam = familyOf(parseInt(r['route_type'] || '3'))
    if (fam) declared.add(fam)
  }
  return declared
}

/** '' when this feed's parsed outputs satisfy every family its routes.txt
 *  declares; otherwise the per-feed detail for the retract-skipped log line.
 *  `tramStopCount === null` = per-feed tram counts are not measurable this
 *  run (the merged v2 stop cache was served, which cannot attribute stops per
 *  feed) — the tram direction is then vouched for by the cache's OWN recorded
 *  provenance (whose recording rule is bidirectional since this fix) and only
 *  re-verified on rebuild. */
export function describeIncompleteFamilies(
  feedId: string,
  declared: ReadonlySet<string>,
  pairCount: number,
  tramStopCount: number | null,
): string {
  const parts: string[] = []
  if (declared.has('rail') && pairCount === 0) parts.push('declares rail but 0 station pairs parsed')
  if (tramStopCount !== null && declared.has('tram') && tramStopCount === 0) parts.push('declares tram but 0 tram stops parsed')
  return parts.length === 0 ? '' : `${feedId}: ${parts.join('; ')}`
}

/** Reject a sampled service day that omits a family present in routes.txt. */
export function describeInactiveFamilies<F extends string>(
  feedId: string,
  declared: ReadonlySet<F>,
  active: ReadonlySet<F>,
): string {
  const missing = [...declared].filter(family => !active.has(family)).sort()
  return missing.length === 0 ? '' : `${feedId}: no active ${missing.join('+')} trips on selected service day`
}

export async function computeActiveTripFamiliesForFeed<F extends string>(
  extractDir: string,
  familyOf: (routeType: number) => F | null,
  dateSelection?: (calendarRows: Record<string, string>[]) => string,
): Promise<ActiveTripFamiliesResult<F>> {
  // Malformed routes.txt THROWS here (2026-07-16 review fix, item 3) — both the
  // pair parser and the per-stop counter ride this one reader, so neither can
  // mistake a broken feed for a legitimately rail-less one.
  const routesRaw = await parseRoutesTxtOrThrow(extractDir)
  const routeFam = new Map<string, F>()
  for (const r of routesRaw) {
    const fam = familyOf(parseInt(r['route_type'] || '3'))
    if (fam) routeFam.set(r['route_id'], fam)
  }
  if (routeFam.size === 0) {
    return { tripFam: new Map(), targetDate: '', calendarPresent: false, activeServiceIds: new Set() }
  }

  const tripsRaw = await parseCsvStream(resolve(extractDir, 'trips.txt'))
  const eligibleTrips: Array<{ row: Record<string, string>; family: F }> = []
  const eligibleServiceIds = new Set<string>()
  for (const row of tripsRaw) {
    const family = routeFam.get(row['route_id'])
    if (!family) continue
    eligibleTrips.push({ row, family })
    eligibleServiceIds.add(row['service_id'])
  }

  const calendarPath = resolve(extractDir, 'calendar.txt')
  const calendarDatesPath = resolve(extractDir, 'calendar_dates.txt')
  const activeServiceIds = new Set<string>()
  let targetDate = ''
  const calendarPresent = existsSync(calendarPath) || existsSync(calendarDatesPath)

  // Exact-dates GTFS convention (SE Trafiklab, common Nordic/DE producers): calendar.txt
  // exists but EVERY weekday flag is 0 on every row — the rows only declare validity
  // spans, real activation lives in calendar_dates.txt add/remove exceptions. Detected
  // 2026-07-16 on SE: the weekday-driven path sampled zero services on every Wednesday,
  // fell to the span-midpoint fallback, and picked a date outside the operator's rolling
  // exception horizon (SL trams: 0 active services => "feed empty" => the completeness
  // gate blocked the world-wide legacy retract). Such a feed must take the
  // calendar_dates-only path below instead.
  const { firstServiceDate, lastServiceDate } = await readGtfsFeedWindow(extractDir)
  const withinFeedWindow = (date: string) =>
    (!firstServiceDate || date >= firstServiceDate) && (!lastServiceDate || date <= lastServiceDate)
  const calendarRaw = existsSync(calendarPath) ? (await parseCsvStream(calendarPath))
    .filter(row => eligibleServiceIds.has(row['service_id']))
    .map((row): Record<string, string> => ({ ...row,
      start_date: firstServiceDate && row['start_date'] < firstServiceDate ? firstServiceDate : row['start_date'],
      end_date: lastServiceDate && row['end_date'] > lastServiceDate ? lastServiceDate : row['end_date'],
    }))
    .filter(row => row.start_date <= row.end_date) : null
  const calDates = existsSync(calendarDatesPath) ? (await parseCsvStream(calendarDatesPath))
    .filter(row => eligibleServiceIds.has(row['service_id']) && withinFeedWindow(row['date'])) : []
  const weekdayDriven = calendarRaw !== null && calendarRaw.some((r) =>
    ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'].some((d) => r[d] === '1'))

  const exceptionsByDate = new Map<string, Record<string, string>[]>()
  for (const row of calDates) {
    const rows = exceptionsByDate.get(row['date'])
    if (rows) rows.push(row)
    else exceptionsByDate.set(row['date'], [row])
  }
  const requiredFamilies = new Set(routeFam.values())
  const tripCountsByService = new Map<string, Map<F, number>>()
  for (const { row, family } of eligibleTrips) {
    let counts = tripCountsByService.get(row['service_id'])
    if (!counts) { counts = new Map<F, number>(); tripCountsByService.set(row['service_id'], counts) }
    counts.set(family, (counts.get(family) ?? 0) + 1)
  }
  const activateDate = (date: string, recurring: boolean): void => {
    activeServiceIds.clear()
    if (recurring && calendarRaw !== null) {
      const weekdayColumn = WEEKDAY_COLUMNS[new Date(parseGtfsDate(date)).getUTCDay()]
      for (const row of calendarRaw) {
        const start = row['start_date'] || ''
        const end = row['end_date'] || ''
        if (row[weekdayColumn] === '1' && date >= start && date <= end) activeServiceIds.add(row['service_id'])
      }
    }
    for (const row of exceptionsByDate.get(date) ?? []) {
      if (row['exception_type'] === '1') activeServiceIds.add(row['service_id'])
      if (row['exception_type'] === '2') activeServiceIds.delete(row['service_id'])
    }
  }
  const activeTripSummary = (): { complete: boolean; trips: number } => {
    const families = new Set<F>()
    let trips = 0
    for (const serviceId of activeServiceIds) {
      for (const [family, count] of tripCountsByService.get(serviceId) ?? []) {
        families.add(family)
        trips += count
      }
    }
    return { complete: [...requiredFamilies].every(family => families.has(family)), trips }
  }
  const rangeDates = (first: string, last: string, wednesdayOnly: boolean): string[] => {
    const firstMs = parseGtfsDate(first), lastMs = parseGtfsDate(last)
    if (!first || !last || !Number.isFinite(firstMs) || !Number.isFinite(lastMs) || firstMs > lastMs) return []
    const dates: string[] = []
    for (let value = firstMs; value <= lastMs; value += DAY_MS) {
      const date = new Date(value)
      if (!wednesdayOnly || date.getUTCDay() === 3) dates.push(formatUtcGtfsDate(date))
    }
    return dates
  }
  const chooseCompleteDate = (
    candidates: readonly string[], recurring: boolean, preferred: string, busiest: boolean,
  ): string => {
    let best = { date: '', trips: -1, distance: Number.POSITIVE_INFINITY }
    const preferredMs = preferred ? parseGtfsDate(preferred) : 0
    for (const date of candidates) {
      activateDate(date, recurring)
      const summary = activeTripSummary()
      if (!summary.complete) continue
      const distance = preferred ? Math.abs(parseGtfsDate(date) - preferredMs) : 0
      if ((busiest && summary.trips > best.trips) ||
          (!busiest && (distance < best.distance || (distance === best.distance && summary.trips > best.trips)))) {
        best = { date, trips: summary.trips, distance }
      }
    }
    return best.date
  }

  if (calendarRaw !== null && weekdayDriven) {
    let first = firstServiceDate, last = lastServiceDate
    for (const row of calendarRaw) {
      if (!first || row['start_date'] < first) first = row['start_date']
      if (!last || row['end_date'] > last) last = row['end_date']
    }
    const preferred = (dateSelection ?? findTargetWednesday)(calendarRaw)
    if (!withinFeedWindow(preferred)) throw new Error(`${extractDir}: selected service day ${preferred} outside feed_info window`)
    const busiest = dateSelection === findBusiestWednesday
    activateDate(preferred, true)
    targetDate = activeTripSummary().complete ? preferred :
      chooseCompleteDate(rangeDates(first, last, true), true, preferred, busiest) ||
      chooseCompleteDate(rangeDates(first, last, false), true, preferred, busiest) || preferred
    activateDate(targetDate, true)
  } else if (existsSync(calendarDatesPath)) {
    const dateCounts = [...exceptionsByDate]
      .map(([date, rows]) => [date, rows.filter(row => row['exception_type'] === '1').length] as const)
      .filter(([, count]) => count > 0)
    const ranked = dateCounts.sort((a, b) => b[1] - a[1])
    const rankedWednesdays = ranked.filter(([date]) => new Date(parseGtfsDate(date)).getUTCDay() === 3)
    const preferred = (rankedWednesdays[0] ?? ranked[0])?.[0] ?? ''
    if (preferred) activateDate(preferred, false)
    targetDate = activeTripSummary().complete ? preferred :
      chooseCompleteDate(rankedWednesdays.map(([date]) => date), false, preferred, true) ||
      chooseCompleteDate(ranked.map(([date]) => date), false, preferred, true) || preferred
    if (targetDate) activateDate(targetDate, false)
  }
  // else: no calendar files at all — calendarPresent is false, every rail/tram trip below counts.

  const tripFam = new Map<string, F>()
  for (const { row: r, family: fam } of eligibleTrips) {
    // BUG FIX (2026-07-15): this used to gate on `activeServiceIds.size > 0`, so a
    // calendar.txt (or calendar_dates.txt) present but resolving to ZERO active services
    // on the target date — an expired or malformed feed, not a rare case — silently
    // counted EVERY trip as running instead of none. Correct semantics: calendar data
    // present means the (possibly empty) active set is authoritative; only when there is
    // NO calendar data at all do we fall back to "count everything". Changes counts for
    // broken feeds only — intentional.
    if (calendarPresent && !activeServiceIds.has(r['service_id'])) continue
    tripFam.set(r['trip_id'], fam)
  }

  return { tripFam, targetDate, calendarPresent, activeServiceIds }
}

// ── Merged stop-frequency cache with feed provenance ──
// The merged CACHE_FREQUENCIES snapshot used to be a bare stops array written
// UNCONDITIONALLY — a partial-feed run could poison it and every later
// cache-served run would both under-enrich and (worse) pass a partial snapshot
// to the retract. v2 records which feeds contributed non-empty parses, and the
// enrichers only persist complete snapshots.

/** `stops` is generic: per-country enrichers carry slightly different
 *  StopTrainCount shapes (ae adds a 'metro' family). */
interface MergedStopCacheV2<T> {
  v: 2
  feedsLoadedNonEmpty: string[]
  stops: T[]
}

export function writeMergedStopCache<T>(path: string, feedsLoadedNonEmpty: string[], stops: T[]): void {
  const payload: MergedStopCacheV2<T> = { v: 2, feedsLoadedNonEmpty, stops }
  writeFileSync(path, JSON.stringify(payload))
}

export function readMergedStopCache<T>(path: string): { stops: T[]; feedsLoadedNonEmpty: string[] | null } {
  const parsed = JSON.parse(readFileSync(path, 'utf-8')) as MergedStopCacheV2<T>
  return { stops: parsed.stops ?? [], feedsLoadedNonEmpty: parsed.feedsLoadedNonEmpty ?? [] }
}

// ── Stops with coordinates + parent-station resolution ──

/** ONE border margin (degrees) for BOTH sides of the GTFS geometry envelope
 *  (2026-07-16 /gg fix batch item 6): the stops kept by `loadStopsWithCoords`
 *  AND the rail-graph country bbox (`enrich-railway-europe.ts`'s
 *  `countryBboxFor`) must pad by the SAME margin — the old mismatch (stops
 *  1°, graph 0.5°) let cross-border stops form pairs whose graph end was
 *  never loaded, so every such pair snap-failed and its endpoint-radius
 *  quarantine froze retract/silent around the border for no real reason.
 *  Tradeoff of 0.5° (~55 km): a genuinely cross-border line running farther
 *  out than that loses its foreign tail's pairs — accepted for now; revisit
 *  per-country in the dedicated cross-border sweep. */
export const GTFS_BORDER_MARGIN_DEG = 0.5

export interface StopsWithCoords {
  /** stop_id -> parsed stop, valid-coords (and in-bounds, when `bbox` was given) only. */
  stopsMap: Map<string, GtfsStop>
  /** child stop_id -> parent_station id, for every stop that declares one — whether or
   *  not the child itself has valid coords (a coordless platform still needs its parent
   *  looked up). */
  childToParent: Map<string, string>
  skippedNoCoords: number
  skippedOutOfBounds: number
}

/**
 * stops.txt -> coordinate map + parent-station index, shared by the per-stop frequency
 * counter (`computeStopFrequenciesForFeed`) and the station-pair parser
 * (`gtfs-stop-pairs.ts`). `bbox` (padded by `GTFS_BORDER_MARGIN_DEG`, the SAME margin
 * the rail-graph country bbox uses — see the constant's doc) drops stops far outside
 * the country the caller cares about; omit it to keep every stop with valid coordinates.
 */
export async function loadStopsWithCoords(
  extractDir: string,
  bbox?: readonly [number, number, number, number],
): Promise<StopsWithCoords> {
  const stopsRaw = await parseCsvStream(resolve(extractDir, 'stops.txt'))
  const stopsMap = new Map<string, GtfsStop>()
  let skippedNoCoords = 0
  let skippedOutOfBounds = 0

  for (const r of stopsRaw) {
    const lat = parseFloat(r['stop_lat'] || '')
    const lon = parseFloat(r['stop_lon'] || '')
    if (!Number.isFinite(lat) || !Number.isFinite(lon) ||
        lat < -90 || lat > 90 || lon < -180 || lon > 180) {
      skippedNoCoords++
      continue
    }

    if (bbox) {
      const [minLat, minLon, maxLat, maxLon] = bbox
      const m = GTFS_BORDER_MARGIN_DEG
      if (lat < minLat - m || lat > maxLat + m || lon < minLon - m || lon > maxLon + m) {
        skippedOutOfBounds++
        continue
      }
    }

    stopsMap.set(r['stop_id'], { stop_id: r['stop_id'], lat, lon, name: (r['stop_name'] || '').trim() })
  }

  const childToParent = new Map<string, string>()
  for (const r of stopsRaw) {
    const parentId = (r['parent_station'] || '').trim()
    if (parentId) childToParent.set(r['stop_id'], parentId)
  }

  return { stopsMap, childToParent, skippedNoCoords, skippedOutOfBounds }
}

/**
 * Resolve a stop_id to its coordinates, falling back to its parent_station when the stop
 * itself has no valid coords (a common pattern: platforms/child stops carry only a name,
 * the parent station carries the GPS). ONE source of truth for this fallback — both the
 * per-stop counter and the pair parser must apply it BEFORE giving up on a stop, or a bare
 * coordless-skip would silently break trip continuity on a platform-heavy feed (Rejseplanen,
 * DELFI, ...).
 */
export function resolveStopViaParent(
  stops: StopsWithCoords,
  stopId: string,
): { stop: GtfsStop | undefined; viaParent: boolean } {
  const direct = stops.stopsMap.get(stopId)
  if (direct) return { stop: direct, viaParent: false }
  const parentId = stops.childToParent.get(stopId)
  const viaParentStop = parentId ? stops.stopsMap.get(parentId) : undefined
  return { stop: viaParentStop, viaParent: viaParentStop !== undefined }
}

// ── Per-feed stop-frequency computation ──
// Composition of computeActiveTripFamiliesForFeed + loadStopsWithCoords above (was
// byte-identical inlined across 11 files pre-2026-07-15); bbox is the only
// per-country input, passed through as a parameter.

export async function computeStopFrequenciesForFeed(
  feed: { id: string },
  extractDir: string,
  bbox: readonly [number, number, number, number],
  // Route-type -> family classifier; defaults to the shared `routeFamily`
  // (metro groups with tram). AE/TH pass a metro-EXCLUDING override here
  // (2026-07-16 review fix, item 1): their pre-migration behavior IGNORED
  // metro-family stops on purpose — an interchange within 500 m does not put
  // metro trains on the tram/light-rail track — and the default's metro→tram
  // grouping would have silently revived those counts on the next rebuild.
  familyOf: (routeType: number) => 'rail' | 'tram' | null = routeFamily,
  dateSelection?: (calendarRows: Record<string, string>[]) => string,
): Promise<StopTrainCount[]> {
  console.log(`\n  [${feed.id}] Parsing GTFS files...`)
  const startTime = Date.now()

  console.log(`  Reading routes.txt, calendar, trips.txt...`)
  const { tripFam, targetDate, calendarPresent, activeServiceIds } =
    await computeActiveTripFamiliesForFeed(extractDir, familyOf, dateSelection)
  if (calendarPresent) {
    console.log(`  Target date: ${targetDate ? formatDate(targetDate) : '(unresolved)'}`)
  } else {
    console.log(`  WARNING: No calendar files. Counting all trips.`)
  }
  console.log(`  ${activeServiceIds.size} active service IDs on target date`)
  console.log(`  ${tripFam.size} rail/tram trips on target day`)

  if (tripFam.size === 0) {
    console.log(`  WARNING: No active rail trips (or no rail routes). Returning empty.`)
    return []
  }

  const tripDepartureMultipliers = await readGtfsTripDepartureMultipliers(extractDir, new Set(tripFam.keys()))

  // ── stop_times.txt (stream for large files) ──
  console.log(`  Reading stop_times.txt (streaming)...`)
  const stopDepartures = new Map<string, { rail: number; tram: number }>()

  const stStream = createReadStream(resolve(extractDir, 'stop_times.txt'), { encoding: 'utf-8' })
  const stRl = createInterface({ input: stStream, crlfDelay: Infinity })
  let stHeaders: string[] | null = null
  let stLines = 0
  let stMatched = 0
  let tripIdIdx = -1
  let stopIdIdx = -1
  let lastProgressTime = Date.now()

  for await (const rawLine of stRl) {
    const line = stHeaders === null ? rawLine.replace(/^\uFEFF/, '') : rawLine
    if (line.trim() === '') continue

    if (!stHeaders) {
      stHeaders = parseCsvLine(line)
      tripIdIdx = stHeaders.indexOf('trip_id')
      stopIdIdx = stHeaders.indexOf('stop_id')
      if (tripIdIdx < 0 || stopIdIdx < 0) {
        throw new Error(`stop_times.txt missing trip_id/stop_id. Found: ${stHeaders.join(', ')}`)
      }
      continue
    }

    stLines++
    const fields = parseCsvLine(line)
    const tripId = fields[tripIdIdx]
    const fam = tripFam.get(tripId)
    if (!fam) continue

    const stopId = fields[stopIdIdx]
    let counts = stopDepartures.get(stopId)
    if (!counts) { counts = { rail: 0, tram: 0 }; stopDepartures.set(stopId, counts) }
    counts[fam] += tripDepartureMultipliers.get(tripId) ?? 1
    stMatched++

    if (Date.now() - lastProgressTime > 10_000) {
      console.log(`    ... ${(stLines / 1e6).toFixed(1)}M lines, ${stMatched} rail stop-times`)
      lastProgressTime = Date.now()
    }
  }
  console.log(`  ${stLines} stop_times lines, ${stMatched} rail stop-times, ${stopDepartures.size} unique stops`)

  // ── stops.txt + parent-station resolution ──
  console.log(`  Reading stops.txt...`)
  const stops = await loadStopsWithCoords(extractDir, bbox)
  console.log(`  ${stops.stopsMap.size} stops with valid coords`)
  if (stops.skippedOutOfBounds > 0) console.log(`  Skipped (out of bounds): ${stops.skippedOutOfBounds}`)

  // ── Build final list ──
  const results: StopTrainCount[] = []
  let resolvedViaParent = 0

  for (const [stopId, counts] of stopDepartures) {
    const { stop, viaParent } = resolveStopViaParent(stops, stopId)
    if (!stop) continue
    if (viaParent) resolvedViaParent++

    for (const family of ['rail', 'tram'] as const) {
      if (counts[family] === 0) continue
      results.push({
        stop_id: stop.stop_id,
        lat: stop.lat,
        lon: stop.lon,
        name: stop.name,
        family,
        trains_passenger: counts[family],
        trains_freight: 0,
      })
    }
  }

  const deduped = dedupeStopsByLocation(results)

  console.log(`  [${feed.id}] ${deduped.length} stops with train counts (${resolvedViaParent} resolved via parent station)`)

  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)
  console.log(`  [${feed.id}] GTFS parsing took ${elapsed}s`)

  return deduped
}
