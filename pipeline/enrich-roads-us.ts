/** Enrich z9 US roads with class-compatible FHWA HPMS 2022 traffic measurements
 * and observed TMAS 2025 hourly TOTAL period profiles. */

import { roadFeatureObservation } from './lib/pinned-road-lines.js'
import type { RoadObservation } from './lib/road-observation.js'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { writeCacheAtomically } from './lib/atomic-cache.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { iso2Code, listPreparedSquares, lonLatToGrid } from './lib/prepared-grid.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  applyRoadTimeProfiles, osmRoadClassRank, readRoadTimeProfilesSource, roadClassTakesCount,
  ROAD_CLASS_RANK_TOLERANCE, writeRoadAadt, type RoadRow, type RoadTimeProfileEntry,
} from './lib/roads-arrow.js'
import { SOURCE_ID_US_FHWA_HPMS, shouldOverwrite } from './lib/sources.js'
import {
  buildOneHundredthDegreePointGrid, buildOneHundredthDegreeSegmentGrid, haversineM, pointGridCandidates, pointSearchReach,
  pointToSegmentDist, runsAlongSegment, wrapLonDeltaDeg, type RankedPoint, type SegmentCoordinates,
} from './lib/spatial.js'
import { writeTmasProfileSquares } from './lib/roads-us-tmas-write.js'
import { ownSquareShard, writeNationalRoadSquares } from './lib/square-pool.js'
import { loadTmasProfiles, TMAS_SOURCE_URL, type TmasStationProfile } from './lib/roads-us-tmas-source.js'

const SOURCE_ID = SOURCE_ID_US_FHWA_HPMS
const coverage = DATASETS.find(dataset => dataset.id === SOURCE_ID)?.roadCoverage
if (!coverage) throw new Error('FHWA source has no registered road coverage')
const COVERED_ROAD_CLASSES = new Set(coverage)
const US_BBOX = [17.5, -180, 71.5, -65] as const
const PAGE_SIZE = 2000
const HPMS_BASE = 'https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0'
// Preserve dev1's class split: the national view publishes total AADT, not classes.
const HEAVY_SHARES = [0.12, 0.10, 0.08, 0.06, 0.05] as const

export interface UsRoadSegment extends RankedPoint, RoadObservation {
  aadt: number
  light: number
  medium: number
  heavy: number
  moto: number
  /** HPMS FACILITY_TYPE: 1 one-way roadway, 2 two-way, 4 ramp, 6 two-way part of a divided road. */
  facilityType: number | null
  /** The section's lines as flat `[longitude, latitude, …]` vertices; `latitude`/`longitude` is their centroid. */
  lines: readonly Float64Array[]
}

function sourceNumber(value: unknown, name: string): number {
  if ((typeof value !== 'number' && typeof value !== 'string') ||
      (typeof value === 'string' && value.trim() === '') ||
      !Number.isFinite(Number(value)) || Number(value) < 0) {
    throw new Error(`invalid FHWA ${name}: ${JSON.stringify(value)}`)
  }
  return Number(value)
}

function sectionLines(value: unknown): { lines: Float64Array[]; latitude: number; longitude: number } {
  if (!value || typeof value !== 'object') throw new Error('FHWA geometry is missing')
  const geometry = value as { type?: unknown; coordinates?: unknown }
  const lines = geometry.type === 'LineString' ? [geometry.coordinates]
    : geometry.type === 'MultiLineString' ? geometry.coordinates : null
  if (!Array.isArray(lines)) throw new Error('FHWA geometry must contain line coordinates')
  let latitude = 0
  let longitude = 0
  let count = 0
  const flat: Float64Array[] = []
  for (const line of lines) {
    if (!Array.isArray(line)) throw new Error('invalid FHWA line coordinates')
    const vertices = new Float64Array(line.length * 2)
    for (const [index, coordinate] of line.entries()) {
      if (!Array.isArray(coordinate) || coordinate.length < 2 ||
          typeof coordinate[0] !== 'number' || typeof coordinate[1] !== 'number' ||
          !Number.isFinite(coordinate[0]) || !Number.isFinite(coordinate[1]) ||
          coordinate[0] < -180 || coordinate[0] > 180 ||
          coordinate[1] < -90 || coordinate[1] > 90) {
        throw new Error(`invalid FHWA coordinate: ${JSON.stringify(coordinate)}`)
      }
      longitude += coordinate[0]
      latitude += coordinate[1]
      vertices[2 * index] = coordinate[0]
      vertices[2 * index + 1] = coordinate[1]
      count++
    }
    if (line.length) flat.push(vertices)
  }
  if (count === 0) throw new Error('FHWA line geometry is empty')
  return { lines: flat, latitude: latitude / count, longitude: longitude / count }
}

export function parseUsPage(page: unknown): { segments: UsRoadSegment[]; features: number } {
  if (!page || typeof page !== 'object' || 'error' in page ||
      !Array.isArray((page as { features?: unknown }).features)) {
    throw new Error('FHWA page has no valid features array')
  }
  const features = (page as { features: unknown[] }).features
  if (features.length > PAGE_SIZE) throw new Error('FHWA page exceeds requested record count')
  const segments: UsRoadSegment[] = []
  for (const feature of features) {
    if (!feature || typeof feature !== 'object') throw new Error('invalid FHWA feature')
    const properties = (feature as { properties?: unknown }).properties
    if (!properties || typeof properties !== 'object') throw new Error('FHWA properties are missing')
    const values = properties as Record<string, unknown>
    // Fourteen archived records have genuine .5 totals; dev1 truncates the
    // positive total BEFORE splitting classes. Do not reject or round them.
    const aadt = Math.trunc(sourceNumber(values.AADT, 'AADT'))
    if (!Number.isSafeInteger(aadt) || aadt > 2_147_483_647) throw new Error('FHWA AADT exceeds Int32')
    if (aadt === 0) continue
    const functionalClass = sourceNumber(values.F_SYSTEM, 'F_SYSTEM')
    if (!Number.isInteger(functionalClass)) throw new Error('FHWA F_SYSTEM must be an integer')
    const { lines, latitude, longitude } = sectionLines((feature as { geometry?: unknown }).geometry)
    if (latitude < US_BBOX[0] || latitude > US_BBOX[2] ||
        longitude < US_BBOX[1] || longitude > US_BBOX[3] ||
        functionalClass < 1 || functionalClass > HEAVY_SHARES.length) continue
    const rank = functionalClass - 1
    const moto = Math.round(aadt * 0.01)
    const totalHeavy = Math.round(aadt * HEAVY_SHARES[rank])
    const medium = Math.round(totalHeavy * 0.20)
    // HPMS Field Manual item 3: FACILITY_TYPE 1 is a one-way roadway and 4 a ramp; their AADT is
    // that one direction. Every other section reports the two-way total; the publisher leaves 118
    // of 235,257 archived sections null, whose scope stays unknown. A page cached without the
    // field would silently make every ramp a two-way mainline, so its absence is an error.
    if (!('FACILITY_TYPE' in values)) throw new Error('FHWA feature has no FACILITY_TYPE: download the snapshot again')
    const facilityType = values.FACILITY_TYPE === null ? null : sourceNumber(values.FACILITY_TYPE, 'FACILITY_TYPE')
    const countBasis = facilityType === null ? 'unknown' : facilityType === 1 || facilityType === 4 ? 'directional' : 'both-directions'
    segments.push({ ...roadFeatureObservation(feature as object, countBasis),
      latitude, longitude, rank, isRamp: facilityType === 4, aadt,
      light: aadt - totalHeavy - moto, medium, heavy: totalHeavy - medium, moto, facilityType, lines,
    })
  }
  return { segments, features: features.length }
}

/** Read the contiguous snapshot through its explicit empty terminal page. */
export async function loadUsSegments(options: RoadLoaderArguments): Promise<UsRoadSegment[]> {
  const directory = resolve(options.enrichmentDirectory, 'us')
  const segments: UsRoadSegment[] = []
  const pendingDownloads: Array<readonly [string, Buffer]> = []
  let previousPageWasPartial = false
  for (let offset = 0; ; offset += PAGE_SIZE) {
    const path = resolve(directory, `hpms-page-${offset}.json`)
    let bytes: Buffer
    const download = options.forceDownload || !existsSync(path)
    if (download) {
      if (options.enrichOnly) throw new Error(`FHWA cache page missing: ${path}`)
      const query = new URLSearchParams({
        where: 'AADT>0', outFields: 'AADT,F_SYSTEM,FACILITY_TYPE',
        f: 'geojson', outSR: '4326', resultOffset: String(offset),
        resultRecordCount: String(PAGE_SIZE), orderByFields: 'OBJECTID',
      })
      const response = await fetch(`${HPMS_BASE}/query?${query}`, { signal: AbortSignal.timeout(120_000) })
      if (!response.ok) throw new Error(`FHWA HTTP ${response.status} at offset ${offset}`)
      bytes = Buffer.from(await response.arrayBuffer())
    } else {
      bytes = readFileSync(path)
    }
    const parsed = parseUsPage(JSON.parse(bytes.toString('utf8')) as unknown)
    if (previousPageWasPartial && parsed.features !== 0) {
      throw new Error(`FHWA nonempty page follows a short page at offset ${offset}`)
    }
    if (download) pendingDownloads.push([path, bytes])
    if (parsed.features === 0) {
      const trailing = existsSync(directory) ? readdirSync(directory).filter(name => {
        const match = /^hpms-page-(\d+)\.json$/.exec(name)
        return match && Number(match[1]) > offset
      }) : []
      if (trailing.length) throw new Error(`FHWA cache continues beyond terminal page: ${trailing.join(', ')}`)
      if (segments.length === 0) throw new Error('FHWA snapshot has no usable traffic measurements')
      for (const [pendingPath, pendingBytes] of pendingDownloads) {
        writeCacheAtomically(pendingPath, pendingBytes)
      }
      return withTwoWayTwinsOfOneWaySections(segments)
    }
    segments.push(...parsed.segments)
    previousPageWasPartial = parsed.features < PAGE_SIZE
  }
}

// An FT1 section that repeats the two-way total of an FT6 section of the same system a few hundred
// metres away is the other state's or the older inventory's copy of that total, not one direction:
// 3,189 of 7,400 km of FT1 have such a twin (centroids <= 300 m, AADT within 15 %), FT6/FT1 median 1.00.
const TWIN_CENTROID_METRES = 300
const TWIN_AADT_RATIO_TOLERANCE = 0.15

/** Relabel FT1 sections with an equal FT6 twin as two-way totals; genuine one-way couplets stay directional. */
export function withTwoWayTwinsOfOneWaySections(segments: UsRoadSegment[]): UsRoadSegment[] {
  const twoWay = buildOneHundredthDegreePointGrid(segments.filter(segment => segment.facilityType === 6))
  return segments.map(segment => {
    if (segment.facilityType !== 1) return segment
    for (const twin of pointGridCandidates(segment.latitude, segment.longitude, TWIN_CENTROID_METRES, twoWay)) {
      if (twin.rank === segment.rank && Math.abs(segment.aadt / twin.aadt - 1) <= TWIN_AADT_RATIO_TOLERANCE &&
          haversineM(segment.latitude, segment.longitude, twin.latitude, twin.longitude) <= TWIN_CENTROID_METRES) {
        return { ...segment, countBasis: 'both-directions' as const }
      }
    }
    return segment
  })
}

// A section applies along its own line: 84 % of Interstate km lay more than 200 m from its section's
// centroid (I-95 Miami 44 km and I-405 Los Angeles 21 km fell to defaults, r260919).
const SECTION_LINE_REACH_METRES = 100

interface SectionBounds extends SegmentCoordinates { section: UsRoadSegment }

/** Index each section under every 0.01-degree cell of its bounds; exact distances come from its lines. */
export function indexUsSections(segments: readonly UsRoadSegment[]): ReadonlyMap<string, readonly SectionBounds[]> {
  return buildOneHundredthDegreeSegmentGrid(segments.map(section => {
    let south = 90, west = 180, north = -90, east = -180
    for (const line of section.lines) for (let index = 0; index < line.length; index += 2) {
      west = Math.min(west, line[index]); east = Math.max(east, line[index])
      south = Math.min(south, line[index + 1]); north = Math.max(north, line[index + 1])
    }
    return { section, startLatitude: south, startLongitude: west, endLatitude: north, endLongitude: east }
  }))
}

/** The class-compatible section whose nearest line piece runs along the row, nearest first. */
export function nearestUsSection(row: RoadRow, grid: ReadonlyMap<string, readonly SectionBounds[]>): UsRoadSegment | null {
  let closest: UsRoadSegment | null = null
  let closestDistance = SECTION_LINE_REACH_METRES
  for (const { section } of pointGridCandidates(row.midLat, row.midLon, SECTION_LINE_REACH_METRES, grid)) {
    if (!roadClassTakesCount(row.roadClass, section)) continue
    let distance = Infinity, piece: SegmentCoordinates | null = null
    for (const line of section.lines) {
      // A single-vertex line is one zero-length piece.
      for (let start = 0; start === 0 || start + 2 < line.length; start += 2) {
        const end = Math.min(start + 2, line.length - 2)
        const candidate = { startLongitude: line[start], startLatitude: line[start + 1],
          endLongitude: line[end], endLatitude: line[end + 1] }
        const along = pointToSegmentDist(row.midLat, row.midLon, candidate.startLatitude, candidate.startLongitude,
          candidate.endLatitude, candidate.endLongitude)
        if (along < distance) { distance = along; piece = candidate }
      }
    }
    if (piece && (distance < closestDistance || (distance === closestDistance && closest !== null &&
        section.observationId < closest.observationId)) && runsAlongSegment(row, piece)) {
      closest = section
      closestDistance = distance
    }
  }
  return closest
}

export async function enrichUsRoads(preparedDirectory: string, segments: readonly UsRoadSegment[]) {
  if (segments.length === 0) throw new Error('FHWA snapshot has no usable traffic measurements')
  const grid = indexUsSections(segments)
  const match = (row: RoadRow) => {
    if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
    const segment = nearestUsSection(row, grid)
    return segment ? { countBasis: segment.countBasis, observationId: segment.observationId,
      light: segment.light, medium: segment.medium, heavy: segment.heavy,
      moto: segment.moto, sourceId: SOURCE_ID,
    } : null
  }
  return writeNationalRoadSquares(preparedDirectory, US_BBOX, 'US', {}, path =>
    writeRoadAadt(path, match, undefined, COVERED_ROAD_CLASSES,
      { sourceIds: [SOURCE_ID], when: row => !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null }))
}

/** Digits tail of an OSM road ref ("US 101" → "101", "I 5" → "5"). */
function refTail(ref: string | null): string {
  const match = ref?.replace(/\s+/g, '').toUpperCase().match(/(\d+)[A-Z]?$/)
  return match ? match[1] : ''
}

/** Individual compass observations apply only to a matching one-way carriageway.
 * Combined observations remain explicit local transfers to either carriageway. */
function tmasDirectionMatches(row: RoadRow, station: TmasStationProfile): boolean {
  const scope = /:D([0-9])$/.exec(station.station)
  if (!scope) return false
  const direction = Number(scope[1])
  if (direction === 0 || direction === 9) return true
  if (row.oneway !== 1 && row.oneway !== 2) return false
  const east = wrapLonDeltaDeg(row.endLon - row.startLon) * Math.cos(row.midLat * Math.PI / 180)
  const north = row.endLat - row.startLat
  if (east === 0 && north === 0) return false
  const heading = Math.atan2(east, north) * 180 / Math.PI + (row.oneway === 2 ? 180 : 0)
  // Eight compass directions quantize bearing in 45-degree sectors.
  return Math.abs(wrapLonDeltaDeg(heading - (direction - 1) * 45)) <= 45
}

/** Nearest compatible US observation within 200 m, respecting route and direction. */
export function matchTmasStation(
  row: RoadRow,
  grid: ReadonlyMap<string, readonly TmasStationProfile[]>,
): TmasStationProfile | null {
  if (row.countryCode !== iso2Code('US')) return null
  const rowRank = osmRoadClassRank(row.roadClass)
  const refs = (row.ref ?? '').split(';').map(refTail).filter(Boolean)
  let closest: TmasStationProfile | null = null
  let closestDistance = 200
  for (const station of pointGridCandidates(row.midLat, row.midLon, closestDistance, grid)) {
    if (!tmasDirectionMatches(row, station)) continue
    if (station.rank !== null && Math.abs(station.rank - rowRank) > ROAD_CLASS_RANK_TOLERANCE) continue
    if (refs.length ? !refs.includes(station.routeNumber) : station.rank === null) continue
    const distance = haversineM(row.midLat, row.midLon, station.latitude, station.longitude)
    if (distance < closestDistance) { closest = station; closestDistance = distance }
  }
  return closest
}

function tmasProfileEntries(stations: readonly TmasStationProfile[]): RoadTimeProfileEntry[] {
  return stations.map(station => ({
    station: station.station,
    window: `${station.windowFrom}..${station.windowTo}`,
    days: station.days,
    status: `${station.status}; applied ≤200 m from station point, route/class-compatible, direction-gated, US-only rows; total share is a transferred estimate for every vehicle class`,
    profile: { total: station.shares },
  }))
}

/** Only owners intersecting the actual match radius, plus previously stamped owners. */
export function tmasCandidateSquares(
  stations: readonly TmasStationProfile[], preparedDirectory: string, existing: readonly string[],
): string[] {
  const existingSet = new Set(existing)
  const candidates = new Set<string>()
  for (const station of stations) {
    const [latReach, lonReach] = pointSearchReach(station.latitude, 200)
    const [west, south] = lonLatToGrid(station.longitude - lonReach, station.latitude - latReach)
    const [east, north] = lonLatToGrid(station.longitude + lonReach, station.latitude + latReach)
    // A z9 owner spans 2^(30-9) grid cells; tile Y runs southward.
    for (let x = Math.floor(west / 2 ** 21); x <= Math.floor(east / 2 ** 21); x++) {
      for (let gy = Math.floor(south / 2 ** 21); gy <= Math.floor(north / 2 ** 21); gy++) {
        const name = `z9/${(x + 512) % 512}/${511 - gy}`
        if (existingSet.has(name)) candidates.add(name)
      }
    }
  }
  for (const square of existing) {
    const source = readRoadTimeProfilesSource(resolve(preparedDirectory, square, 'roads.arrow'))
    if (source === TMAS_SOURCE_URL) candidates.add(square)
  }
  return [...candidates].sort()
}

/** Profile-only producer entry: applies observed TMAS TOTAL shares to the
 * prepared road copies without touching any traffic column — usable on
 * finalized copies for refreshes between AADT enrichment runs. */
export async function enrichTmasTimeProfiles(
  preparedDirectory: string, stations: readonly TmasStationProfile[],
): Promise<{ squares: number; rows: number; matched: number; squaresUpdated: number }> {
  const squares = tmasCandidateSquares(stations, preparedDirectory, listPreparedSquares(preparedDirectory, US_BBOX))
  return { squares: squares.length, ...await writeTmasProfileSquares(preparedDirectory, squares, stations) }
}

/** Shared serial shard body; each worker owns disjoint Arrow files. */
export async function applyTmasProfileSquares(
  preparedDirectory: string, squares: readonly string[], stations: readonly TmasStationProfile[],
) {
  const grid = buildOneHundredthDegreePointGrid(stations)
  const entries = tmasProfileEntries(stations)
  const indexOf = new Map(entries.map((entry, index) => [entry.station, index + 1]))
  return applyRoadTimeProfiles(preparedDirectory, squares, TMAS_SOURCE_URL, entries,
    row => indexOf.get(matchTmasStation(row, grid)?.station ?? '') ?? 0)
}

/** The normal US producer flow: HPMS AADT enrichment, then observed TMAS
 * TOTAL period profiles when the dataset is pinned (absent skips silently —
 * the AADT step stays independently runnable). */
export async function runUsEnrichment(options: RoadLoaderArguments) {
  const segments = await loadUsSegments(options)
  const result = { segments: segments.length,
    oneWaySegments: segments.filter(segment => segment.countBasis === 'directional' && !segment.isRamp).length,
    rampSegments: segments.filter(segment => segment.isRamp).length,
    unknownScopeSegments: segments.filter(segment => segment.countBasis === 'unknown').length,
    ...await enrichUsRoads(options.preparedDirectory, segments) }
  // The TMAS writer runs its own pool over a list read from file contents, so only the fan-out parent
  // runs it, and loads it after the walk so that it does not count as memory every shard repeats.
  const tmas = ownSquareShard ? null : await loadTmasProfiles(options)
  if (!tmas) return result
  const profiles = await enrichTmasTimeProfiles(options.preparedDirectory, tmas.stations)
  return {
    ...result,
    tmasStations: tmas.stations.length,
    tmasCompleteStationDays: tmas.stations.reduce((sum, station) => sum + station.days, 0),
    tmasRejectedRows: tmas.rejectedRows,
    tmasRejected: tmas.rejected,
    tmasMatched: profiles.matched,
    tmasSquares: profiles.squares,
    tmasSquaresUpdated: profiles.squaresUpdated,
  }
}

runRoadLoaderCli(import.meta.url, runUsEnrichment)
