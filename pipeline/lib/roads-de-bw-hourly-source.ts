/** Baden-Württemberg 2025 Dauerzählstellen hourly counts: parse, validate, expose. */

import { createHash } from 'node:crypto'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, chmodSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { path7za } from '7zip-bin'
import type { RoadLoaderArguments } from './road-loader-cli.js'

export const BW_HOURLY_SOURCE_URL = 'https://mobidata-bw.de/de/dataset/stundenwerte_dauerzaehlstellen'
const DATASET_DIRECTORY = ['de', 'bw-hourly', '2025']
const SOURCE_MANIFEST = 'source-manifest.json'
const STATIONS_CSV = 'stations.csv'

/** Class groups mirror the BASt mapping (roads-de-source); So stays unknown. */
const CLASS_GROUPS = {
  light: ['Pkw', 'Lfw', 'PmA'],
  medium: ['Bus', 'LoA'],
  heavy: ['LmA', 'Sat'],
  moto: ['Mot'],
} as const
type ClassGroup = keyof typeof CLASS_GROUPS
const GROUPS = Object.keys(CLASS_GROUPS) as ClassGroup[]

export interface BwStationProfile {
  svznr: string
  /** Normalized road ref (`klasse` + `nummer`, spaces stripped) for matching. */
  ref: string
  klasse: string
  lat: number
  lon: number
  /** Observed window (station-months with at least one complete observed day). */
  windowFrom: string
  windowTo: string
  /** Complete observed station-days per month — honest coverage, no threshold. */
  daysByMonth: Record<string, number>
  /** Complete observed station-days across the window. */
  days: number
  status: string
  /** Raw period vehicle counts (day 07–19, evening 19–23, night 23–07 local). */
  counts: Partial<Record<ClassGroup, [number, number, number]>>
  /** Period shares of the 24 h volume; a class without counts stays absent. */
  shares: Partial<Record<ClassGroup, [number, number, number]>>
}

interface BwDataset {
  stations: BwStationProfile[]
  stationMonths: number
  completeDays: number
  rejectedDays: number
}

interface StationAccumulator {
  station: { ref: string; klasse: string; lat: number; lon: number }
  headerId: string
  daysByMonth: Record<string, number>
  /** Hourly totals per class group, index 0 = hour ending 01. */
  hourly: Partial<Record<ClassGroup, number[]>>
  /** So (unclassified) vehicles: counted, never mapped into a class share. */
  unknownVehicles: number
}

const CP1252 = new TextDecoder('windows-1252')
const ACCEPTED_QUALITY = new Set(['-', 'u'])

/** The 7zip-bin tarball ships `7za` without the execute bit (see railway-gtfs-feeds). */
function sevenZipExecutable(): string {
  if (!(statSync(path7za).mode & 0o111)) chmodSync(path7za, 0o755)
  return path7za
}

/** Hour columns 01..24 are ENDING hours: day = 08..19, evening = 20..23, night = 24 + 01..07. */
const PERIOD_OF_HOUR = (hour: number): number =>
  hour >= 8 && hour <= 19 ? 0 : hour >= 20 && hour <= 23 ? 1 : 2

function parseStationRows(text: string): Map<string, { ref: string; klasse: string; lat: number; lon: number }> {
  const stations = new Map<string, { ref: string; klasse: string; lat: number; lon: number }>()
  const lines = text.split(/\r?\n/).filter(line => line.trim() !== '')
  const header = lines[0].split(',')
  const columns = Object.fromEntries(['svznr', 'klasse', 'nummer', 'gpsx1', 'gpsy1'].map(name => [
    name, header.indexOf(name),
  ]))
  if (Object.values(columns).some(index => index < 0)) throw new Error('BW stations.csv missing required columns')
  for (const line of lines.slice(1)) {
    const row = line.split(',')
    const svznr = row[columns.svznr]
    const lon = Number(row[columns.gpsx1])
    const lat = Number(row[columns.gpsy1])
    const klasse = row[columns.klasse]
    const nummer = row[columns.nummer].replace(/\s+/g, '')
    if (!/^\d+$/.test(svznr) || !Number.isFinite(lat) || !Number.isFinite(lon) ||
        !/^[A-Z]/.test(klasse) || !/^\d/.test(nummer)) {
      throw new Error(`invalid BW stations.csv row: ${line}`)
    }
    stations.set(svznr, { ref: `${klasse}${nummer}`, klasse, lat, lon })
  }
  return stations
}

/** `YYMMDD` matching the member month and a real calendar day of that month. */
function validDate(date: string, year: number, month: number): boolean {
  if (!/^\d{6}$/.test(date)) return false
  const day = Number(date.slice(4, 6))
  return Number(date.slice(0, 2)) === year % 100 && Number(date.slice(2, 4)) === month &&
    day >= 1 && day <= new Date(Date.UTC(year, month, 0)).getUTCDate()
}

/** Parse one station-month CSV text into complete observed days (flags -/u only). */
export function parseStationMonth(
  text: string,
  month: number,
  station: { ref: string; klasse: string; lat: number; lon: number },
  headerId: string,
  totals: Map<string, StationAccumulator>,
): { completeDays: number; rejectedDays: number } {
  const lines = text.split(/\r?\n/).filter(line => line.trim() !== '')
  if (lines.length <= 4) throw new Error(`BW station month ${headerId}/${month} has no data rows`)
  const stationHeader = lines[0].split(';')
  const directionHeader = lines[1].split(';')
  const types = lines[2].split(';')
  const header = lines[3].split(';')
  if (types[1] !== '09') return { completeDays: 0, rejectedDays: 0 } // not all nine classes measured
  const lanes = Number(directionHeader[0]) + Number(directionHeader[1])
  if (!Number.isInteger(lanes) || lanes < 1) throw new Error(`BW ${headerId}/${month}: invalid lane header`)
  const names: string[] = []
  const laneClasses = [...GROUPS.flatMap(group => CLASS_GROUPS[group]), 'So']
  for (const cls of laneClasses) for (let lane = 1; lane <= lanes; lane++) names.push(`${cls}FS${lane}`)
  const indices = names.map(name => header.indexOf(name))
  const checks = names.map(name => header.indexOf(`K_${name}`))
  if (indices.some(i => i < 0) || checks.some(i => i < 0)) {
    throw new Error(`BW ${headerId}/${month}: missing class/quality columns`)
  }
  const YEAR = 2025
  // A complete day needs all 24 hours observed; duplicates, missing or flagged
  // hours reject the WHOLE day (never zero-filled) — qualification parity.
  const days = new Map<string, Map<number, { counts: Partial<Record<ClassGroup, number>>; unknown: number } | null>>()
  const invalidDays = new Set<string>()
  for (const line of lines.slice(4)) {
    const row = line.split(';')
    const date = row[0]
    const hour = Number(row[1].slice(0, 2))
    if (!validDate(date, YEAR, month) || !/^(0[1-9]|1\d|2[0-4]):00$/.test(row[1]) || row.length < header.length) {
      throw new Error(`BW ${headerId}/${month}: malformed row ${line.slice(0, 40)}`)
    }
    const day = days.get(date) ?? new Map()
    if (day.has(hour)) invalidDays.add(date)
    const quality = checks.map(i => row[i])
    const values = indices.map(i => row[i])
    if (quality.some(q => !ACCEPTED_QUALITY.has(q)) ||
        values.some(v => !/^\d+$/.test(v))) {
      day.set(hour, null) // hour present but NOT observed: blank is not zero either
    } else {
      const counts: Partial<Record<ClassGroup, number>> = {}
      let unknown = 0
      let offset = 0
      for (const cls of laneClasses) {
        let sum = 0
        for (let k = 0; k < lanes; k++) sum += Number(values[offset + k])
        offset += lanes
        if (cls === 'So') unknown = sum
        else {
          const group = GROUPS.find(g => CLASS_GROUPS[g].includes(cls as never))!
          counts[group] = (counts[group] ?? 0) + sum
        }
      }
      const marked = day.get(hour)
      if (marked === null) continue // duplicate of a rejected hour stays rejected
      day.set(hour, { counts, unknown })
    }
    days.set(date, day)
  }
  let completeDays = 0
  let rejectedDays = 0
  const accumulator = totals.get(headerId) ?? {
    station, headerId: stationHeader[0] + stationHeader[1], daysByMonth: {}, hourly: {}, unknownVehicles: 0,
  }
  for (const [date, day] of days) {
    if (invalidDays.has(date) || day.size !== 24) { rejectedDays++; continue }
    let complete = true
    for (let hour = 1; hour <= 24; hour++) if (!day.get(hour)) complete = false
    if (!complete) { rejectedDays++; continue }
    completeDays++
    for (let hour = 1; hour <= 24; hour++) {
      const observed = day.get(hour)!
      accumulator.unknownVehicles += observed.unknown
      for (const group of GROUPS) {
        const hourly = accumulator.hourly[group] ?? Array<number>(24).fill(0)
        hourly[hour - 1] += observed.counts[group] ?? 0
        accumulator.hourly[group] = hourly
      }
    }
  }
  accumulator.daysByMonth[String(month).padStart(2, '0')] = completeDays
  totals.set(headerId, accumulator)
  return { completeDays, rejectedDays }
}

export function stationProfiles(totals: Map<string, StationAccumulator>): BwDataset {
  const stations: BwStationProfile[] = []
  let completeDays = 0
  for (const [svznr, accumulator] of totals) {
    // Window spans months WITH observed days; daysByMonth keeps zero-months
    // too — honest coverage, never an implicit threshold.
    const months = Object.entries(accumulator.daysByMonth)
      .filter(([, days]) => days > 0)
      .map(([month]) => month)
      .sort()
    const days = months.reduce((sum, month) => sum + accumulator.daysByMonth[month], 0)
    completeDays += days
    if (days === 0) continue
    const counts: BwStationProfile['counts'] = {}
    const shares: BwStationProfile['shares'] = {}
    for (const group of GROUPS) {
      const hourly = accumulator.hourly[group]
      const total = hourly?.reduce((sum, v) => sum + v, 0) ?? 0
      if (!hourly || total === 0) continue // class without observations stays unknown
      const periods = [0, 1, 2].map(period =>
        hourly.reduce((sum, v, hour) => sum + (PERIOD_OF_HOUR(hour + 1) === period ? v : 0), 0))
      counts[group] = periods as [number, number, number]
      shares[group] = periods.map(v => v / total) as [number, number, number]
    }
    stations.push({
      svznr, ref: accumulator.station.ref, klasse: accumulator.station.klasse,
      lat: accumulator.station.lat, lon: accumulator.station.lon,
      windowFrom: `2025-${months[0]}`, windowTo: `2025-${months[months.length - 1]}`,
      daysByMonth: accumulator.daysByMonth, days,
      status: `flags -/u only; ${accumulator.headerId === svznr ? 'header id = svznr' : `header id ${accumulator.headerId} ≠ svznr`}; combined both-direction lanes (per-lane sides, not directional proof); So unclassified vehicles ${accumulator.unknownVehicles} counted but excluded from class shares`,
      counts, shares,
    })
  }
  return { stations, stationMonths: totals.size, completeDays, rejectedDays: 0 }
}

function validateShares(shares: BwStationProfile['shares'], context: string): void {
  for (const [group, periods] of Object.entries(shares)) {
    if (periods.length !== 3 || periods.some(v => !Number.isFinite(v) || v < 0) ||
        Math.abs(periods.reduce((a, b) => a + b, 0) - 1) > 1e-9) {
      throw new Error(`BW profile ${context}: invalid ${group} shares`)
    }
  }
}

function sha256(bytes: Buffer): string {
  return createHash('sha256').update(bytes).digest('hex')
}

interface PinnedFile {
  file: string
  url: string
  sha256: string
  bytes: number
}

/** Verify every pinned file of the dataset; a partial dataset is invalid, not absent. */
function verifyPinned(directory: string, manifest: { files: PinnedFile[] }): PinnedFile[] {
  const archives: PinnedFile[] = []
  for (const pinned of manifest.files) {
    const path = resolve(directory, pinned.file)
    if (!existsSync(path)) throw new Error(`BW hourly dataset incomplete: ${pinned.file} missing`)
    const bytes = readFileSync(path)
    if (sha256(bytes) !== pinned.sha256 || bytes.length !== pinned.bytes) {
      throw new Error(`BW hourly dataset corrupt: ${pinned.file} failed sha256/size verification`)
    }
    if (/^Stundenwerte_\d{6}\.zip$/.test(pinned.file) && /^https:\/\/mobidata-bw\.de\//.test(pinned.url)) {
      archives.push(pinned)
    }
  }
  if (archives.length === 0) throw new Error('BW hourly dataset has no archives')
  return archives
}

/** Extract each archive once to bounded scratch, parse members, clean up. */
function parseArchives(
  directory: string,
  archives: readonly PinnedFile[],
  stations: Map<string, { ref: string; klasse: string; lat: number; lon: number }>,
): BwDataset {
  const totals = new Map<string, StationAccumulator>()
  let stationMonths = 0
  let completeDays = 0
  let rejectedDays = 0
  for (const archive of archives) {
    const scratch = mkdtempSync(join(tmpdir(), 'bw-hourly-'))
    try {
      // `e` flattens member paths into scratch: members parse by basename.
      const extraction = spawnSync(sevenZipExecutable(), ['e', '-y', `-o${scratch}`, resolve(directory, archive.file)], { encoding: 'utf8' })
      if (extraction.status !== 0) throw new Error(`7za extract failed for ${archive.file}: ${extraction.stderr.trim()}`)
      for (const name of readdirSync(scratch).sort()) {
        if (!name.endsWith('.csv')) continue
        // Member names look like `BW_62221202_2501.csv`: the FILENAME svz id
        // joins the map (never the first header field); `_YYMM` fixes the month.
        const parts = name.slice(0, -4).split('_')
        const svznr = parts[1]
        const month = Number(parts[2]?.slice(2, 4))
        if (!/^\d+$/.test(svznr) || !Number.isInteger(month) || month < 1 || month > 12) {
          throw new Error(`BW hourly archive ${archive.file}: unparseable member ${name}`)
        }
        const station = stations.get(svznr)
        if (!station) continue // station without map coordinates cannot be matched
        stationMonths++
        const parsed = parseStationMonth(
          CP1252.decode(readFileSync(join(scratch, name))), month, station, svznr, totals)
        completeDays += parsed.completeDays
        rejectedDays += parsed.rejectedDays
      }
    } finally {
      rmSync(scratch, { recursive: true, force: true })
    }
  }
  const dataset = stationProfiles(totals)
  for (const station of dataset.stations) validateShares(station.shares, station.svznr)
  return { ...dataset, stationMonths, completeDays, rejectedDays }
}

/** Load the pinned BW hourly dataset; `null` only when the dataset is absent
 * (no manifest at all). Present-but-incomplete fails loudly. No derived cache:
 * the manifest's sha256 identities ARE the cache truth for a 52 MB source,
 * and nothing is ever written into the frozen input root. */
export async function loadBwHourlyProfiles(
  options: RoadLoaderArguments,
): Promise<BwDataset | null> {
  const directory = resolve(options.enrichmentDirectory, ...DATASET_DIRECTORY)
  const manifestPath = resolve(directory, SOURCE_MANIFEST)
  if (!existsSync(directory)) return null
  if (!existsSync(manifestPath)) throw new Error(`BW hourly dataset incomplete: ${SOURCE_MANIFEST} missing`)
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
    files: PinnedFile[]; station_metadata_date?: string
  }
  if (!Array.isArray(manifest.files) || typeof manifest.station_metadata_date !== 'string') {
    throw new Error(`BW hourly source manifest invalid: ${manifestPath}`)
  }
  const archives = verifyPinned(directory, manifest)
  const stationsPath = resolve(directory, STATIONS_CSV)
  if (!existsSync(stationsPath)) throw new Error(`BW hourly dataset incomplete: ${STATIONS_CSV} missing`)
  const stations = parseStationRows(readFileSync(stationsPath, 'utf8'))
  const dataset = parseArchives(directory, archives, stations)
  return { ...dataset, stations: dataset.stations.map(station => ({
    ...station,
    status: `${station.status}; counts year 2025, station map ${manifest.station_metadata_date}`,
  })) }
}
