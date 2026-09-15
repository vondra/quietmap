/** FHWA TMAS 2025 hourly TOTAL volumes: parse, validate, expose. Monthly VOL
 * files carry no vehicle-class breakdown, so the product of this module is a
 * per-station-direction TOTAL period share — a transferred estimate per
 * vehicle class, never a measured class profile. Observation keys are state +
 * station + direction + lane + date (one station-description record per key,
 * TMG §4.1). Scope conventions verified against the official format page:
 * https://www.fhwa.dot.gov/policyinformation/tmguide/tmg_2022/traffic-data-formats.cfm
 * — Table 4-4: Travel_Dir 1–8 are individual compass directions, 0 = E–W (or
 * SE–NW) COMBINED and 9 = N–S (or NE–SW) COMBINED (neither is unknown);
 * Table 4-5: Travel_Lane 0 = combined lanes, 1–9 individual lanes; §4.3
 * field 11: Restrictions 0 = none. */

import { createHash } from 'node:crypto'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, chmodSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { path7za } from '7zip-bin'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import type { RankedPoint } from './spatial.js'

export const TMAS_SOURCE_URL = 'https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/'
/** Canonical field-semantics reference for the non-obvious scope codes. */
export const TMAS_FORMAT_REFERENCE =
  'https://www.fhwa.dot.gov/policyinformation/tmguide/tmg_2022/traffic-data-formats.cfm'
const DATASET_DIRECTORY = ['us', 'tmas', '2025']
const SOURCE_MANIFEST = 'source-manifest.json'
const YEAR = 2025
const MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'] as const
const COMBINED_DIRECTIONS = new Set(['0', '9'])
/** A combined direction duplicates the individual directions it spans. */
const isCombined = (direction: string): boolean => COMBINED_DIRECTIONS.has(direction)

export interface TmasStationProfile extends RankedPoint {
  /** Dictionary identity: state + Station_Id + direction scope, e.g. `CA021560:D1`. */
  station: string
  /** Posted route number (`Posted_Route_Sign_Number`), '' when unsigned or ambiguous. */
  routeNumber: string
  latitude: number
  longitude: number
  /** Normalized HPMS class minus one; null when the source code is unknown. */
  rank: number | null
  /** Observed window (station-direction-months with complete observed days). */
  windowFrom: string
  windowTo: string
  /** Complete observed days per month — honest coverage, no threshold. */
  daysByMonth: Record<string, number>
  /** Complete observed days across the window. */
  days: number
  status: string
  /** Raw TOTAL period counts (day 07–19, evening 19–23, night 23–07 local). */
  counts: [number, number, number]
  /** TOTAL period shares of the 24 h volume — a transferred estimate per class. */
  shares: [number, number, number]
}

export interface TmasDataset {
  stations: TmasStationProfile[]
  observationRows: number
  rejectedRows: number
  /** Rows/months rejected per cause; missing hours are never zero-filled. */
  rejected: Record<string, number>
}

/** Station metadata for ONE (state, station, direction) scope: coordinates
 * average over that direction's S rows (lanes of one direction share a
 * point); a posted route number survives only when unanimous; `lanes` are the
 * STA-declared Travel_Lane keys the VOL scope must be supported by. */
interface DirectionMetadata {
  latSum: number
  lonSum: number
  rows: number
  routes: Set<string>
  lanes: Set<string>
  /** First standard-domain (1–7) functional class seen in this direction. */
  fSystem: number | null
  rawFSystem: string
  inconsistentFSystem: boolean
}

/** One direction-month: days are complete only within the month's lane scope;
 * the yearly lane universe decides at assembly whether the month counts. */
interface DirectionMonth {
  /** Calendar month 1–12 (the archives parse in calendar order). */
  month: number
  /** Sorted observed lane keys joined with '+' (e.g. `0`, `1+2`). */
  laneScope: string
  days: number
  hourly: Float64Array
}

const LATIN1 = new TextDecoder('iso-8859-1')

/** TMAS hours are STARTING hours (Hour_00 = 00:00–00:59): day = 07–19,
 * evening = 19–23, night = 23–07 local. */
const PERIOD_OF_HOUR = (hour: number): number =>
  hour >= 7 && hour < 19 ? 0 : hour >= 19 && hour < 23 ? 1 : 2

/** The 7zip-bin tarball ships `7za` without the execute bit (see railway-gtfs-feeds). */
function sevenZipExecutable(): string {
  if (!(statSync(path7za).mode & 0o111)) chmodSync(path7za, 0o755)
  return path7za
}

function splitHeader(text: string): { columns: Map<string, number>; lines: string[] } {
  const lines = text.split(/\r?\n/).filter(line => line.trim() !== '')
  const columns = new Map(lines[0].split('|').map((name, index) => [name, index]))
  return { columns, lines }
}

function requireColumns(columns: Map<string, number>, names: readonly string[], member: string): number[] {
  const indices = names.map(name => columns.get(name) ?? -1)
  if (indices.some(index => index < 0)) throw new Error(`TMAS member ${member} missing required columns`)
  return indices
}

function sha256(bytes: Buffer): string {
  return createHash('sha256').update(bytes).digest('hex')
}

interface PinnedFile {
  file: string
  url: string
  sha256: string
  bytes: number
  members?: number
}

/** Verify every pinned file of the dataset; a partial dataset is invalid,
 * not absent. The manifest's anchored hashes are the verification truth —
 * this loader generates no receipts of its own. */
function verifyPinned(directory: string, manifest: { files: PinnedFile[] }): Map<string, PinnedFile> {
  const archives = new Map<string, PinnedFile>()
  for (const pinned of manifest.files) {
    const path = resolve(directory, pinned.file)
    if (!existsSync(path)) throw new Error(`TMAS dataset incomplete: ${pinned.file} missing`)
    const bytes = readFileSync(path)
    if (sha256(bytes) !== pinned.sha256 || bytes.length !== pinned.bytes) {
      throw new Error(`TMAS dataset corrupt: ${pinned.file} failed sha256/size verification`)
    }
    if (pinned.members !== undefined) archives.set(pinned.file, pinned)
  }
  const monthly = MONTHS.map(month => `${month}_2025_ccs_data.zip`)
  if (monthly.some(file => !archives.has(file)) || ![...archives.keys()].some(file => /station/i.test(file))) {
    throw new Error(`TMAS dataset pins ${[...archives.keys()].length} archives, expected 12 monthly + stations`)
  }
  return archives
}

function stateOfMember(name: string): string {
  const state = name.split('_')[0]
  if (!/^[A-Z]{2}$/.test(state)) throw new Error(`TMAS member ${name}: unparseable state prefix`)
  return state
}

const count = (rejected: Record<string, number>, cause: string): void => {
  rejected[cause] = (rejected[cause] ?? 0) + 1
}

// Legacy numeric F_System crosswalk: FHWA HPMS Appendix H.
// https://www.fhwa.dot.gov/policyinformation/hpms/fieldmanual/page16.cfm
// Codes not documented there stay unknown and require a matching route at transfer.
const FUNCTIONAL_CLASS_OF_F_SYSTEM = new Map([
  ['1', 1], ['2', 3], ['6', 4], ['7', 5], ['8', 6], ['9', 7],
  ['11', 1], ['12', 2], ['14', 3], ['16', 4], ['17', 5], ['19', 7],
])

/** Station-description records: one S row per (state, station, direction,
 * lane) — TMG §4.1. Station_Id is state-local alphanumeric (`0F0002`,
 * `N0801E`); rows without usable coordinates are incomplete inputs, skipped. */
export function parseStationMetadata(
  text: string,
  state: string,
  member: string,
  metadata: Map<string, DirectionMetadata>,
): void {
  const { columns, lines } = splitHeader(text)
  const [type, stationId, travelDir, travelLane, latitude, longitude, signing, signNumber] = requireColumns(
    columns, ['Record_Type', 'Station_Id', 'Travel_Dir', 'Travel_Lane', 'Latitude', 'Longitude', 'Posted_Route_Signing', 'Posted_Route_Sign_Number'], member)
  for (const line of lines.slice(1)) {
    const row = line.split('|')
    if (row[type] !== 'S') continue
    const id = row[stationId]?.trim() ?? ''
    const direction = row[travelDir]?.trim() ?? ''
    const lane = row[travelLane]?.trim() ?? ''
    const lat = Number(row[latitude])
    const lon = Number(row[longitude])
    if (!/^[A-Za-z0-9][A-Za-z0-9-]*$/.test(id) || !/^[0-9]$/.test(direction) ||
        !/^[0-9]$/.test(lane) || !Number.isFinite(lat) || !Number.isFinite(lon)) continue
    const route = row[signing]?.trim() !== '' && /^\d+$/.test(row[signNumber]?.trim() ?? '')
      ? row[signNumber].replace(/\s+/g, '').replace(/^0+(?=\d)/, '')
      : ''
    const key = `${state}|${id}|${direction}`
    const scope = metadata.get(key) ?? {
      latSum: 0, lonSum: 0, rows: 0, routes: new Set(), lanes: new Set(),
      fSystem: null, rawFSystem: '', inconsistentFSystem: false,
    }
    scope.latSum += lat
    scope.lonSum += lon
    scope.rows++
    scope.lanes.add(lane)
    if (route !== '') scope.routes.add(route)
    metadata.set(key, scope)
  }
}

export interface VolumeMonthResult {
  observationRows: number
  rejectedRows: number
  rejected: Record<string, number>
}

/** Parse one state-month VOL text into per-direction month summaries. A
 * station-direction-day is complete only when EVERY lane observed that month
 * reports all 24 valid hours that day; missing/blank hours are never zero,
 * duplicate observations conflict, and restrictions other than blank/0
 * (TMG §4.3: 0 = none) exclude the day. Combined lane rows (Travel_Lane 0)
 * never mix with per-lane rows. */
export function parseVolumeMonth(
  text: string,
  state: string,
  month: number,
  member: string,
  metadata: ReadonlyMap<string, DirectionMetadata>,
  directionMonths: Map<string, DirectionMonth[]>,
  rejected: Record<string, number>,
): VolumeMonthResult {
  const { columns, lines } = splitHeader(text)
  const names = ['Record_Type', 'State_Code', 'F_System', 'Station_Id', 'Travel_Dir', 'Travel_Lane',
    'Year_Record', 'Month_Record', 'Day_Record', 'Restrictions',
    ...Array.from({ length: 24 }, (_, hour) => `Hour_${String(hour).padStart(2, '0')}`)]
  const indices = requireColumns(columns, names, member)
  const [type, stateCode, fSystem, stationId, travelDir, travelLane, yearRecord, monthRecord,
    dayRecord, restrictions] = indices
  const hourColumns = indices.slice(10)
  const lastDayOfMonth = new Date(Date.UTC(YEAR, month, 0)).getUTCDate()
  // direction key → lane → day-of-month → 24 hourly values (null = invalid).
  const observations = new Map<string, Map<string, Map<string, number[] | null>>>()
  let stateCodeValue = ''
  let observationRows = 0
  let rejectedRows = 0
  for (const line of lines.slice(1)) {
    const row = line.split('|')
    if (row[type] !== '3') continue
    if (stateCodeValue === '') stateCodeValue = row[stateCode]
    else if (row[stateCode] !== stateCodeValue) {
      throw new Error(`TMAS member ${member}: State_Code ${row[stateCode]} differs from ${stateCodeValue}`)
    }
    observationRows++
    const day = Number(row[dayRecord])
    if (row[yearRecord] !== String(YEAR) || Number(row[monthRecord]) !== month ||
        !Number.isInteger(day) || day < 1 || day > lastDayOfMonth) {
      count(rejected, 'invalid-date'); rejectedRows++; continue
    }
    const key = `${state}|${row[stationId]}|${row[travelDir]}`
    const lanes = observations.get(key) ?? new Map()
    const days = lanes.get(row[travelLane]) ?? new Map<string, number[] | null>()
    const dateKey = String(day)
    // Any second record for the same (direction, lane, day) — valid or not —
    // conflicts: the day is invalidated, never first-wins.
    if (days.has(dateKey)) {
      days.set(dateKey, null)
      count(rejected, 'duplicate-day')
      lanes.set(row[travelLane], days)
      observations.set(key, lanes)
      continue
    }
    // TMG §4.3 field 11: 0 = no restrictions; blank = unreported (the 2025
    // archive is uniformly blank). 1/3 change volume, 2 is malfunction, 4/5
    // change the traffic pattern — a restricted day stays observed-but-excluded
    // and also blocks a conflicting record of the same day.
    const restriction = row[restrictions]?.trim() ?? ''
    if (restriction !== '' && restriction !== '0') {
      days.set(dateKey, null)
      count(rejected, 'restricted-day'); rejectedRows++
      lanes.set(row[travelLane], days)
      observations.set(key, lanes)
      continue
    }
    const hours: number[] = []
    let valid = true
    for (let hour = 0; hour < 24; hour++) {
      const value = row[hourColumns[hour]]?.trim() ?? ''
      if (!/^\d+$/.test(value)) { valid = false; break } // blank/unknown is never zero
      hours.push(Number(value))
    }
    days.set(dateKey, valid ? hours : null)
    if (!valid) count(rejected, 'incomplete-hours')
    lanes.set(row[travelLane], days)
    observations.set(key, lanes)
    // Rank pinning per direction scope: legacy-numeric functional class (see
    // FUNCTIONAL_CLASS_OF_F_SYSTEM); disagreement or an out-of-domain code
    // leaves the scope class-agnostic.
    if (metadata.has(key)) {
      const scope = metadata.get(key)!
      const raw = row[fSystem]?.trim() ?? ''
      const functionalClass = FUNCTIONAL_CLASS_OF_F_SYSTEM.get(raw) ?? null
      if (scope.rawFSystem === '') scope.rawFSystem = raw
      if (functionalClass !== null) {
        if (scope.fSystem === null) scope.fSystem = functionalClass
        else if (scope.fSystem !== functionalClass) scope.inconsistentFSystem = true
      }
    }
  }
  for (const [key, lanes] of observations) {
    const scope = metadata.get(key)
    if (!scope) { count(rejected, 'unscoped-observation'); continue }
    // The VOL lane scope must be supported by the STA declaration (Table 4-5:
    // lane 0 = combined lanes, 1–9 individual).
    if ([...lanes.keys()].some(lane => !scope.lanes.has(lane))) {
      count(rejected, 'unsupported-lane')
      continue
    }
    if (!lanes.has('0') && [...scope.lanes].some(lane => lane !== '0' && !lanes.has(lane))) {
      count(rejected, 'missing-declared-lane'); continue
    }
    if (lanes.has('0') && lanes.size > 1) {
      count(rejected, 'conflicting-lane-scope') // combined row beside per-lane rows
      continue
    }
    const hourly = new Float64Array(24)
    let days = 0
    for (let day = 1; day <= lastDayOfMonth; day++) {
      const dateKey = String(day)
      let complete = true
      for (const laneDays of lanes.values()) {
        const hours = laneDays.get(dateKey)
        if (hours === undefined || hours === null) { complete = false; break }
      }
      if (!complete) continue
      days++
      for (const laneDays of lanes.values()) {
        const hours = laneDays.get(dateKey)!
        for (let hour = 0; hour < 24; hour++) hourly[hour] += hours[hour]
      }
    }
    const months = directionMonths.get(key) ?? []
    months.push({ month, laneScope: [...lanes.keys()].sort().join('+'), days, hourly })
    directionMonths.set(key, months)
    if (days === 0) count(rejected, 'month-without-complete-days')
  }
  return { observationRows, rejectedRows, rejected }
}

const DIRECTION_DESCRIPTION = (direction: string): string =>
  isCombined(direction)
    ? `direction ${direction} = ${direction === '0' ? 'E–W (or SE–NW)' : 'N–S (or NE–SW)'} COMBINED total (TMG Table 4-4)`
    : `direction ${direction} (individual compass, TMG Table 4-4)`

/** Assemble direction-scoped station profiles from the retained months. The
 * yearly lane universe is the union of the month lane scopes: a month missing
 * a lane the year observed is NOT complete station-direction coverage and is
 * excluded (never presented with the full scope). */
export function stationProfiles(
  metadata: ReadonlyMap<string, DirectionMetadata>,
  directionMonths: ReadonlyMap<string, DirectionMonth[]>,
  rejected: Record<string, number>,
): TmasStationProfile[] {
  // A combined direction duplicates the individual directions it spans: drop
  // the combined scope wherever the same station IN THE SAME STATE also
  // reports individuals (station ids are only unique within a state).
  const stations = new Map<string, Set<string>>()
  for (const key of directionMonths.keys()) {
    const [state, id, direction] = key.split('|')
    const directions = stations.get(`${state}|${id}`) ?? new Set<string>()
    directions.add(direction)
    stations.set(`${state}|${id}`, directions)
  }
  const droppedCombined = new Set<string>()
  for (const [station, directions] of stations) {
    if ([...directions].some(direction => !isCombined(direction))) {
      for (const direction of directions) {
        if (isCombined(direction)) droppedCombined.add(`${station}|${direction}`)
      }
    }
  }
  const profiles: TmasStationProfile[] = []
  for (const [key, months] of directionMonths) {
    const [state, id, direction] = key.split('|')
    if (droppedCombined.has(`${state}|${id}|${direction}`)) {
      count(rejected, 'combined-direction-duplicated')
      continue
    }
    const scope = metadata.get(key)
    if (!scope || scope.rows === 0 || scope.inconsistentFSystem) continue
    // Yearly lane scope: months report only the widest VOL-observed lane set.
    // Narrower months are excluded — a missing lane is not complete coverage
    // (observed coverage only, never a fabricated complete year).
    const byScope = new Map<string, DirectionMonth[]>()
    for (const month of months) {
      const list = byScope.get(month.laneScope) ?? []
      list.push(month)
      byScope.set(month.laneScope, list)
    }
    const [widestScope, widestMonths] = [...byScope.entries()].sort((a, b) =>
      b[0].split('+').length - a[0].split('+').length ||
      b[1].reduce((sum, m) => sum + m.days, 0) - a[1].reduce((sum, m) => sum + m.days, 0))[0]
    if (widestMonths.length !== months.length) {
      rejected['narrower-lane-scope-months'] = (rejected['narrower-lane-scope-months'] ?? 0) + months.length - widestMonths.length
    }
    const contributing = widestMonths.filter(month => month.days > 0)
    if (contributing.length === 0) continue
    const daysByMonth: Record<string, number> = {}
    for (const month of contributing) {
      daysByMonth[String(month.month).padStart(2, '0')] = month.days
    }
    const days = contributing.reduce((sum, month) => sum + month.days, 0)
    const total = contributing.reduce((sum, month) => sum + month.hourly.reduce((a, b) => a + b, 0), 0)
    if (days === 0 || total === 0) continue
    const hourly = contributing.reduce((sum, month) => {
      for (let hour = 0; hour < 24; hour++) sum[hour] += month.hourly[hour]
      return sum
    }, new Float64Array(24))
    const periods = [0, 1, 2].map(period =>
      hourly.reduce((sum, v, hour) => sum + (PERIOD_OF_HOUR(hour) === period ? v : 0), 0))
    const routes = [...scope.routes]
    const latitude = scope.latSum / scope.rows
    const longitude = scope.lonSum / scope.rows
    // Placeholder or out-of-territory coordinates cannot match any US road.
    if (latitude < 17.5 || latitude > 71.5 || longitude < -180 || longitude > -65) continue
    profiles.push({
      station: `${state}${id}:D${direction}`,
      routeNumber: routes.length === 1 ? routes[0] : '',
      latitude,
      longitude,
      rank: scope.fSystem === null ? null : scope.fSystem - 1,
      windowFrom: `${YEAR}-${String(contributing[0].month).padStart(2, '0')}`,
      windowTo: `${YEAR}-${String(contributing[contributing.length - 1].month).padStart(2, '0')}`,
      daysByMonth,
      days,
      status: `${DIRECTION_DESCRIPTION(direction)}; lanes ${widestScope === '0' ? '0=combined (Table 4-5)' : widestScope.split('+').join(', ')}${scope.lanes.size && [...scope.lanes].sort().join('+') !== widestScope ? ` (STA declares ${[...scope.lanes].sort().join(',')})` : ''}; F_System ${scope.rawFSystem === '' ? 'unreported' : scope.rawFSystem}${scope.fSystem === null ? ' → class unknown' : ''}; total hourly volumes only (no vehicle classes); restriction flag blank/0 accepted; restricted days excluded; FHWA TMAS state-reported data`,
      counts: periods as [number, number, number],
      shares: periods.map(v => v / total) as [number, number, number],
    })
  }
  return profiles.sort((a, b) => a.station.localeCompare(b.station))
}

/** Extract one archive to bounded scratch and hand each member to `parse`. */
function withArchiveMembers<T>(
  directory: string,
  archive: PinnedFile,
  parse: (name: string, bytes: Buffer) => T,
): T[] {
  const scratch = mkdtempSync(join(tmpdir(), 'us-tmas-'))
  try {
    const extraction = spawnSync(sevenZipExecutable(),
      ['e', '-y', `-o${scratch}`, resolve(directory, archive.file)], { encoding: 'utf8' })
    if (extraction.status !== 0) throw new Error(`7za extract failed for ${archive.file}: ${extraction.stderr.trim()}`)
    const parsed: T[] = []
    for (const name of readdirSync(scratch).sort()) {
      if (!/\.(VOL|STA)$/i.test(name)) continue
      parsed.push(parse(name, readFileSync(join(scratch, name))))
    }
    if (archive.members !== undefined && parsed.length !== archive.members) {
      throw new Error(`TMAS archive ${archive.file}: ${parsed.length} members, manifest pins ${archive.members}`)
    }
    return parsed
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

/** Load the pinned TMAS 2025 dataset; `null` only when the dataset is absent
 * (no directory at all). Present-but-incomplete fails loudly. Each archive is
 * extracted exactly once to bounded scratch and parsed member-by-member;
 * nothing is written into the frozen input root. */
export async function loadTmasProfiles(options: RoadLoaderArguments): Promise<TmasDataset | null> {
  const directory = resolve(options.enrichmentDirectory, ...DATASET_DIRECTORY)
  if (!existsSync(directory)) return null
  const manifestPath = resolve(directory, SOURCE_MANIFEST)
  if (!existsSync(manifestPath)) throw new Error(`TMAS dataset incomplete: ${SOURCE_MANIFEST} missing`)
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as { files: PinnedFile[]; year?: number }
  if (!Array.isArray(manifest.files) || manifest.year !== YEAR) {
    throw new Error(`TMAS source manifest invalid: ${manifestPath}`)
  }
  const archives = verifyPinned(directory, manifest)
  const metadata = new Map<string, DirectionMetadata>()
  const directionMonths = new Map<string, DirectionMonth[]>()
  const rejected: Record<string, number> = {}
  const stationArchive = [...archives.values()].find(archive => /station/i.test(archive.file))!
  withArchiveMembers(directory, stationArchive, (name, bytes) =>
    parseStationMetadata(LATIN1.decode(bytes), stateOfMember(name), name, metadata))
  let observationRows = 0
  let rejectedRows = 0
  for (const month of MONTHS) {
    const archive = archives.get(`${month}_2025_ccs_data.zip`)!
    for (const _ of withArchiveMembers(directory, archive, (name, bytes) => {
      const state = stateOfMember(name)
      const memberMonth = (MONTHS as readonly string[]).indexOf(name.split('_')[1]?.slice(0, 3).toLowerCase())
      if (memberMonth !== (MONTHS as readonly string[]).indexOf(month)) {
        throw new Error(`TMAS archive ${archive.file}: member ${name} is not a ${month} volume file`)
      }
      return parseVolumeMonth(LATIN1.decode(bytes), state, memberMonth + 1, name, metadata, directionMonths, rejected)
    })) {
      observationRows += _.observationRows
      rejectedRows += _.rejectedRows
    }
  }
  return {
    stations: stationProfiles(metadata, directionMonths, rejected),
    observationRows, rejectedRows, rejected,
  }
}
